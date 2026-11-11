//! The local database: one SQLCipher file per profile (design section 11).
//!
//! OpenMLS's tables live in the same file, so a unit of work (process a
//! message, record it, advance the cursor) commits or rolls back as one.

use std::collections::HashMap;
use std::path::Path;

use postern_identity::{AccountId, Chain, DeviceId, LinkHash};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{DbKey, Error, Result};

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS meta (
    k  TEXT PRIMARY KEY,
    v  BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS chain_links (
    account_id  BLOB NOT NULL,
    seq         INTEGER NOT NULL,
    link        BLOB NOT NULL,
    PRIMARY KEY (account_id, seq)
);
-- The directory as this device has verified it. A username, once seen, stays
-- pinned to its account (design section 5).
CREATE TABLE IF NOT EXISTS accounts (
    account_id  BLOB PRIMARY KEY,
    username    TEXT NOT NULL UNIQUE,
    disabled    INTEGER NOT NULL DEFAULT 0,
    first_seen  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS conversations (
    group_id       BLOB PRIMARY KEY,
    kind           INTEGER NOT NULL,
    peer_account   BLOB,
    name           TEXT NOT NULL DEFAULT '',
    created_at     INTEGER NOT NULL,
    last_activity  INTEGER NOT NULL,
    cursor         INTEGER NOT NULL DEFAULT 0,
    -- Shown in the conversation list. False for a retired duplicate DM.
    active         INTEGER NOT NULL DEFAULT 1,
    -- Still fetched from the server. False once this device is removed.
    following      INTEGER NOT NULL DEFAULT 1
);
-- One live DM per peer. A duplicate created concurrently is retired, and its
-- messages are shown in the live one (design section 8, Delivery).
CREATE UNIQUE INDEX IF NOT EXISTS one_live_dm_per_peer ON conversations(peer_account) WHERE kind = 1 AND active = 1;
CREATE TABLE IF NOT EXISTS messages (
    pk              INTEGER PRIMARY KEY,
    group_id        BLOB NOT NULL,
    id              BLOB NOT NULL,
    sender_account  BLOB NOT NULL,
    sender_device   BLOB NOT NULL,
    outgoing        INTEGER NOT NULL,
    sent_at         INTEGER NOT NULL,
    received_at     INTEGER NOT NULL,
    seq             INTEGER,
    state           INTEGER NOT NULL,
    unsupported     INTEGER NOT NULL DEFAULT 0,
    deleted_at      INTEGER,
    UNIQUE (group_id, id)
);
CREATE TABLE IF NOT EXISTS message_versions (
    pk          INTEGER PRIMARY KEY,
    group_id    BLOB NOT NULL,
    message_id  BLOB NOT NULL,
    edit_id     BLOB NOT NULL,
    seq         INTEGER,
    body        TEXT NOT NULL,
    at          INTEGER NOT NULL,
    UNIQUE (group_id, edit_id)
);
CREATE INDEX IF NOT EXISTS versions_by_message ON message_versions(group_id, message_id);
-- Every client_msg_id this device has sent. The server's log names no
-- sender, so this is how the device recognises its own entries.
CREATE TABLE IF NOT EXISTS own_sends (
    group_id       BLOB NOT NULL,
    client_msg_id  BLOB NOT NULL,
    PRIMARY KEY (group_id, client_msg_id)
);
CREATE TABLE IF NOT EXISTS outbox (
    pk             INTEGER PRIMARY KEY,
    client_msg_id  BLOB NOT NULL UNIQUE,
    group_id       BLOB NOT NULL,
    content        BLOB NOT NULL,
    ciphertext     BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS pending_groups (
    group_id  BLOB PRIMARY KEY,
    request   BLOB NOT NULL
);
-- Anonymous tokens (design section 8). Each is deleted before it is spent,
-- so no token is ever presented twice.
CREATE TABLE IF NOT EXISTS tokens (
    pk           INTEGER PRIMARY KEY,
    token        BLOB NOT NULL,
    valid_until  INTEGER NOT NULL
);
-- Each account's latest profile, own included (design section 7).
CREATE TABLE IF NOT EXISTS profiles (
    account_id       BLOB PRIMARY KEY,
    version          INTEGER NOT NULL,
    display_name     TEXT NOT NULL,
    typing_template  TEXT NOT NULL,
    note_text        TEXT NOT NULL DEFAULT '',
    note_set_at      INTEGER NOT NULL DEFAULT 0,
    note_expires_at  INTEGER NOT NULL DEFAULT 0
);
-- Display names people set for one conversation.
CREATE TABLE IF NOT EXISTS conversation_names (
    group_id    BLOB NOT NULL,
    account_id  BLOB NOT NULL,
    name        TEXT NOT NULL,
    PRIMARY KEY (group_id, account_id)
);
-- Names this user gives others, visible only to them. Synced between their
-- own devices through the self group from phase 3.
CREATE TABLE IF NOT EXISTS nicknames (
    account_id  BLOB PRIMARY KEY,
    name        TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS notices (
    pk        INTEGER PRIMARY KEY,
    group_id  BLOB,
    at        INTEGER NOT NULL,
    text      TEXT NOT NULL
);
";

/// Message delivery state, as stored.
pub const STATE_PENDING: i64 = 0;
/// Accepted by the server.
pub const STATE_SENT: i64 = 1;
/// Received from someone else.
pub const STATE_RECEIVED: i64 = 2;
/// The server refused it for good.
pub const STATE_FAILED: i64 = 3;

/// Conversation kinds, as stored.
pub const KIND_DM: i64 = 1;
/// A named group.
pub const KIND_GROUP: i64 = 2;

/// Open a profile database with its key. Fails with [`Error::WrongKey`] if
/// the key does not match.
pub fn open(path: &Path, key: &DbKey) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "key", format!("x'{}'", hex::encode(key.bytes())))?;
    // SQLCipher sets default logging (WARN, to stderr) when a key is first
    // applied, so silence it after keying and before the first read. Postern
    // reports a wrong key itself.
    conn.pragma_update(None, "cipher_log_level", "NONE")?;
    // The first read fails if the key is wrong.
    if conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).is_err() {
        return Err(Error::WrongKey);
    }
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

/// Create tables, both ours and OpenMLS's.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    crate::provider::migrate(conn).map_err(Error::Storage)?;
    Ok(())
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn i64_of(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

pub fn account_id(b: &[u8]) -> Result<AccountId> {
    Ok(LinkHash(b.try_into().map_err(|_| Error::Corrupt("account id"))?))
}

// ---------------------------------------------------------------- meta

pub fn meta_get(conn: &Connection, k: &str) -> Result<Option<Vec<u8>>> {
    Ok(conn.query_row("SELECT v FROM meta WHERE k = ?1", [k], |r| r.get(0)).optional()?)
}

pub fn meta_put(conn: &Connection, k: &str, v: &[u8]) -> Result<()> {
    conn.execute("INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT (k) DO UPDATE SET v = excluded.v", params![k, v])?;
    Ok(())
}

pub fn meta_str(conn: &Connection, k: &str) -> Result<Option<String>> {
    meta_get(conn, k)?.map(|v| String::from_utf8(v).map_err(|_| Error::Corrupt("meta value"))).transpose()
}

pub fn meta_u64(conn: &Connection, k: &str) -> Result<u64> {
    Ok(meta_get(conn, k)?.and_then(|v| <[u8; 8]>::try_from(v.as_slice()).ok()).map(u64::from_be_bytes).unwrap_or(0))
}

pub fn meta_put_u64(conn: &Connection, k: &str, v: u64) -> Result<()> {
    meta_put(conn, k, &v.to_be_bytes())
}

// ---------------------------------------------------------------- directory

/// Every cached chain, re-verified.
pub fn load_chains(conn: &Connection) -> Result<HashMap<AccountId, Chain>> {
    let mut stmt = conn.prepare("SELECT account_id, link FROM chain_links ORDER BY account_id, seq")?;
    let mut links: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    for row in stmt.query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)))? {
        let (a, l) = row?;
        links.entry(a).or_default().push(l);
    }
    let mut out = HashMap::new();
    for (_, ls) in links {
        let chain = Chain::from_links(ls.iter().map(Vec::as_slice)).map_err(|_| Error::Corrupt("stored chain"))?;
        out.insert(chain.account_id(), chain);
    }
    Ok(out)
}

