//! Server storage (design section 8): SQLite in WAL mode, encrypted at rest
//! with SQLCipher, one writer.
//!
//! **No table links a device or account to a group**, and the group log stores
//! no sender. Groups are known only by random ID, epoch and the hash of each
//! epoch's access key. See "Metadata protection" in the design.
//!
//! Every function here runs in one transaction and is synchronous; the
//! session layer calls them on a blocking thread. Errors are protocol errors,
//! so the reason a request failed reaches the client.

use std::path::Path;

use postern_identity::{AccountId, Chain, DeviceId, LinkHash};
use postern_mls::{ContentType, Header, validate_key_package};
use postern_proto::{self as proto, ErrorCode};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::tokens;

/// A request failure, sent to the client as a protocol error.
pub type Result<T> = std::result::Result<T, proto::Error>;

pub(crate) fn err(code: ErrorCode, msg: impl Into<String>) -> proto::Error {
    proto::Error::new(code, msg)
}

pub(crate) trait Internal<T> {
    fn db(self) -> Result<T>;
}
impl<T> Internal<T> for rusqlite::Result<T> {
    fn db(self) -> Result<T> {
        self.map_err(|e| {
            tracing::error!(error = %e, "database error");
            err(ErrorCode::Internal, "database error")
        })
    }
}

