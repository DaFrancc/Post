//! Anonymous tokens (design section 8, "Metadata protection").
//!
//! A logged-in device asks for blind signatures on random messages it has
//! blinded. Later, on an anonymous connection, it spends a (message,
//! signature) pair. Blinding is information-theoretically hiding: the server
//! cannot link a spent token to the session that obtained it.
//!
//! Scheme: RFC 9474 RSABSSA-SHA384-PSS-Randomized, RSA-2048. Not
//! post-quantum: a quantum computer could forge tokens (bypassing the spam
//! gate) but could not deanonymize them. Tokens carry a version so the scheme
//! can be replaced.

use blind_rsa_signatures::{
    DefaultRng, KeyPairSha384PSSRandomized as KeyPair, MessageRandomizer, PublicKeySha384PSSRandomized as PublicKey,
    SecretKeySha384PSSRandomized as SecretKey, Signature,
};
use postern_proto::{self as proto, ErrorCode};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::db::{Internal as _, Result, err, now_ms, sql_i64};

/// A key issues tokens for this long after it is created.
pub const ISSUE_FOR_MS: u64 = 7 * 24 * 3600 * 1000;
/// Tokens under a key are accepted for this long after it is created, so a
/// token issued at the end of the issuing window still has a week to be spent.
pub const VALID_FOR_MS: u64 = 2 * ISSUE_FOR_MS;
/// Tokens one device may obtain per day.
pub const DAILY_LIMIT: u32 = 200;
/// Most tokens one request may ask for.
pub const MAX_PER_REQUEST: usize = 100;
const RSA_BITS: usize = 2048;

/// The key currently issuing tokens.
pub struct IssuingKey {
    pub key_id: Vec<u8>,
    pub public_der: Vec<u8>,
    pub valid_until_ms: u64,
}