/// Store any links of `chain` not yet stored.
pub fn save_chain(conn: &Connection, chain: &Chain) -> Result<()> {
    let id = chain.account_id();
    for (seq, link) in chain.links().iter().enumerate() {
        conn.execute(
            "INSERT OR IGNORE INTO chain_links (account_id, seq, link) VALUES (?1, ?2, ?3)",
            params![&id.0[..], seq as i64, link],
        )?;
    }
    Ok(())
}

/// Record an account from the directory. A username already pinned to a
/// different account is refused.
pub fn upsert_account(conn: &Connection, account: &AccountId, username: &str, disabled: bool) -> Result<()> {
    let pinned: Option<Vec<u8>> =
        conn.query_row("SELECT account_id FROM accounts WHERE username = ?1", [username], |r| r.get(0)).optional()?;
    if pinned.is_some_and(|a| a[..] != account.0) {
        return Err(Error::IdentityChanged(username.into()));
    }
    conn.execute(
        "INSERT INTO accounts (account_id, username, disabled, first_seen) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (account_id) DO UPDATE SET disabled = excluded.disabled",
        params![&account.0[..], username, disabled, i64_of(now_ms())],
    )?;
    Ok(())
}

pub fn account_by_username(conn: &Connection, username: &str) -> Result<Option<(AccountId, bool)>> {
    let row: Option<(Vec<u8>, bool)> = conn
        .query_row("SELECT account_id, disabled FROM accounts WHERE username = ?1", [username], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    row.map(|(a, d)| Ok((account_id(&a)?, d))).transpose()
}

pub fn username_of(conn: &Connection, account: &[u8]) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT username FROM accounts WHERE account_id = ?1", [account], |r| r.get(0)).optional()?)
}