/// Unclaimed one-time KeyPackages a device may hold.
pub const MAX_KEY_PACKAGES: u32 = 100;
/// Default and largest number of entries one fetch returns.
pub const FETCH_LIMIT: u32 = 500;
/// Group log entries, old epochs and mailbox entries older than this are
/// deleted. The server cannot tell when everyone has read a group entry, so
/// retention is by time.
pub const RETENTION_MS: u64 = 30 * 24 * 3600 * 1000;
/// Most devices one Welcome may be addressed to.
pub const MAX_WELCOME_TO: usize = 300;

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS meta (
    k  TEXT PRIMARY KEY,
    v  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS accounts (
    account_id   BLOB PRIMARY KEY,
    username     TEXT NOT NULL UNIQUE,
    is_bot       INTEGER NOT NULL,
    created_day  INTEGER NOT NULL,
    disabled_at  INTEGER,
    dir_version  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS accounts_by_version ON accounts(dir_version);
CREATE TABLE IF NOT EXISTS chain_links (
    account_id  BLOB NOT NULL REFERENCES accounts(account_id),
    seq         INTEGER NOT NULL,
    link        BLOB NOT NULL,
    PRIMARY KEY (account_id, seq)
);
CREATE TABLE IF NOT EXISTS devices (
    device_id      BLOB PRIMARY KEY,
    account_id     BLOB NOT NULL REFERENCES accounts(account_id),
    mls_sig_pub    BLOB NOT NULL,
    active         INTEGER NOT NULL,
    last_seen_day  INTEGER
);
CREATE INDEX IF NOT EXISTS devices_by_account ON devices(account_id);
CREATE TABLE IF NOT EXISTS key_packages (
    id           INTEGER PRIMARY KEY,
    device_id    BLOB NOT NULL REFERENCES devices(device_id),
    kp           BLOB NOT NULL,
    last_resort  INTEGER NOT NULL,
    created_day  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS key_packages_by_device ON key_packages(device_id, last_resort, id);
CREATE UNIQUE INDEX IF NOT EXISTS one_last_resort ON key_packages(device_id) WHERE last_resort = 1;
CREATE TABLE IF NOT EXISTS mailbox (
    device_id   BLOB NOT NULL,
    seq         INTEGER NOT NULL,
    welcome     BLOB NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (device_id, seq)
);
CREATE TABLE IF NOT EXISTS mailbox_state (
    device_id  BLOB PRIMARY KEY,
    next_seq   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS groups (
    group_id     BLOB PRIMARY KEY,
    epoch        INTEGER NOT NULL,
    next_seq     INTEGER NOT NULL,
    created_day  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS group_epochs (
    group_id     BLOB NOT NULL REFERENCES groups(group_id),
    epoch        INTEGER NOT NULL,
    access_hash  BLOB NOT NULL,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (group_id, epoch)
);
CREATE TABLE IF NOT EXISTS group_log (
    group_id       BLOB NOT NULL REFERENCES groups(group_id),
    seq            INTEGER NOT NULL,
    epoch          INTEGER NOT NULL,
    kind           INTEGER NOT NULL,
    client_msg_id  BLOB NOT NULL,
    payload        BLOB NOT NULL,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (group_id, seq)
);
CREATE UNIQUE INDEX IF NOT EXISTS log_idempotency ON group_log(group_id, client_msg_id);
CREATE INDEX IF NOT EXISTS log_by_epoch ON group_log(group_id, epoch, seq);
CREATE TABLE IF NOT EXISTS token_keys (
    key_id       BLOB PRIMARY KEY,
    secret_der   BLOB NOT NULL,
    public_der   BLOB NOT NULL,
    created_at   INTEGER NOT NULL,
    valid_until  INTEGER NOT NULL,
    issued       INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS spent_tokens (
    key_id        BLOB NOT NULL,
    message_hash  BLOB NOT NULL,
    PRIMARY KEY (key_id, message_hash)
);
CREATE TABLE IF NOT EXISTS token_issuance (
    device_id  BLOB NOT NULL,
    day        INTEGER NOT NULL,
    count      INTEGER NOT NULL,
    PRIMARY KEY (device_id, day)
);
CREATE TABLE IF NOT EXISTS invites (
    code_hash   BLOB PRIMARY KEY,
    bot         INTEGER NOT NULL,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    used_by     BLOB,
    used_at     INTEGER
);
";

/// Open (creating if needed) the server database, encrypted with `key`.
/// Fails if the key does not match an existing database.
pub fn open(path: &Path, key: &[u8; 32]) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "key", format!("x'{}'", hex::encode(key)))?;
    // SQLCipher sets default logging (WARN, to stderr) when a key is first
    // applied; silence it after keying and before the first read.
    conn.pragma_update(None, "cipher_log_level", "NONE")?;
    // Fails here, not later, if the key is wrong.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Current Unix time in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Today as days since the Unix epoch. Dates, not times, are stored where a
/// day is precise enough, as Signal does.
pub fn today() -> i64 {
    sql_i64(now_ms() / (24 * 3600 * 1000))
}

pub(crate) fn sql_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn write_tx(conn: &mut Connection) -> Result<Transaction<'_>> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate).db()
}

fn account_id(b: &[u8]) -> Result<AccountId> {
    let a: [u8; 32] = b.try_into().map_err(|_| err(ErrorCode::BadRequest, "account id must be 32 bytes"))?;
    Ok(LinkHash(a))
}

fn device_id(b: &[u8]) -> Result<DeviceId> {
    DeviceId::from_slice(b).map_err(|_| err(ErrorCode::BadRequest, "device id must be 16 bytes"))
}

fn sha256(b: &[u8]) -> Vec<u8> {
    Sha256::digest(b).to_vec()
}

/// Bump and return the directory version.
fn bump_directory(tx: &Connection) -> Result<i64> {
    tx.execute("INSERT INTO meta (k, v) VALUES ('dir_version', 1) ON CONFLICT (k) DO UPDATE SET v = v + 1", []).db()?;
    tx.query_row("SELECT v FROM meta WHERE k = 'dir_version'", [], |r| r.get(0)).db()
}

// ---------------------------------------------------------------- invites

/// Invite codes are stored hashed, so a database leak reveals no usable code.
fn invite_hash(code: &str) -> Vec<u8> {
    sha256(code.trim().as_bytes())
}

/// Create an invite. Returns the code to hand out.
pub fn create_invite(conn: &mut Connection, bot: bool, valid_ms: u64) -> rusqlite::Result<String> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let code = hex::encode(raw);
    let now = now_ms();
    conn.execute(
        "INSERT INTO invites (code_hash, bot, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![invite_hash(&code), bot, sql_i64(now), sql_i64(now + valid_ms)],
    )?;
    Ok(code)
}

// ---------------------------------------------------------------- accounts and chains

/// The verified chain of an account, from its stored links.
pub fn load_chain(conn: &Connection, account: &AccountId) -> Result<Chain> {
    let links = chain_links(conn, &account.0)?;
    if links.is_empty() {
        return Err(err(ErrorCode::NotFound, "no such account"));
    }
    // Verified when stored; verifying again guards against a tampered database.
    Chain::from_links(links.iter().map(Vec::as_slice)).map_err(|e| {
        tracing::error!(error = %e, "stored chain failed verification");
        err(ErrorCode::Internal, "stored chain is invalid")
    })
}

fn chain_links(conn: &Connection, account: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut stmt = conn.prepare_cached("SELECT link FROM chain_links WHERE account_id = ?1 ORDER BY seq").db()?;
    stmt.query_map([account], |r| r.get(0)).db()?.collect::<rusqlite::Result<_>>().db()
}

/// Register an account from its genesis link, consuming an invite.
/// `verify_auth` checks the login signature against the genesis device.
pub fn register(
    conn: &mut Connection,
    server_domain: &str,
    invite_code: &str,
    genesis: &[u8],
    verify_auth: impl FnOnce(&Chain) -> bool,
) -> Result<(AccountId, DeviceId)> {
    let chain = Chain::from_genesis(genesis).map_err(|e| err(ErrorCode::ChainRejected, e.to_string()))?;
    if chain.server_domain() != server_domain {
        return Err(err(ErrorCode::ChainRejected, "genesis names a different server"));
    }
    if !verify_auth(&chain) {
        return Err(err(ErrorCode::Unauthenticated, "bad login signature"));
    }
    let device = &chain.devices()[0];
    let now = sql_i64(now_ms());
    let tx = write_tx(conn)?;
    let invite: Option<(bool, i64, Option<Vec<u8>>)> = tx
        .query_row("SELECT bot, expires_at, used_by FROM invites WHERE code_hash = ?1", [invite_hash(invite_code)], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
        .db()?;
    match invite {
        Some((bot, expires, None)) if expires > now => {
            if bot != chain.is_bot() {
                return Err(err(ErrorCode::InviteInvalid, "invite and account disagree on bot status"));
            }
        }
        _ => return Err(err(ErrorCode::InviteInvalid, "invite is unknown, used or expired")),
    }
    let taken: bool =
        tx.query_row("SELECT EXISTS(SELECT 1 FROM accounts WHERE username = ?1)", [chain.username()], |r| r.get(0)).db()?;
    if taken {
        return Err(err(ErrorCode::UsernameTaken, "username is taken"));
    }
    let account = chain.account_id();
    let version = bump_directory(&tx)?;
    tx.execute(
        "INSERT INTO accounts (account_id, username, is_bot, created_day, dir_version) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![&account.0[..], chain.username(), chain.is_bot(), today(), version],
    )
    .db()?;
    tx.execute("INSERT INTO chain_links (account_id, seq, link) VALUES (?1, 0, ?2)", params![&account.0[..], genesis]).db()?;
    tx.execute(
        "INSERT INTO devices (device_id, account_id, mls_sig_pub, active, last_seen_day) VALUES (?1, ?2, ?3, 1, ?4)",
        params![&device.id.0[..], &account.0[..], &device.entry.mls_sig_pub[..], today()],
    )
    .db()?;
    tx.execute("UPDATE invites SET used_by = ?1, used_at = ?2 WHERE code_hash = ?3", params![&account.0[..], now, invite_hash(invite_code)])
        .db()?;
    tx.commit().db()?;
    Ok((account, device.id))
}

/// Look up an active device for login. Returns its account and chain, and
/// records today as the device's last-seen day.
pub fn device_for_login(conn: &mut Connection, device: &DeviceId) -> Result<(AccountId, Chain)> {
    let row: Option<(Vec<u8>, bool, Option<i64>)> = conn
        .query_row(
            "SELECT d.account_id, d.active, a.disabled_at FROM devices d JOIN accounts a USING (account_id) WHERE d.device_id = ?1",
            [&device.0[..]],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .db()?;
    let (account, active, disabled) = row.ok_or_else(|| err(ErrorCode::Unauthenticated, "unknown device"))?;
    if !active || disabled.is_some() {
        return Err(err(ErrorCode::Disabled, "device or account is disabled"));
    }
    let account = account_id(&account)?;
    let chain = load_chain(conn, &account)?;
    conn.execute("UPDATE devices SET last_seen_day = ?1 WHERE device_id = ?2", params![today(), &device.0[..]]).db()?;
    Ok((account, chain))
}

/// Accounts changed since `since`, with their full chains.
pub fn directory(conn: &Connection, since: u64) -> Result<proto::DirectoryUpdate> {
    let version: i64 = conn.query_row("SELECT COALESCE((SELECT v FROM meta WHERE k = 'dir_version'), 0)", [], |r| r.get(0)).db()?;
    let mut stmt = conn
        .prepare_cached("SELECT account_id, username, disabled_at IS NOT NULL FROM accounts WHERE dir_version > ?1 ORDER BY dir_version")
        .db()?;
    let rows: Vec<(Vec<u8>, String, bool)> =
        stmt.query_map([sql_i64(since)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).db()?.collect::<rusqlite::Result<_>>().db()?;
    let mut accounts = Vec::with_capacity(rows.len());
    for (account_id, username, disabled) in rows {
        let chain = chain_links(conn, &account_id)?;
        accounts.push(proto::DirectoryAccount { account_id, username, chain, disabled });
    }
    Ok(proto::DirectoryUpdate { version: version as u64, accounts })
}

/// What appending a chain link changed, so sessions can be cut off.
pub struct ChainUpdate {
    /// Devices the link removed.
    pub removed: Vec<DeviceId>,
}

/// Verify and append a link to the caller's own chain, and apply its effect.
pub fn append_chain(conn: &mut Connection, account: &AccountId, link: &[u8]) -> Result<ChainUpdate> {
    let tx = write_tx(conn)?;
    let mut chain = load_chain(&tx, account)?;
    let before: Vec<_> = chain.devices().to_vec();
    chain.append(link).map_err(|e| err(ErrorCode::ChainRejected, e.to_string()))?;
    let seq = chain.next_seq() - 1;
    tx.execute("INSERT INTO chain_links (account_id, seq, link) VALUES (?1, ?2, ?3)", params![&account.0[..], sql_i64(seq), link])
        .db()?;
    let mut removed = Vec::new();
    for d in chain.devices() {
        match before.iter().find(|b| b.id == d.id) {
            None => {
                tx.execute(
                    "INSERT INTO devices (device_id, account_id, mls_sig_pub, active) VALUES (?1, ?2, ?3, 1)",
                    params![&d.id.0[..], &account.0[..], &d.entry.mls_sig_pub[..]],
                )
                .db()?;
            }
            Some(b) if b.is_active() && !d.is_active() => {
                for sql in [
                    "UPDATE devices SET active = 0 WHERE device_id = ?1",
                    "DELETE FROM key_packages WHERE device_id = ?1",
                    "DELETE FROM mailbox WHERE device_id = ?1",
                ] {
                    tx.execute(sql, [&d.id.0[..]]).db()?;
                }
                removed.push(d.id);
            }
            Some(b) if b.entry.mls_sig_pub != d.entry.mls_sig_pub => {
                // Old KeyPackages carry the old key and would fail clients' checks.
                tx.execute("UPDATE devices SET mls_sig_pub = ?1 WHERE device_id = ?2", params![&d.entry.mls_sig_pub[..], &d.id.0[..]])
                    .db()?;
                tx.execute("DELETE FROM key_packages WHERE device_id = ?1", [&d.id.0[..]]).db()?;
            }
            Some(_) => {}
        }
    }
    let version = bump_directory(&tx)?;
    tx.execute("UPDATE accounts SET dir_version = ?1 WHERE account_id = ?2", params![version, &account.0[..]]).db()?;
    tx.commit().db()?;
    Ok(ChainUpdate { removed })
}

// ---------------------------------------------------------------- key packages

/// One-time KeyPackages remaining, and whether a last-resort one exists.
pub fn key_package_counts(conn: &Connection, device: &DeviceId) -> Result<(u32, bool)> {
    conn.query_row(
        "SELECT COALESCE(SUM(last_resort = 0), 0), COALESCE(SUM(last_resort = 1), 0) FROM key_packages WHERE device_id = ?1",
        [&device.0[..]],
        |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)? > 0)),
    )
    .db()
}

/// Validate and store KeyPackages for the caller's own device.
pub fn upload_key_packages(
    conn: &mut Connection,
    account: &AccountId,
    device: &DeviceId,
    one_time: &[Vec<u8>],
    last_resort: Option<&[u8]>,
) -> Result<u32> {
    let crypto = postern_mls::crypto();
    let tx = write_tx(conn)?;
    let mls_key: Vec<u8> = tx
        .query_row("SELECT mls_sig_pub FROM devices WHERE device_id = ?1 AND active = 1", [&device.0[..]], |r| r.get(0))
        .db()?;
    let check = |bytes: &[u8], want_last_resort: bool| -> Result<()> {
        let v = validate_key_package(bytes, &crypto).map_err(|e| err(ErrorCode::KeyPackageInvalid, e.to_string()))?;
        if v.leaf.account != *account || v.leaf.device != *device || v.leaf.signature_key != mls_key {
            return Err(err(ErrorCode::KeyPackageInvalid, "KeyPackage is not for this device and key"));
        }
        if v.last_resort != want_last_resort {
            return Err(err(ErrorCode::KeyPackageInvalid, "last-resort flag does not match its slot"));
        }
        Ok(())
    };
    let (have, _) = key_package_counts(&tx, device)?;
    if have.saturating_add(one_time.len() as u32) > MAX_KEY_PACKAGES {
        return Err(err(ErrorCode::BadRequest, format!("at most {MAX_KEY_PACKAGES} unclaimed KeyPackages")));
    }
    for kp in one_time {
        check(kp, false)?;
        tx.execute(
            "INSERT INTO key_packages (device_id, kp, last_resort, created_day) VALUES (?1, ?2, 0, ?3)",
            params![&device.0[..], kp, today()],
        )
        .db()?;
    }
    if let Some(kp) = last_resort {
        check(kp, true)?;
        tx.execute("DELETE FROM key_packages WHERE device_id = ?1 AND last_resort = 1", [&device.0[..]]).db()?;
        tx.execute(
            "INSERT INTO key_packages (device_id, kp, last_resort, created_day) VALUES (?1, ?2, 1, ?3)",
            params![&device.0[..], kp, today()],
        )
        .db()?;
    }
    let (remaining, _) = key_package_counts(&tx, device)?;
    tx.commit().db()?;
    Ok(remaining)
}

/// Spend a token, then claim KeyPackages.
///
/// For a whole account: one per active device, the oldest one-time package,
/// else the last-resort one (not consumed). Devices with neither are left out.
///
/// For one device (`device_id` set): up to `count` one-time packages, oldest
/// first, and the last-resort one too if there were fewer than `count`. The
/// caller may use the last-resort package for every group it lacks one for.
pub fn claim_key_packages(conn: &mut Connection, req: &proto::ClaimKeyPackages) -> Result<Vec<proto::ClaimedKeyPackage>> {
    if !req.device_id.is_empty() {
        return claim_for_device(conn, req);
    }
    let tx = write_tx(conn)?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE account_id = ?1 AND disabled_at IS NULL)",
            [&req.account_id],
            |r| r.get(0),
        )
        .db()?;
    if !exists {
        return Err(err(ErrorCode::NotFound, "no such account"));
    }
    tokens::spend(&tx, req.token.as_ref())?;
    let devices: Vec<Vec<u8>> = {
        let mut stmt = tx.prepare_cached("SELECT device_id FROM devices WHERE account_id = ?1 AND active = 1").db()?;
        stmt.query_map([&req.account_id], |r| r.get(0)).db()?.collect::<rusqlite::Result<_>>().db()?
    };
    let mut out = Vec::new();
    for d in devices {
        let row: Option<(i64, Vec<u8>, bool)> = tx
            .query_row("SELECT id, kp, last_resort FROM key_packages WHERE device_id = ?1 ORDER BY last_resort, id LIMIT 1", [&d], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .db()?;
        if let Some((id, kp, last_resort)) = row {
            if !last_resort {
                tx.execute("DELETE FROM key_packages WHERE id = ?1", [id]).db()?;
            }
            out.push(proto::ClaimedKeyPackage { device_id: d, key_package: kp });
        }
    }
    tx.commit().db()?;
    Ok(out)
}

fn claim_for_device(conn: &mut Connection, req: &proto::ClaimKeyPackages) -> Result<Vec<proto::ClaimedKeyPackage>> {
    let device = device_id(&req.device_id)?;
    let count = req.count.clamp(1, proto::MAX_CLAIM);
    let tx = write_tx(conn)?;
    let active: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM devices d JOIN accounts a USING (account_id)
              WHERE d.device_id = ?1 AND d.account_id = ?2 AND d.active = 1 AND a.disabled_at IS NULL)",
            params![&device.0[..], &req.account_id],
            |r| r.get(0),
        )
        .db()?;
    if !active {
        return Err(err(ErrorCode::NotFound, "no such device"));
    }
    tokens::spend(&tx, req.token.as_ref())?;
    let rows: Vec<(i64, Vec<u8>)> = {
        let mut stmt =
            tx.prepare_cached("SELECT id, kp FROM key_packages WHERE device_id = ?1 AND last_resort = 0 ORDER BY id LIMIT ?2").db()?;
        stmt.query_map(params![&device.0[..], count], |r| Ok((r.get(0)?, r.get(1)?))).db()?.collect::<rusqlite::Result<_>>().db()?
    };
    let mut out = Vec::with_capacity(rows.len() + 1);
    for (id, kp) in rows {
        tx.execute("DELETE FROM key_packages WHERE id = ?1", [id]).db()?;
        out.push(proto::ClaimedKeyPackage { device_id: device.0.to_vec(), key_package: kp });
    }
    if out.len() < count as usize {
        let last: Option<Vec<u8>> = tx
            .query_row("SELECT kp FROM key_packages WHERE device_id = ?1 AND last_resort = 1", [&device.0[..]], |r| r.get(0))
            .optional()
            .db()?;
        if let Some(kp) = last {
            out.push(proto::ClaimedKeyPackage { device_id: device.0.to_vec(), key_package: kp });
        }
    }
    tx.commit().db()?;
    Ok(out)
}