/// The current issuing key, creating one if none is still issuing.
pub fn current_key(conn: &Connection) -> Result<IssuingKey> {
    let now = now_ms();
    let row: Option<(Vec<u8>, Vec<u8>, i64)> = conn
        .query_row(
            "SELECT key_id, public_der, valid_until FROM token_keys WHERE created_at > ?1 ORDER BY created_at DESC LIMIT 1",
            [sql_i64(now.saturating_sub(ISSUE_FOR_MS))],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .db()?;
    if let Some((key_id, public_der, valid_until)) = row {
        return Ok(IssuingKey { key_id, public_der, valid_until_ms: valid_until as u64 });
    }
    let kp = KeyPair::generate(&mut DefaultRng, RSA_BITS).map_err(|_| err(ErrorCode::Internal, "token key generation failed"))?;
    let public_der = kp.pk.to_spki().map_err(|_| err(ErrorCode::Internal, "token key encoding failed"))?;
    let secret_der = zeroize::Zeroizing::new(kp.sk.to_der().map_err(|_| err(ErrorCode::Internal, "token key encoding failed"))?);
    let key_id = Sha256::digest(&public_der)[..8].to_vec();
    let valid_until = now + VALID_FOR_MS;
    conn.execute(
        "INSERT INTO token_keys (key_id, secret_der, public_der, created_at, valid_until) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![key_id, secret_der.as_slice(), public_der, sql_i64(now), sql_i64(valid_until)],
    )
    .db()?;
    tracing::info!(key_id = %hex::encode(&key_id), "new anonymous token key");
    Ok(IssuingKey { key_id, public_der, valid_until_ms: valid_until })
}

fn day(ms: u64) -> i64 {
    sql_i64(ms / (24 * 3600 * 1000))
}

/// Blind-sign `blinded` for a logged-in device, within its daily limit.
pub fn issue(conn: &Connection, device: &[u8], key_id: &[u8], blinded: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
    if blinded.is_empty() || blinded.len() > MAX_PER_REQUEST {
        return Err(err(ErrorCode::BadRequest, format!("ask for 1 to {MAX_PER_REQUEST} tokens")));
    }
    let current = current_key(conn)?;
    if current.key_id != key_id {
        return Err(err(ErrorCode::BadToken, "token key is not the current issuing key"));
    }
    let today = day(now_ms());
    let used: u32 = conn
        .query_row("SELECT count FROM token_issuance WHERE device_id = ?1 AND day = ?2", params![device, today], |r| r.get(0))
        .optional()
        .db()?
        .unwrap_or(0);
    if used.saturating_add(blinded.len() as u32) > DAILY_LIMIT {
        return Err(err(ErrorCode::RateLimited, format!("at most {DAILY_LIMIT} tokens per day")));
    }
    let secret: Vec<u8> = conn.query_row("SELECT secret_der FROM token_keys WHERE key_id = ?1", [key_id], |r| r.get(0)).db()?;
    let secret = zeroize::Zeroizing::new(secret);
    let sk = SecretKey::from_der(&secret).map_err(|_| err(ErrorCode::Internal, "stored token key is invalid"))?;
    let mut out = Vec::with_capacity(blinded.len());
    for b in blinded {
        let sig = sk.blind_sign(b).map_err(|_| err(ErrorCode::BadRequest, "invalid blinded message"))?;
        out.push(sig.0);
    }
    conn.execute(
        "INSERT INTO token_issuance (device_id, day, count) VALUES (?1, ?2, ?3)
         ON CONFLICT (device_id, day) DO UPDATE SET count = count + excluded.count",
        params![device, today, blinded.len() as u32],
    )
    .db()?;
    conn.execute("UPDATE token_keys SET issued = issued + ?1 WHERE key_id = ?2", params![blinded.len() as i64, key_id]).db()?;
    Ok(out)
}

/// Verify a token and mark it spent. Call inside the transaction of the
/// request it pays for, so a failed request does not burn the token.
pub fn spend(conn: &Connection, token: Option<&proto::Token>) -> Result<()> {
    let bad = |m: &str| err(ErrorCode::BadToken, m);
    let t = token.ok_or_else(|| bad("token required"))?;
    if t.version != proto::TOKEN_VERSION_RSABSSA_SHA384 {
        return Err(bad("unsupported token version"));
    }
    if t.message.len() != 32 {
        return Err(bad("token message must be 32 bytes"));
    }
    let randomizer: [u8; 32] = t.randomizer.as_slice().try_into().map_err(|_| bad("token randomizer must be 32 bytes"))?;
    let key: Option<(Vec<u8>, i64)> = conn
        .query_row("SELECT public_der, valid_until FROM token_keys WHERE key_id = ?1", [&t.key_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .db()?;
    let (public_der, valid_until) = key.ok_or_else(|| bad("unknown token key"))?;
    if valid_until <= sql_i64(now_ms()) {
        return Err(bad("token key has expired"));
    }
    let pk = PublicKey::from_spki(&public_der).map_err(|_| err(ErrorCode::Internal, "stored token key is invalid"))?;
    pk.verify(&Signature(t.signature.clone()), Some(MessageRandomizer(randomizer)), &t.message)
        .map_err(|_| bad("token signature is invalid"))?;
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO spent_tokens (key_id, message_hash) VALUES (?1, ?2)",
            params![t.key_id, Sha256::digest(&t.message).to_vec()],
        )
        .db()?;
    if n == 0 {
        return Err(bad("token already spent"));
    }
    // More spent than issued can only mean forged tokens: the token key leaked
    // or RSA was broken. Alert loudly; the operator should rotate the key.
    let (issued, spent): (i64, i64) = conn
        .query_row(
            "SELECT issued, (SELECT count(*) FROM spent_tokens WHERE key_id = ?1) FROM token_keys WHERE key_id = ?1",
            [&t.key_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .db()?;
    if spent > issued {
        tracing::error!(key_id = %hex::encode(&t.key_id), issued, spent, "more tokens spent than issued: tokens are being forged");
    }
    Ok(())
}

/// Drop keys past their validity, and the spent-token records under them.
pub fn collect_garbage(conn: &Connection) -> rusqlite::Result<usize> {
    let now = sql_i64(now_ms());
    let mut n = conn.execute("DELETE FROM spent_tokens WHERE key_id IN (SELECT key_id FROM token_keys WHERE valid_until <= ?1)", [now])?;
    n += conn.execute("DELETE FROM token_keys WHERE valid_until <= ?1", [now])?;
    n += conn.execute("DELETE FROM token_issuance WHERE day < ?1", [day(now_ms()) - 2])?;
    Ok(n)
}