// ---------------------------------------------------------------- conversations

pub struct ConversationRow {
    pub group_id: [u8; 16],
    pub kind: i64,
    pub peer_account: Option<Vec<u8>>,
    pub name: String,
    pub last_activity: i64,
    pub cursor: u64,
    pub active: bool,
    pub following: bool,
}

const CONV_COLS: &str = "group_id, kind, peer_account, name, last_activity, cursor, active, following";

fn conv_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationRow> {
    let g: Vec<u8> = r.get(0)?;
    Ok(ConversationRow {
        group_id: g.try_into().map_err(|_| rusqlite::Error::InvalidColumnType(0, "group_id".into(), rusqlite::types::Type::Blob))?,
        kind: r.get(1)?,
        peer_account: r.get(2)?,
        name: r.get(3)?,
        last_activity: r.get(4)?,
        cursor: r.get::<_, i64>(5)? as u64,
        active: r.get(6)?,
        following: r.get(7)?,
    })
}

pub fn insert_conversation(
    conn: &Connection,
    group: &[u8; 16],
    kind: i64,
    peer: Option<&AccountId>,
    name: &str,
    active: bool,
) -> Result<()> {
    let now = i64_of(now_ms());
    conn.execute(
        "INSERT OR IGNORE INTO conversations (group_id, kind, peer_account, name, created_at, last_activity, active)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
        params![&group[..], kind, peer.map(|p| p.0.to_vec()), name, now, active],
    )?;
    Ok(())
}

pub fn conversation(conn: &Connection, group: &[u8]) -> Result<Option<ConversationRow>> {
    Ok(conn.query_row(&format!("SELECT {CONV_COLS} FROM conversations WHERE group_id = ?1"), [group], conv_row).optional()?)
}

/// The live DM with `peer`, if any.
pub fn dm_with(conn: &Connection, peer: &AccountId) -> Result<Option<ConversationRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {CONV_COLS} FROM conversations WHERE kind = 1 AND active = 1 AND peer_account = ?1"),
            [&peer.0[..]],
            conv_row,
        )
        .optional()?)
}

/// Live conversations, most recently active first.
pub fn conversations(conn: &Connection) -> Result<Vec<ConversationRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {CONV_COLS} FROM conversations WHERE active = 1 ORDER BY last_activity DESC"))?;
    Ok(stmt.query_map([], conv_row)?.collect::<rusqlite::Result<_>>()?)
}