// ---------------------------------------------------------------- mailboxes

fn queue_welcome(tx: &Connection, to: &[u8], welcome: &[u8]) -> Result<()> {
    tx.execute("INSERT OR IGNORE INTO mailbox_state (device_id, next_seq) VALUES (?1, 1)", [to]).db()?;
    let seq: i64 = tx.query_row("SELECT next_seq FROM mailbox_state WHERE device_id = ?1", [to], |r| r.get(0)).db()?;
    tx.execute(
        "INSERT INTO mailbox (device_id, seq, welcome, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![to, seq, welcome, sql_i64(now_ms())],
    )
    .db()?;
    tx.execute("UPDATE mailbox_state SET next_seq = next_seq + 1 WHERE device_id = ?1", [to]).db()?;
    Ok(())
}

/// Check a Welcome and its recipients, and queue it in their mailboxes.
fn deliver_welcome(tx: &Connection, welcome: &[u8], welcome_to: &[Vec<u8>]) -> Result<Vec<DeviceId>> {
    if welcome_to.is_empty() {
        return if welcome.is_empty() { Ok(Vec::new()) } else { Err(err(ErrorCode::BadRequest, "Welcome without recipients")) };
    }
    if welcome_to.len() > MAX_WELCOME_TO {
        return Err(err(ErrorCode::BadRequest, "too many Welcome recipients"));
    }
    postern_mls::check_welcome(welcome).map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
    let mut out = Vec::with_capacity(welcome_to.len());
    for d in welcome_to {
        let id = device_id(d)?;
        let active: bool = tx
            .query_row("SELECT EXISTS(SELECT 1 FROM devices WHERE device_id = ?1 AND active = 1)", [d], |r| r.get(0))
            .db()?;
        if !active {
            return Err(err(ErrorCode::NotFound, "Welcome recipient unknown or inactive"));
        }
        if !out.contains(&id) {
            queue_welcome(tx, d, welcome)?;
            out.push(id);
        }
    }
    Ok(out)
}

/// The device's mailbox entries, oldest first.
pub fn fetch_mailbox(conn: &Connection, device: &DeviceId, limit: u32) -> Result<proto::Mailbox> {
    let limit = if limit == 0 { FETCH_LIMIT } else { limit.min(FETCH_LIMIT) };
    let mut stmt = conn.prepare_cached("SELECT seq, welcome FROM mailbox WHERE device_id = ?1 ORDER BY seq LIMIT ?2").db()?;
    let mut entries: Vec<proto::MailboxEntry> = stmt
        .query_map(params![&device.0[..], limit + 1], |r| Ok(proto::MailboxEntry { seq: r.get::<_, i64>(0)? as u64, welcome: r.get(1)? }))
        .db()?
        .collect::<rusqlite::Result<_>>()
        .db()?;
    let more = entries.len() > limit as usize;
    entries.truncate(limit as usize);
    Ok(proto::Mailbox { entries, more })
}

/// Delete mailbox entries up to `seq`.
pub fn ack_mailbox(conn: &mut Connection, device: &DeviceId, seq: u64) -> Result<()> {
    conn.execute("DELETE FROM mailbox WHERE device_id = ?1 AND seq <= ?2", params![&device.0[..], sql_i64(seq)]).db()?;
    Ok(())
}

// ---------------------------------------------------------------- groups