/// Groups this device still fetches: every conversation it has not been
/// removed from, including retired duplicate DMs, so nothing sent into one
/// before both sides noticed the duplicate is missed.
pub fn followed_groups(conn: &Connection) -> Result<Vec<ConversationRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {CONV_COLS} FROM conversations WHERE following = 1"))?;
    Ok(stmt.query_map([], conv_row)?.collect::<rusqlite::Result<_>>()?)
}

pub fn stop_following(conn: &Connection, group: &[u8]) -> Result<()> {
    conn.execute("UPDATE conversations SET following = 0 WHERE group_id = ?1", [group])?;
    Ok(())
}

pub fn set_cursor(conn: &Connection, group: &[u8], seq: u64) -> Result<()> {
    conn.execute("UPDATE conversations SET cursor = MAX(cursor, ?1) WHERE group_id = ?2", params![i64_of(seq), group])?;
    Ok(())
}

pub fn touch(conn: &Connection, group: &[u8]) -> Result<()> {
    conn.execute("UPDATE conversations SET last_activity = ?1 WHERE group_id = ?2", params![i64_of(now_ms()), group])?;
    Ok(())
}

pub fn set_inactive(conn: &Connection, group: &[u8]) -> Result<()> {
    conn.execute("UPDATE conversations SET active = 0 WHERE group_id = ?1", [group])?;
    Ok(())
}

pub fn set_name(conn: &Connection, group: &[u8], name: &str) -> Result<()> {
    conn.execute("UPDATE conversations SET name = ?1 WHERE group_id = ?2", params![name, group])?;
    Ok(())
}

/// Every group whose messages belong in conversation `group`: itself, plus
/// for a DM any retired duplicate DM with the same peer.
pub fn conversation_groups(conn: &Connection, group: &[u8]) -> Result<Vec<Vec<u8>>> {
    let Some(c) = conversation(conn, group)? else { return Ok(Vec::new()) };
    if c.kind != KIND_DM {
        return Ok(vec![group.to_vec()]);
    }
    let mut stmt = conn.prepare("SELECT group_id FROM conversations WHERE kind = 1 AND peer_account = ?1")?;
    Ok(stmt.query_map([&c.peer_account], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------- messages

pub struct NewMessage<'a> {
    pub group: &'a [u8],
    pub id: &'a [u8],
    pub sender_account: &'a AccountId,
    pub sender_device: &'a DeviceId,
    pub outgoing: bool,
    pub sent_at: u64,
    pub seq: Option<u64>,
    pub state: i64,
    /// `None` for a message type this client does not understand.
    pub body: Option<&'a str>,
}

/// Insert a message and its first version. Returns false if a message with
/// this id already exists in the group (the first one wins).
pub fn insert_message(conn: &Connection, m: &NewMessage<'_>) -> Result<bool> {
    let now = i64_of(now_ms());
    let n = conn.execute(
        "INSERT OR IGNORE INTO messages (group_id, id, sender_account, sender_device, outgoing, sent_at, received_at, seq, state, unsupported)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            m.group,
            m.id,
            &m.sender_account.0[..],
            &m.sender_device.0[..],
            m.outgoing,
            i64_of(m.sent_at),
            now,
            m.seq.map(i64_of),
            m.state,
            m.body.is_none()
        ],
    )?;
    if n == 0 {
        return Ok(false);
    }
    if let Some(body) = m.body {
        add_version(conn, m.group, m.id, m.id, m.seq, body, m.sent_at)?;
    }
    Ok(true)
}

pub fn add_version(conn: &Connection, group: &[u8], message: &[u8], edit_id: &[u8], seq: Option<u64>, body: &str, at: u64) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO message_versions (group_id, message_id, edit_id, seq, body, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![group, message, edit_id, seq.map(i64_of), body, i64_of(at)],
    )?;
    Ok(n > 0)
}

/// Who sent a message, and whether it is deleted.
pub struct MessageMeta {
    pub sender_account: AccountId,
    pub deleted: bool,
}