fn check_len(b: &[u8], n: usize, what: &str) -> Result<()> {
    if b.len() == n { Ok(()) } else { Err(err(ErrorCode::BadRequest, format!("{what} must be {n} bytes"))) }
}

/// Spend a token and register a group its creator built at epoch 1. Returns
/// the devices whose mailboxes got the Welcome.
pub fn create_group(conn: &mut Connection, req: &proto::CreateGroup) -> Result<Vec<DeviceId>> {
    check_len(&req.group_id, 16, "group id")?;
    check_len(&req.access_hash, 32, "access hash")?;
    let tx = write_tx(conn)?;
    let exists: bool =
        tx.query_row("SELECT EXISTS(SELECT 1 FROM groups WHERE group_id = ?1)", [&req.group_id], |r| r.get(0)).db()?;
    if exists {
        // Checked before spending, so a creator retrying after a lost reply
        // keeps its token. Group IDs are random, so this is that retry.
        return Err(err(ErrorCode::GroupExists, "group already exists"));
    }
    tokens::spend(&tx, req.token.as_ref())?;
    let now = sql_i64(now_ms());
    tx.execute("INSERT INTO groups (group_id, epoch, next_seq, created_day) VALUES (?1, 1, 1, ?2)", params![&req.group_id, today()])
        .db()?;
    tx.execute(
        "INSERT INTO group_epochs (group_id, epoch, access_hash, created_at) VALUES (?1, 1, ?2, ?3)",
        params![&req.group_id, &req.access_hash, now],
    )
    .db()?;
    let welcomed = deliver_welcome(&tx, &req.welcome, &req.welcome_to)?;
    tx.commit().db()?;
    Ok(welcomed)
}