pub fn message_meta(conn: &Connection, group: &[u8], id: &[u8]) -> Result<Option<MessageMeta>> {
    let row: Option<(Vec<u8>, Option<i64>)> = conn
        .query_row("SELECT sender_account, deleted_at FROM messages WHERE group_id = ?1 AND id = ?2", params![group, id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    row.map(|(a, d)| Ok(MessageMeta { sender_account: account_id(&a)?, deleted: d.is_some() })).transpose()
}

/// Delete a message's content: every version goes, a marker stays.
pub fn delete_message(conn: &Connection, group: &[u8], id: &[u8]) -> Result<()> {
    conn.execute(
        "UPDATE messages SET deleted_at = ?1 WHERE group_id = ?2 AND id = ?3 AND deleted_at IS NULL",
        params![i64_of(now_ms()), group, id],
    )?;
    conn.execute("DELETE FROM message_versions WHERE group_id = ?1 AND message_id = ?2", params![group, id])?;
    Ok(())
}

/// Record the server's sequence number for one of our own sends: on the
/// message itself if it was a text, and on the version it created.
pub fn mark_sent(conn: &Connection, group: &[u8], client_msg_id: &[u8], seq: u64) -> Result<()> {
    conn.execute(
        "UPDATE messages SET seq = ?1, state = ?2 WHERE group_id = ?3 AND id = ?4 AND state = ?5",
        params![i64_of(seq), STATE_SENT, group, client_msg_id, STATE_PENDING],
    )?;
    conn.execute("UPDATE message_versions SET seq = ?1 WHERE group_id = ?2 AND edit_id = ?3", params![i64_of(seq), group, client_msg_id])?;
    Ok(())
}

pub fn mark_failed(conn: &Connection, group: &[u8], client_msg_id: &[u8]) -> Result<()> {
    conn.execute(
        "UPDATE messages SET state = ?1 WHERE group_id = ?2 AND id = ?3 AND state = ?4",
        params![STATE_FAILED, group, client_msg_id, STATE_PENDING],
    )?;
    Ok(())
}

pub fn record_own_send(conn: &Connection, group: &[u8], client_msg_id: &[u8]) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO own_sends (group_id, client_msg_id) VALUES (?1, ?2)", params![group, client_msg_id])?;
    Ok(())
}

pub fn is_own_send(conn: &Connection, group: &[u8], client_msg_id: &[u8]) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM own_sends WHERE group_id = ?1 AND client_msg_id = ?2)",
        params![group, client_msg_id],
        |r| r.get(0),
    )?)
}

pub struct MessageRow {
    pub group_id: Vec<u8>,
    pub id: Vec<u8>,
    pub sender_account: Vec<u8>,
    pub outgoing: bool,
    pub sent_at: i64,
    pub state: i64,
    pub unsupported: bool,
    pub deleted: bool,
    pub body: Option<String>,
    pub versions: u32,
}

const MSG_SELECT: &str = "
SELECT m.group_id, m.id, m.sender_account, m.outgoing, m.sent_at, m.state, m.unsupported, m.deleted_at IS NOT NULL,
       (SELECT body FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id
         ORDER BY v.seq IS NULL DESC, v.seq DESC, v.pk DESC LIMIT 1),
       (SELECT count(*) FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id)
FROM messages m";

fn msg_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        group_id: r.get(0)?,
        id: r.get(1)?,
        sender_account: r.get(2)?,
        outgoing: r.get(3)?,
        sent_at: r.get(4)?,
        state: r.get(5)?,
        unsupported: r.get(6)?,
        deleted: r.get(7)?,
        body: r.get(8)?,
        versions: r.get(9)?,
    })
}

pub fn message(conn: &Connection, group: &[u8], id: &[u8]) -> Result<Option<MessageRow>> {
    Ok(conn.query_row(&format!("{MSG_SELECT} WHERE m.group_id = ?1 AND m.id = ?2"), params![group, id], msg_row).optional()?)
}

/// The newest `limit` messages across `groups`, oldest first. Within a group,
/// ordered by the server's log; messages not yet accepted come last.
pub fn messages(conn: &Connection, groups: &[Vec<u8>], limit: u32) -> Result<Vec<MessageRow>> {
    let mut rows: Vec<(i64, MessageRow)> = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "{MSG_SELECT} WHERE m.group_id = ?1 ORDER BY m.seq IS NULL DESC, m.seq DESC, m.pk DESC LIMIT ?2"
    ))?;
    for g in groups {
        for r in stmt.query_map(params![g, limit], msg_row)? {
            let r = r?;
            rows.push((r.sent_at, r));
        }
    }
    // Several groups only for a DM with a retired duplicate; merge by send time.
    if groups.len() > 1 {
        rows.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
        rows.truncate(limit as usize);
    }
    let mut out: Vec<MessageRow> = rows.into_iter().map(|(_, r)| r).collect();
    out.reverse();
    Ok(out)
}

pub struct VersionRow {
    pub body: String,
    pub at: i64,
    pub pending: bool,
}

/// Every version of a message, oldest first, in log order.
pub fn versions(conn: &Connection, group: &[u8], id: &[u8]) -> Result<Vec<VersionRow>> {
    let mut stmt = conn.prepare(
        "SELECT body, at, seq IS NULL FROM message_versions WHERE group_id = ?1 AND message_id = ?2 ORDER BY seq IS NULL, seq, pk",
    )?;
    Ok(stmt
        .query_map(params![group, id], |r| Ok(VersionRow { body: r.get(0)?, at: r.get(1)?, pending: r.get(2)? }))?
        .collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------- outbox, pending groups, tokens

pub struct OutboxRow {
    pub client_msg_id: Vec<u8>,
    pub group_id: Vec<u8>,
    pub content: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

pub fn outbox_push(conn: &Connection, row: &OutboxRow) -> Result<()> {
    conn.execute(
        "INSERT INTO outbox (client_msg_id, group_id, content, ciphertext) VALUES (?1, ?2, ?3, ?4)",
        params![row.client_msg_id, row.group_id, row.content, row.ciphertext],
    )?;
    Ok(())
}

pub fn outbox_first(conn: &Connection) -> Result<Option<OutboxRow>> {
    Ok(conn
        .query_row("SELECT client_msg_id, group_id, content, ciphertext FROM outbox ORDER BY pk LIMIT 1", [], |r| {
            Ok(OutboxRow { client_msg_id: r.get(0)?, group_id: r.get(1)?, content: r.get(2)?, ciphertext: r.get(3)? })
        })
        .optional()?)
}

pub fn outbox_replace_ciphertext(conn: &Connection, client_msg_id: &[u8], ciphertext: &[u8]) -> Result<()> {
    conn.execute("UPDATE outbox SET ciphertext = ?1 WHERE client_msg_id = ?2", params![ciphertext, client_msg_id])?;
    Ok(())
}

pub fn outbox_remove(conn: &Connection, client_msg_id: &[u8]) -> Result<()> {
    conn.execute("DELETE FROM outbox WHERE client_msg_id = ?1", [client_msg_id])?;
    Ok(())
}

pub fn pending_group_put(conn: &Connection, group: &[u8], request: &[u8]) -> Result<()> {
    conn.execute("INSERT OR REPLACE INTO pending_groups (group_id, request) VALUES (?1, ?2)", params![group, request])?;
    Ok(())
}

pub fn pending_groups(conn: &Connection) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut stmt = conn.prepare("SELECT group_id, request FROM pending_groups")?;
    Ok(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
}

pub fn pending_group_remove(conn: &Connection, group: &[u8]) -> Result<()> {
    conn.execute("DELETE FROM pending_groups WHERE group_id = ?1", [group])?;
    Ok(())
}

pub fn tokens_available(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("SELECT count(*) FROM tokens WHERE valid_until > ?1", [i64_of(now_ms() + 3_600_000)], |r| r.get(0))?)
}

pub fn store_token(conn: &Connection, token: &[u8], valid_until: u64) -> Result<()> {
    conn.execute("INSERT INTO tokens (token, valid_until) VALUES (?1, ?2)", params![token, i64_of(valid_until)])?;
    Ok(())
}

/// Remove and return a token valid for at least another hour, chosen at
/// random so spending order does not mirror issuing order.
pub fn take_token(conn: &Connection) -> Result<Option<Vec<u8>>> {
    conn.execute("DELETE FROM tokens WHERE valid_until <= ?1", [i64_of(now_ms() + 3_600_000)])?;
    let row: Option<(i64, Vec<u8>)> =
        conn.query_row("SELECT pk, token FROM tokens ORDER BY random() LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    match row {
        Some((pk, token)) => {
            conn.execute("DELETE FROM tokens WHERE pk = ?1", [pk])?;
            Ok(Some(token))
        }
        None => Ok(None),
    }
}

// ---------------------------------------------------------------- profiles and names

pub struct ProfileRow {
    pub version: u64,
    pub display_name: String,
    pub typing_template: String,
    pub note_text: String,
    pub note_set_at: u64,
    pub note_expires_at: u64,
}

/// Store a profile if it is newer than the one held. Returns whether it was.
pub fn upsert_profile(conn: &Connection, account: &[u8], p: &ProfileRow) -> Result<bool> {
    let n = conn.execute(
        "INSERT INTO profiles (account_id, version, display_name, typing_template, note_text, note_set_at, note_expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (account_id) DO UPDATE SET
           version = excluded.version, display_name = excluded.display_name, typing_template = excluded.typing_template,
           note_text = excluded.note_text, note_set_at = excluded.note_set_at, note_expires_at = excluded.note_expires_at
         WHERE excluded.version > profiles.version",
        params![
            account,
            i64_of(p.version),
            p.display_name,
            p.typing_template,
            p.note_text,
            i64_of(p.note_set_at),
            i64_of(p.note_expires_at)
        ],
    )?;
    Ok(n > 0)
}

pub fn profile(conn: &Connection, account: &[u8]) -> Result<Option<ProfileRow>> {
    Ok(conn
        .query_row(
            "SELECT version, display_name, typing_template, note_text, note_set_at, note_expires_at FROM profiles WHERE account_id = ?1",
            [account],
            |r| {
                Ok(ProfileRow {
                    version: r.get::<_, i64>(0)? as u64,
                    display_name: r.get(1)?,
                    typing_template: r.get(2)?,
                    note_text: r.get(3)?,
                    note_set_at: r.get::<_, i64>(4)? as u64,
                    note_expires_at: r.get::<_, i64>(5)? as u64,
                })
            },
        )
        .optional()?)
}

/// Set or clear (empty name) someone's display name for one conversation.
pub fn set_conversation_name(conn: &Connection, group: &[u8], account: &[u8], name: &str) -> Result<()> {
    if name.is_empty() {
        conn.execute("DELETE FROM conversation_names WHERE group_id = ?1 AND account_id = ?2", params![group, account])?;
    } else {
        conn.execute(
            "INSERT INTO conversation_names (group_id, account_id, name) VALUES (?1, ?2, ?3)
             ON CONFLICT (group_id, account_id) DO UPDATE SET name = excluded.name",
            params![group, account, name],
        )?;
    }
    Ok(())
}

pub fn conversation_name(conn: &Connection, group: &[u8], account: &[u8]) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT name FROM conversation_names WHERE group_id = ?1 AND account_id = ?2", params![group, account], |r| r.get(0))
        .optional()?)
}