/// Check an access key against the stored hash for a group's epoch.
fn check_access(tx: &Connection, group: &[u8], epoch: u64, access_key: &[u8]) -> Result<()> {
    let hash: Option<Vec<u8>> = tx
        .query_row("SELECT access_hash FROM group_epochs WHERE group_id = ?1 AND epoch = ?2", params![group, sql_i64(epoch)], |r| r.get(0))
        .optional()
        .db()?;
    match hash {
        Some(h) if h == sha256(access_key) => Ok(()),
        // The same answer for an unknown group and a wrong key, so a guess
        // does not reveal whether a group ID exists.
        _ => Err(err(ErrorCode::BadAccessKey, "no such group, or wrong access key")),
    }
}

fn current_epoch(tx: &Connection, group: &[u8]) -> Result<(u64, u64)> {
    tx.query_row("SELECT epoch, next_seq FROM groups WHERE group_id = ?1", [group], |r| {
        Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
    })
    .optional()
    .db()?
    .ok_or_else(|| err(ErrorCode::BadAccessKey, "no such group, or wrong access key"))
}

fn check_header(header: &Header, group: &[u8], epoch: u64, want: ContentType) -> Result<()> {
    if header.group_id != group {
        return Err(err(ErrorCode::BadRequest, "message is for a different group"));
    }
    if header.content_type != want {
        return Err(err(ErrorCode::BadRequest, "wrong MLS content type for this request"));
    }
    if header.epoch != epoch {
        let mut e = err(ErrorCode::WrongEpoch, "message is not in the group's current epoch");
        e.current_epoch = epoch;
        return Err(e);
    }
    Ok(())
}

fn existing_entry(tx: &Connection, group: &[u8], client_msg_id: &[u8]) -> Result<Option<(u64, u64, i32)>> {
    tx.query_row(
        "SELECT seq, epoch, kind FROM group_log WHERE group_id = ?1 AND client_msg_id = ?2",
        params![group, client_msg_id],
        |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get(2)?)),
    )
    .optional()
    .db()
}

fn append_log(tx: &Connection, group: &[u8], seq: u64, epoch: u64, kind: proto::EntryKind, client_msg_id: &[u8], payload: &[u8]) -> Result<()> {
    tx.execute(
        "INSERT INTO group_log (group_id, seq, epoch, kind, client_msg_id, payload, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![group, sql_i64(seq), sql_i64(epoch), kind as i32, client_msg_id, payload, sql_i64(now_ms())],
    )
    .db()?;
    tx.execute("UPDATE groups SET next_seq = next_seq + 1 WHERE group_id = ?1", [group]).db()?;
    Ok(())
}

/// Append an application message. Returns (seq, epoch, newly appended).
pub fn send(conn: &mut Connection, req: &proto::Send) -> Result<(u64, u64, bool)> {
    check_len(&req.client_msg_id, 16, "client_msg_id")?;
    let header = postern_mls::parse_header(&req.message).map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
    let tx = write_tx(conn)?;
    // Idempotent retry: answered before the access check, since a retry may
    // come after a commit has changed the key. The id is a 16-byte secret.
    if let Some((seq, epoch, kind)) = existing_entry(&tx, &req.group_id, &req.client_msg_id)? {
        if kind != proto::EntryKind::Application as i32 {
            return Err(err(ErrorCode::BadRequest, "client_msg_id already used for a commit"));
        }
        return Ok((seq, epoch, false));
    }
    let (epoch, next_seq) = current_epoch(&tx, &req.group_id)?;
    check_access(&tx, &req.group_id, epoch, &req.access_key)?;
    check_header(&header, &req.group_id, epoch, ContentType::Application)?;
    append_log(&tx, &req.group_id, next_seq, epoch, proto::EntryKind::Application, &req.client_msg_id, &req.message)?;
    tx.commit().db()?;
    Ok((next_seq, epoch, true))
}