/// Set or clear (empty name) this user's private nickname for someone.
pub fn set_nickname(conn: &Connection, account: &[u8], name: &str) -> Result<()> {
    if name.is_empty() {
        conn.execute("DELETE FROM nicknames WHERE account_id = ?1", [account])?;
    } else {
        conn.execute(
            "INSERT INTO nicknames (account_id, name) VALUES (?1, ?2) ON CONFLICT (account_id) DO UPDATE SET name = excluded.name",
            params![account, name],
        )?;
    }
    Ok(())
}

pub fn nickname(conn: &Connection, account: &[u8]) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT name FROM nicknames WHERE account_id = ?1", [account], |r| r.get(0)).optional()?)
}

/// The name a viewer sees for `account` in conversation `group` (design
/// section 7, "Names"): the viewer's nickname, else the person's name for this
/// conversation, else their global display name, else their username.
pub fn shown_name(conn: &Connection, group: Option<&[u8]>, account: &[u8]) -> Result<String> {
    if let Some(n) = nickname(conn, account)? {
        return Ok(n);
    }
    if let Some(g) = group
        && let Some(n) = conversation_name(conn, g, account)?
    {
        return Ok(n);
    }
    if let Some(p) = profile(conn, account)?
        && !p.display_name.is_empty()
    {
        return Ok(p.display_name);
    }
    Ok(username_of(conn, account)?.unwrap_or_else(|| format!("unknown ({})", &hex::encode(account)[..8])))
}

pub fn add_notice(conn: &Connection, group: Option<&[u8]>, text: &str) -> Result<()> {
    conn.execute("INSERT INTO notices (group_id, at, text) VALUES (?1, ?2, ?3)", params![group, i64_of(now_ms()), text])?;
    Ok(())
}