/// Append a commit and advance the epoch. Returns (seq, new epoch, devices
/// welcomed, newly appended).
pub fn commit(conn: &mut Connection, req: &proto::Commit) -> Result<(u64, u64, Vec<DeviceId>, bool)> {
    check_len(&req.client_msg_id, 16, "client_msg_id")?;
    check_len(&req.next_access_hash, 32, "next access hash")?;
    let header = postern_mls::parse_header(&req.commit).map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
    let tx = write_tx(conn)?;
    if let Some((seq, epoch, kind)) = existing_entry(&tx, &req.group_id, &req.client_msg_id)? {
        if kind != proto::EntryKind::Commit as i32 {
            return Err(err(ErrorCode::BadRequest, "client_msg_id already used for a message"));
        }
        return Ok((seq, epoch + 1, Vec::new(), false));
    }
    let (epoch, next_seq) = current_epoch(&tx, &req.group_id)?;
    check_access(&tx, &req.group_id, epoch, &req.access_key)?;
    check_header(&header, &req.group_id, epoch, ContentType::Commit)?;
    append_log(&tx, &req.group_id, next_seq, epoch, proto::EntryKind::Commit, &req.client_msg_id, &req.commit)?;
    let new_epoch = epoch + 1;
    tx.execute("UPDATE groups SET epoch = ?1 WHERE group_id = ?2", params![sql_i64(new_epoch), &req.group_id]).db()?;
    tx.execute(
        "INSERT INTO group_epochs (group_id, epoch, access_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![&req.group_id, sql_i64(new_epoch), &req.next_access_hash, sql_i64(now_ms())],
    )
    .db()?;
    let welcomed = deliver_welcome(&tx, &req.welcome, &req.welcome_to)?;
    tx.commit().db()?;
    Ok((next_seq, new_epoch, welcomed, true))
}

/// Entries for each cursor: those of the cursor's epoch after its position,
/// authorized by that epoch's key. A bad key fails only its own group.
pub fn fetch_groups(conn: &Connection, req: &proto::FetchGroups) -> Result<proto::GroupBatch> {
    let limit = if req.limit == 0 { FETCH_LIMIT } else { req.limit.min(FETCH_LIMIT) } as usize;
    let mut batch = proto::GroupBatch::default();
    let mut budget = limit;
    let mut stmt = conn
        .prepare_cached(
            "SELECT seq, epoch, kind, client_msg_id, payload, created_at FROM group_log
             WHERE group_id = ?1 AND epoch = ?2 AND seq > ?3 ORDER BY seq LIMIT ?4",
        )
        .db()?;
    for c in &req.groups {
        let mut ge = proto::GroupEntries { group_id: c.group_id.clone(), entries: Vec::new(), error: None };
        if let Err(e) = check_access(conn, &c.group_id, c.epoch, &c.access_key) {
            ge.error = Some(e);
            batch.groups.push(ge);
            continue;
        }
        if budget == 0 {
            batch.more = true;
            break;
        }
        let rows = stmt
            .query_map(params![&c.group_id, sql_i64(c.epoch), sql_i64(c.after_seq), (budget + 1) as i64], |r| {
                Ok(proto::LogEntry {
                    seq: r.get::<_, i64>(0)? as u64,
                    epoch: r.get::<_, i64>(1)? as u64,
                    kind: r.get(2)?,
                    client_msg_id: r.get(3)?,
                    payload: r.get(4)?,
                    created_at_ms: r.get::<_, i64>(5)? as u64,
                })
            })
            .db()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .db()?;
        if rows.len() > budget {
            batch.more = true;
        }
        ge.entries = rows.into_iter().take(budget).collect();
        budget -= ge.entries.len();
        batch.groups.push(ge);
    }
    Ok(batch)
}

/// Check an access key against a group's current epoch.
pub fn check_current_access(conn: &mut Connection, group: &[u8], access_key: &[u8]) -> Result<()> {
    let (epoch, _) = current_epoch(conn, group)?;
    check_access(conn, group, epoch, access_key)
}

/// The groups among `cursors` whose access key is valid for its epoch.
pub fn valid_subscriptions(conn: &Connection, cursors: &[proto::GroupCursor]) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for c in cursors {
        if check_access(conn, &c.group_id, c.epoch, &c.access_key).is_ok() {
            out.push(c.group_id.clone());
        }
    }
    Ok(out)
}

/// Delete what is past retention. Returns the number of rows deleted.
pub fn collect_garbage(conn: &mut Connection) -> rusqlite::Result<usize> {
    let cutoff = sql_i64(now_ms().saturating_sub(RETENTION_MS));
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut n = tx.execute("DELETE FROM group_log WHERE created_at < ?1", [cutoff])?;
    // Keep every group's current epoch, so an idle group stays usable.
    n += tx.execute(
        "DELETE FROM group_epochs WHERE created_at < ?1
            AND epoch < (SELECT epoch FROM groups g WHERE g.group_id = group_epochs.group_id)",
        [cutoff],
    )?;
    n += tx.execute("DELETE FROM mailbox WHERE created_at < ?1", [cutoff])?;
    n += tx.execute("DELETE FROM invites WHERE used_by IS NULL AND expires_at < ?1", [sql_i64(now_ms())])?;
    n += tokens::collect_garbage(&tx)?;
    tx.commit()?;
    Ok(n)
}
