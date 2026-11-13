//! The local database: one SQLCipher file per profile (design section 11).
//!
//! OpenMLS's tables live in the same file, so a unit of work (process a
//! message, record it, advance the cursor) commits or rolls back as one.

use std::collections::HashMap;
use std::path::Path;

use post_identity::{AccountId, Chain, DeviceId, LinkHash};
use rusqlite::{Connection, OptionalExtension, params};

use crate::picture::PictureKind;
use crate::{DbKey, Error, Result, content};

/// Version 1 of the schema, plus tables added since. New tables may be added
/// here; changes to existing tables go in [`MIGRATIONS`].
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
-- One live self group. If two were ever created, the lower ID wins, as for DMs.
CREATE UNIQUE INDEX IF NOT EXISTS one_live_self_group ON conversations(kind) WHERE kind = 3 AND active = 1;
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
-- Proposals waiting to reach the server. Unlike messages they cannot be
-- re-encrypted for a later epoch: one that misses its epoch is dropped, and
-- the device proposes again in the new epoch if it still needs to.
CREATE TABLE IF NOT EXISTS outbox_proposals (
    client_msg_id  BLOB PRIMARY KEY,
    group_id       BLOB NOT NULL,
    epoch          INTEGER NOT NULL,
    request        BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS pending_groups (
    group_id  BLOB PRIMARY KEY,
    request   BLOB NOT NULL
);
-- A commit staged in OpenMLS and sent, or about to be sent, to the server.
-- It is merged when it shows up in the group's log, and dropped if another
-- commit took its epoch. At most one per group, as OpenMLS allows.
CREATE TABLE IF NOT EXISTS pending_commits (
    group_id       BLOB PRIMARY KEY,
    client_msg_id  BLOB NOT NULL,
    request        BLOB NOT NULL
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
-- own devices through the self group; migration 2 adds msg_id and seq.
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
CREATE TABLE IF NOT EXISTS announcements (
    id          INTEGER PRIMARY KEY,
    created_at  INTEGER NOT NULL,
    kind        INTEGER NOT NULL,
    text        TEXT NOT NULL,
    starts_at   INTEGER NOT NULL,
    minutes     INTEGER NOT NULL
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
/// This account's self group. Never listed as a conversation.
pub const KIND_SELF: i64 = 3;

/// Open a profile database with its key. Fails with [`Error::WrongKey`] if
/// the key does not match.
pub fn open(path: &Path, key: &DbKey) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "key", format!("x'{}'", hex::encode(key.bytes())))?;
    // SQLCipher sets default logging (WARN, to stderr) when a key is first
    // applied, so silence it after keying and before the first read. Post
    // reports a wrong key itself.
    conn.pragma_update(None, "cipher_log_level", "NONE")?;
    // The first read fails if the key is wrong.
    if conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).is_err() {
        return Err(Error::WrongKey);
    }
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // FULL (SQLite's default, stated so no build or later edit relaxes it):
    // a commit is on disk before it returns. Receiving acknowledges to the
    // server only after its commit, and sending transmits only what the
    // outbox holds, so a power cut can neither lose a message nor roll MLS
    // state back behind what the server has seen (design section 11).
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    register_functions(&conn)?;
    Ok(conn)
}

/// SQL functions the schema's triggers call, so every connection that
/// writes needs them.
///
/// `post_mentions(body)`: the usernames `body` mentions, as a JSON array
/// (usernames need no escaping); `[]` for none or NULL.
fn register_functions(conn: &Connection) -> Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function("post_mentions", 1, FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC, |ctx| {
        let body: Option<String> = ctx.get(0)?;
        let names = body.as_deref().map(crate::content::mentions).unwrap_or_default();
        Ok(format!("[{}]", names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(",")))
    })?;
    Ok(())
}

/// A trigger that refreshes who one message mentions, from its latest
/// text, as [`reindex_trigger`] refreshes the search index. Mentions are
/// kept for messages that are not deleted and are understood.
macro_rules! remention_trigger {
    ($name:literal, $when:literal, $group:literal, $id:literal) => {
        concat!(
            "CREATE TRIGGER ", $name, " ", $when, " BEGIN
                 DELETE FROM message_mentions WHERE pk = (SELECT pk FROM messages WHERE group_id = ", $group, " AND id = ", $id, ");
                 INSERT OR IGNORE INTO message_mentions (username, pk)
                     SELECT j.value, m.pk
                     FROM messages m, json_each(post_mentions(
                         (SELECT body FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id
                          ORDER BY v.seq IS NULL DESC, v.seq DESC, v.pk DESC LIMIT 1))) j
                     WHERE m.group_id = ", $group, " AND m.id = ", $id, " AND m.deleted_at IS NULL AND m.unsupported = 0;
             END;"
        )
    };
}

/// A trigger that puts one message back into the search index as it now
/// is: out, then in again with its latest text, unless it is deleted, not
/// understood, or has no text.
macro_rules! reindex_trigger {
    ($name:literal, $when:literal, $group:literal, $id:literal) => {
        concat!(
            "CREATE TRIGGER ", $name, " ", $when, " BEGIN
                 DELETE FROM message_search WHERE rowid = (SELECT pk FROM messages WHERE group_id = ", $group, " AND id = ", $id, ");
                 INSERT INTO message_search (rowid, body)
                     SELECT m.pk, (SELECT body FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id
                                   ORDER BY v.seq IS NULL DESC, v.seq DESC, v.pk DESC LIMIT 1)
                     FROM messages m
                     WHERE m.group_id = ", $group, " AND m.id = ", $id, " AND m.deleted_at IS NULL AND m.unsupported = 0
                       AND EXISTS (SELECT 1 FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id);
             END;"
        )
    };
}

/// Changes to existing tables, in order. `SCHEMA` creates version 1 (and any
/// tables added since, which `IF NOT EXISTS` handles); entry `i` here takes a
/// database from version `i + 1` to `i + 2`. Never edit an entry once
/// released: add a new one.
const MIGRATIONS: &[&str] = &[
    // 2: nicknames and device names sync through the self group, ordered by
    // the self group's log (see `synced_set_pending`).
    "ALTER TABLE nicknames ADD COLUMN msg_id BLOB;
     ALTER TABLE nicknames ADD COLUMN seq INTEGER DEFAULT 0;
     CREATE TABLE device_names (
         device_id  BLOB PRIMARY KEY,
         name       TEXT NOT NULL,
         msg_id     BLOB,
         seq        INTEGER
     );",
    // 3: leaving a group takes until another member commits the removal; the
    // conversation is shown as left from the moment the user leaves.
    "ALTER TABLE conversations ADD COLUMN leaving INTEGER NOT NULL DEFAULT 0;",
    // 4: when this device last replaced its own leaf in each group, for the
    // weekly key refresh (design section 6, "Key refresh").
    "ALTER TABLE conversations ADD COLUMN refreshed_at INTEGER NOT NULL DEFAULT 0;
     UPDATE conversations SET refreshed_at = created_at;",
    // 5: devices the directory lists as stale (unseen for 30 days).
    "CREATE TABLE stale_devices (
         device_id   BLOB PRIMARY KEY,
         account_id  BLOB NOT NULL
     );",
    // 6: how far the user has read each group, as a position in its server
    // log; later messages from others are unread. A log position means the
    // same on every device, and history fetched later sits below it. Every
    // message already here counts as read.
    "ALTER TABLE conversations ADD COLUMN read_seq INTEGER NOT NULL DEFAULT 0;
     UPDATE conversations SET read_seq =
         (SELECT COALESCE(MAX(seq), 0) FROM messages WHERE messages.group_id = conversations.group_id);",
    // 7: people whose safety number the user verified, synced between their
    // devices like nicknames. `name` is '1' for verified, '' for withdrawn.
    "CREATE TABLE verified (
         account_id  BLOB PRIMARY KEY,
         name        TEXT NOT NULL,
         msg_id      BLOB,
         seq         INTEGER
     );",
    // 8: how each person's messages arrive on screen (their profile's
    // arrival preset; empty for the app's default).
    "ALTER TABLE profiles ADD COLUMN arrival TEXT NOT NULL DEFAULT '';",
    // 9: a full-text index of messages, for search: each message's latest
    // text under its row's pk, for messages not deleted and understood. It
    // lives in this (encrypted) database like everything else. Words match
    // ignoring case and accents. Triggers keep it current through sends,
    // edits (including a pending edit being accepted, which can change
    // which version is latest) and deletes, whatever code makes them.
    concat!(
        "CREATE VIRTUAL TABLE message_search USING fts5(body, tokenize = 'unicode61 remove_diacritics 2');",
        reindex_trigger!("search_version_added", "AFTER INSERT ON message_versions", "NEW.group_id", "NEW.message_id"),
        reindex_trigger!("search_version_sent", "AFTER UPDATE OF seq ON message_versions", "NEW.group_id", "NEW.message_id"),
        reindex_trigger!("search_version_gone", "AFTER DELETE ON message_versions", "OLD.group_id", "OLD.message_id"),
        reindex_trigger!("search_message_added", "AFTER INSERT ON messages", "NEW.group_id", "NEW.id"),
        reindex_trigger!("search_message_changed", "AFTER UPDATE OF deleted_at, unsupported ON messages", "NEW.group_id", "NEW.id"),
        "CREATE TRIGGER search_message_gone AFTER DELETE ON messages BEGIN
             DELETE FROM message_search WHERE rowid = OLD.pk;
         END;
         INSERT INTO message_search (rowid, body)
             SELECT m.pk, (SELECT body FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id
                           ORDER BY v.seq IS NULL DESC, v.seq DESC, v.pk DESC LIMIT 1)
             FROM messages m
             WHERE m.deleted_at IS NULL AND m.unsupported = 0
               AND EXISTS (SELECT 1 FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id);"
    ),
    // 10: who each message mentions (`@username`, design section 7), from
    // its latest text, for unread mention counts and search. Kept current by
    // triggers like the search index's, with every message filled in now.
    concat!(
        "CREATE TABLE message_mentions (
             username  TEXT NOT NULL,
             pk        INTEGER NOT NULL,
             PRIMARY KEY (username, pk)
         ) WITHOUT ROWID;
         CREATE INDEX message_mentions_pk ON message_mentions (pk);",
        remention_trigger!("mentions_version_added", "AFTER INSERT ON message_versions", "NEW.group_id", "NEW.message_id"),
        remention_trigger!("mentions_version_sent", "AFTER UPDATE OF seq ON message_versions", "NEW.group_id", "NEW.message_id"),
        remention_trigger!("mentions_version_gone", "AFTER DELETE ON message_versions", "OLD.group_id", "OLD.message_id"),
        remention_trigger!("mentions_message_added", "AFTER INSERT ON messages", "NEW.group_id", "NEW.id"),
        remention_trigger!("mentions_message_changed", "AFTER UPDATE OF deleted_at, unsupported ON messages", "NEW.group_id", "NEW.id"),
        "CREATE TRIGGER mentions_message_gone AFTER DELETE ON messages BEGIN
             DELETE FROM message_mentions WHERE pk = OLD.pk;
         END;
         INSERT OR IGNORE INTO message_mentions (username, pk)
             SELECT j.value, m.pk
             FROM messages m, json_each(post_mentions(
                 (SELECT body FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id
                  ORDER BY v.seq IS NULL DESC, v.seq DESC, v.pk DESC LIMIT 1))) j
             WHERE m.deleted_at IS NULL AND m.unsupported = 0;"
    ),
    // 11: what a notice is about, for how it is shown: 'notice' (security
    // and other warnings, as every earlier one was), 'call', 'missed_call'.
    "ALTER TABLE notices ADD COLUMN kind TEXT NOT NULL DEFAULT 'notice';",
    // 12: receipts (design section 7, "Receipts"). How far each account has
    // received and read each group, from its receipts; the positions every
    // other member has reached, for the ticks on our own messages; and what
    // this account last said in its own receipts. Plus account-wide
    // settings synced like nicknames (whether to send read receipts).
    "CREATE TABLE receipts (
         group_id       BLOB NOT NULL,
         account_id     BLOB NOT NULL,
         delivered_seq  INTEGER NOT NULL DEFAULT 0,
         read_seq       INTEGER NOT NULL DEFAULT 0,
         PRIMARY KEY (group_id, account_id)
     );
     ALTER TABLE conversations ADD COLUMN delivered_floor INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE conversations ADD COLUMN read_floor INTEGER NOT NULL DEFAULT 0;
     CREATE TABLE synced_settings (
         key     BLOB PRIMARY KEY,
         name    TEXT NOT NULL,
         msg_id  BLOB,
         seq     INTEGER
     );",
    // 13: profile pictures and banners (design section 7). Each profile's
    // images as encoded `ProfileImage`s, and its banner colour; pictures
    // fetched and opened, by the hash of their ciphertext; ones that could
    // not be fetched, and when to try again; and this device's own uploads,
    // with the secrets that renew and delete them.
    "ALTER TABLE profiles ADD COLUMN avatar BLOB;
     ALTER TABLE profiles ADD COLUMN banner BLOB;
     ALTER TABLE profiles ADD COLUMN banner_color INTEGER NOT NULL DEFAULT 0;
     CREATE TABLE pictures (
         sha256      BLOB PRIMARY KEY,
         data        BLOB NOT NULL,
         expires_at  INTEGER NOT NULL
     );
     CREATE TABLE picture_misses (
         sha256    BLOB PRIMARY KEY,
         retry_at  INTEGER NOT NULL,
         tries     INTEGER NOT NULL
     );
     CREATE TABLE own_pictures (
         blob_id       BLOB PRIMARY KEY,
         sha256        BLOB NOT NULL,
         renew_secret  BLOB NOT NULL,
         expires_at    INTEGER NOT NULL
     );",
];

/// Create tables, both ours and OpenMLS's, and bring old databases up to date.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    register_functions(conn)?;
    conn.execute_batch(SCHEMA)?;
    // Version 0 is a database made before versioning: it has version 1's tables.
    let version = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?.max(1) as usize;
    if version > MIGRATIONS.len() + 1 {
        return Err(Error::Storage(format!("this profile was written by a newer Post (schema version {version}); update the app")));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version - 1) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as i64 + 2)?;
        tx.commit()?;
    }
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

pub fn meta_delete(conn: &Connection, k: &str) -> Result<()> {
    conn.execute("DELETE FROM meta WHERE k = ?1", [k])?;
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

/// Replace an account's stale devices with what the directory says now.
pub fn set_stale_devices(conn: &Connection, account: &AccountId, devices: &[DeviceId]) -> Result<()> {
    conn.execute("DELETE FROM stale_devices WHERE account_id = ?1", [&account.0[..]])?;
    for d in devices {
        conn.execute("INSERT OR REPLACE INTO stale_devices (device_id, account_id) VALUES (?1, ?2)", params![&d.0[..], &account.0[..]])?;
    }
    Ok(())
}

pub fn stale_devices(conn: &Connection) -> Result<std::collections::HashSet<DeviceId>> {
    let mut stmt = conn.prepare("SELECT device_id FROM stale_devices")?;
    let rows: Vec<Vec<u8>> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows.iter().filter_map(|d| DeviceId::from_slice(d).ok()).collect())
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
    /// The user left; the removal waits for another member to commit it.
    pub leaving: bool,
    /// When this device last replaced its own leaf here (Unix ms).
    pub refreshed_at: u64,
}

const CONV_COLS: &str = "group_id, kind, peer_account, name, last_activity, cursor, active, following, leaving, refreshed_at";

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
        leaving: r.get(8)?,
        refreshed_at: r.get::<_, i64>(9)? as u64,
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
    // Creating or joining gives this device a fresh leaf.
    conn.execute(
        "INSERT OR IGNORE INTO conversations (group_id, kind, peer_account, name, created_at, last_activity, active, refreshed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?5)",
        params![&group[..], kind, peer.map(|p| p.0.to_vec()), name, now, active],
    )?;
    Ok(())
}

/// Back in a group after being removed or leaving, with a fresh leaf.
pub fn rejoin(conn: &Connection, group: &[u8]) -> Result<()> {
    conn.execute(
        "UPDATE conversations SET following = 1, leaving = 0, refreshed_at = ?1, last_activity = ?1 WHERE group_id = ?2",
        params![i64_of(now_ms()), group],
    )?;
    Ok(())
}

pub fn set_refreshed(conn: &Connection, group: &[u8]) -> Result<()> {
    conn.execute("UPDATE conversations SET refreshed_at = ?1 WHERE group_id = ?2", params![i64_of(now_ms()), group])?;
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

/// Live conversations, most recently active first. The self group is not a
/// conversation and is left out.
pub fn conversations(conn: &Connection) -> Result<Vec<ConversationRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CONV_COLS} FROM conversations WHERE active = 1 AND kind != 3 ORDER BY last_activity DESC"
    ))?;
    Ok(stmt.query_map([], conv_row)?.collect::<rusqlite::Result<_>>()?)
}

/// Groups that should hear about account-wide changes such as a new profile:
/// every live conversation this device is still in, and the self group.
pub fn broadcast_groups(conn: &Connection) -> Result<Vec<[u8; 16]>> {
    let mut stmt = conn.prepare(&format!("SELECT {CONV_COLS} FROM conversations WHERE active = 1 AND following = 1 AND leaving = 0"))?;
    Ok(stmt.query_map([], conv_row)?.map(|r| r.map(|c| c.group_id)).collect::<rusqlite::Result<_>>()?)
}

/// The live self group, if this device is in one.
pub fn self_group(conn: &Connection) -> Result<Option<[u8; 16]>> {
    Ok(conn
        .query_row(&format!("SELECT {CONV_COLS} FROM conversations WHERE kind = 3 AND active = 1"), [], conv_row)
        .optional()?
        .map(|c| c.group_id))
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

pub fn set_leaving(conn: &Connection, group: &[u8]) -> Result<()> {
    conn.execute("UPDATE conversations SET leaving = 1 WHERE group_id = ?1", [group])?;
    Ok(())
}

/// Groups this device is leaving and still in.
pub fn leaving_groups(conn: &Connection) -> Result<Vec<[u8; 16]>> {
    let mut stmt = conn.prepare(&format!("SELECT {CONV_COLS} FROM conversations WHERE leaving = 1 AND following = 1"))?;
    Ok(stmt.query_map([], conv_row)?.map(|r| r.map(|c| c.group_id)).collect::<rusqlite::Result<_>>()?)
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
    synced_placed(conn, client_msg_id, Some(seq))
}

pub fn mark_failed(conn: &Connection, group: &[u8], client_msg_id: &[u8]) -> Result<()> {
    conn.execute(
        "UPDATE messages SET state = ?1 WHERE group_id = ?2 AND id = ?3 AND state = ?4",
        params![STATE_FAILED, group, client_msg_id, STATE_PENDING],
    )?;
    synced_placed(conn, client_msg_id, None)
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
    /// How far the other members' receipts reach it: 0, or
    /// [`RECEIPT_DELIVERED`] or [`RECEIPT_READ`].
    pub receipt: i64,
}

/// A message's columns as [`msg_row`] reads them, from `messages m`.
macro_rules! msg_columns {
    () => {
        "m.group_id, m.id, m.sender_account, m.outgoing, m.sent_at, m.state, m.unsupported, m.deleted_at IS NOT NULL,
       (SELECT body FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id
         ORDER BY v.seq IS NULL DESC, v.seq DESC, v.pk DESC LIMIT 1),
       (SELECT count(*) FROM message_versions v WHERE v.group_id = m.group_id AND v.message_id = m.id),
       COALESCE((SELECT CASE WHEN m.seq <= rc.read_floor THEN 2 WHEN m.seq <= rc.delivered_floor THEN 1 ELSE 0 END
                 FROM conversations rc WHERE rc.group_id = m.group_id), 0)"
    };
}

/// [`MessageRow::receipt`]: every other member has received it.
pub const RECEIPT_DELIVERED: i64 = 1;
/// [`MessageRow::receipt`]: every other member has read it.
pub const RECEIPT_READ: i64 = 2;

const MSG_SELECT: &str = concat!("SELECT ", msg_columns!(), " FROM messages m");

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
        receipt: r.get(10)?,
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

/// What to search for: every part is optional, and all given must match.
#[derive(Default)]
pub struct SearchFilter<'a> {
    /// Words, each matching a word that starts with it, ignoring case and
    /// accents.
    pub words: Vec<String>,
    pub sender: Option<&'a [u8]>,
    /// Only in these groups.
    pub groups: Option<&'a [Vec<u8>]>,
    /// Sent at or after, and before (Unix ms).
    pub after: Option<i64>,
    pub before: Option<i64>,
    /// Only messages with a link.
    pub has_link: bool,
    /// Only messages that mention this username.
    pub mentions: Option<&'a str>,
    pub oldest_first: bool,
}

/// The FTS5 query for `words`: each a quoted phrase (so nothing typed is
/// read as query syntax) matching as a prefix, all of them required.
pub fn fts_query(words: &[String]) -> Option<String> {
    let terms: Vec<String> = words.iter().filter(|w| !w.trim().is_empty()).map(|w| format!("\"{}\"*", w.replace('"', "\"\""))).collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

/// Messages matching `f` from the search index (which holds every message
/// that is not deleted and is understood, by its latest text), in
/// conversations the user can see: how many in all, and the page asked for.
pub fn search(conn: &Connection, f: &SearchFilter<'_>, limit: u32, offset: u32) -> Result<(u32, Vec<MessageRow>)> {
    let mut wheres = vec![format!("c.kind != {KIND_SELF}")];
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(q) = fts_query(&f.words) {
        wheres.push("message_search MATCH ?".into());
        args.push(Box::new(q));
    }
    if let Some(s) = f.sender {
        wheres.push("m.sender_account = ?".into());
        args.push(Box::new(s.to_vec()));
    }
    if let Some(u) = f.mentions {
        wheres.push("EXISTS (SELECT 1 FROM message_mentions mm WHERE mm.username = ? AND mm.pk = m.pk)".into());
        args.push(Box::new(u.to_owned()));
    }
    if let Some(groups) = f.groups {
        if groups.is_empty() {
            return Ok((0, Vec::new()));
        }
        wheres.push(format!("m.group_id IN ({})", vec!["?"; groups.len()].join(", ")));
        args.extend(groups.iter().map(|g| Box::new(g.clone()) as Box<dyn rusqlite::ToSql>));
    }
    if let Some(t) = f.after {
        wheres.push("m.sent_at >= ?".into());
        args.push(Box::new(t));
    }
    if let Some(t) = f.before {
        wheres.push("m.sent_at < ?".into());
        args.push(Box::new(t));
    }
    if f.has_link {
        wheres.push("(instr(lower(message_search.body), 'http://') > 0 OR instr(lower(message_search.body), 'https://') > 0)".into());
    }
    let from = format!(
        "FROM message_search JOIN messages m ON m.pk = message_search.rowid JOIN conversations c ON c.group_id = m.group_id WHERE {}",
        wheres.join(" AND ")
    );
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
    let total: u32 = conn.query_row(&format!("SELECT count(*) {from}"), refs.as_slice(), |r| r.get(0))?;
    let order = if f.oldest_first { "ASC" } else { "DESC" };
    let mut stmt = conn.prepare(&format!(
        "SELECT {} {from} ORDER BY m.sent_at {order}, m.pk {order} LIMIT {limit} OFFSET {offset}",
        msg_columns!()
    ))?;
    let rows = stmt.query_map(refs.as_slice(), msg_row)?.collect::<rusqlite::Result<_>>()?;
    Ok((total, rows))
}

/// Messages from others in `groups` past each group's read position, not
/// deleted.
pub fn unread(conn: &Connection, groups: &[Vec<u8>]) -> Result<u32> {
    let mut stmt = conn.prepare(
        "SELECT count(*) FROM messages m JOIN conversations c ON c.group_id = m.group_id
         WHERE m.group_id = ?1 AND m.outgoing = 0 AND m.deleted_at IS NULL AND m.seq > c.read_seq",
    )?;
    let mut n = 0u32;
    for g in groups {
        n = n.saturating_add(stmt.query_row([g], |r| r.get::<_, u32>(0))?);
    }
    Ok(n)
}

/// Unread messages (as [`unread`] counts them) in `groups` that mention
/// `username`.
pub fn unread_mentions(conn: &Connection, groups: &[Vec<u8>], username: &str) -> Result<u32> {
    let mut stmt = conn.prepare(
        "SELECT count(*) FROM message_mentions mm JOIN messages m ON m.pk = mm.pk JOIN conversations c ON c.group_id = m.group_id
         WHERE mm.username = ?2 AND m.group_id = ?1 AND m.outgoing = 0 AND m.deleted_at IS NULL AND m.seq > c.read_seq",
    )?;
    let mut n = 0u32;
    for g in groups {
        n = n.saturating_add(stmt.query_row(params![g, username], |r| r.get::<_, u32>(0))?);
    }
    Ok(n)
}

/// Mark everything in `groups` read, up to the newest message each has.
/// Returns the groups whose position moved, with the new position.
pub fn mark_read(conn: &Connection, groups: &[Vec<u8>]) -> Result<Vec<(Vec<u8>, u64)>> {
    let mut newest = conn.prepare("SELECT COALESCE(MAX(seq), 0) FROM messages WHERE group_id = ?1")?;
    let mut moved = Vec::new();
    for g in groups {
        let seq: i64 = newest.query_row([g], |r| r.get(0))?;
        let seq = u64::try_from(seq).unwrap_or(0);
        if raise_read(conn, g, seq)? {
            moved.push((g.clone(), seq));
        }
    }
    Ok(moved)
}

/// Move a group's read position forward to `seq`. Returns whether it moved.
pub fn raise_read(conn: &Connection, group: &[u8], seq: u64) -> Result<bool> {
    let n = conn.execute(
        "UPDATE conversations SET read_seq = ?2 WHERE group_id = ?1 AND read_seq < ?2",
        params![group, i64_of(seq)],
    )?;
    Ok(n > 0)
}

/// Every group's read position, for a self-group snapshot.
pub fn read_positions(conn: &Connection) -> Result<Vec<(Vec<u8>, u64)>> {
    let mut stmt = conn.prepare("SELECT group_id, read_seq FROM conversations WHERE kind != 3 AND read_seq > 0")?;
    Ok(stmt
        .query_map([], |r| Ok((r.get(0)?, u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0))))?
        .collect::<rusqlite::Result<_>>()?)
}

/// A notice, as stored.
pub struct NoticeRow {
    pub at: i64,
    pub text: String,
    /// 'notice', 'call' or 'missed_call' (migration 11).
    pub kind: String,
}

/// The newest `limit` notices about `groups`, or about the account as a whole
/// when `groups` is empty; oldest first.
pub fn notices(conn: &Connection, groups: &[Vec<u8>], limit: u32) -> Result<Vec<NoticeRow>> {
    let row = |r: &rusqlite::Row<'_>| Ok(NoticeRow { at: r.get(0)?, text: r.get(1)?, kind: r.get(2)? });
    let mut out: Vec<NoticeRow> = if groups.is_empty() {
        let mut stmt = conn.prepare("SELECT at, text, kind FROM notices WHERE group_id IS NULL ORDER BY pk DESC LIMIT ?1")?;
        stmt.query_map([limit], row)?.collect::<rusqlite::Result<_>>()?
    } else {
        let mut stmt = conn.prepare("SELECT at, text, kind FROM notices WHERE group_id = ?1 ORDER BY pk DESC LIMIT ?2")?;
        let mut all = Vec::new();
        for g in groups {
            all.extend(stmt.query_map(params![g, limit], row)?.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        all.sort_by_key(|n| std::cmp::Reverse(n.at));
        all.truncate(limit as usize);
        all
    };
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

pub fn proposal_push(conn: &Connection, client_msg_id: &[u8], group: &[u8], epoch: u64, request: &[u8]) -> Result<()> {
    conn.execute(
        "INSERT INTO outbox_proposals (client_msg_id, group_id, epoch, request) VALUES (?1, ?2, ?3, ?4)",
        params![client_msg_id, group, i64_of(epoch), request],
    )?;
    Ok(())
}

/// A proposal waiting to be sent.
pub struct WaitingProposal {
    pub client_msg_id: Vec<u8>,
    pub group_id: Vec<u8>,
    /// The only epoch it is valid in.
    pub epoch: u64,
    /// An encoded `Send` request.
    pub request: Vec<u8>,
}

pub fn proposals_waiting(conn: &Connection) -> Result<Vec<WaitingProposal>> {
    let mut stmt = conn.prepare("SELECT client_msg_id, group_id, epoch, request FROM outbox_proposals")?;
    Ok(stmt
        .query_map([], |r| {
            Ok(WaitingProposal { client_msg_id: r.get(0)?, group_id: r.get(1)?, epoch: r.get::<_, i64>(2)? as u64, request: r.get(3)? })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn proposal_remove(conn: &Connection, client_msg_id: &[u8]) -> Result<()> {
    conn.execute("DELETE FROM outbox_proposals WHERE client_msg_id = ?1", [client_msg_id])?;
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

pub fn pending_commit_put(conn: &Connection, group: &[u8], client_msg_id: &[u8], request: &[u8]) -> Result<()> {
    conn.execute(
        "INSERT INTO pending_commits (group_id, client_msg_id, request) VALUES (?1, ?2, ?3)",
        params![group, client_msg_id, request],
    )?;
    Ok(())
}

/// The pending commit for a group: (client_msg_id, request).
pub fn pending_commit(conn: &Connection, group: &[u8]) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
    Ok(conn
        .query_row("SELECT client_msg_id, request FROM pending_commits WHERE group_id = ?1", [group], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?)
}

pub fn pending_commits(conn: &Connection) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut stmt = conn.prepare("SELECT group_id, request FROM pending_commits")?;
    Ok(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
}

pub fn pending_commit_remove(conn: &Connection, group: &[u8]) -> Result<()> {
    conn.execute("DELETE FROM pending_commits WHERE group_id = ?1", [group])?;
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

#[derive(Clone, Default)]
pub struct ProfileRow {
    pub version: u64,
    pub display_name: String,
    pub typing_template: String,
    pub note_text: String,
    pub note_set_at: u64,
    pub note_expires_at: u64,
    pub arrival: String,
    pub avatar: Option<content::ProfileImage>,
    pub banner: Option<content::ProfileImage>,
    /// `0x01RRGGBB`, or 0 for none.
    pub banner_color: u32,
}

impl ProfileRow {
    /// Its picture of `kind`.
    pub fn image(&self, kind: PictureKind) -> Option<&content::ProfileImage> {
        match kind {
            PictureKind::Avatar => self.avatar.as_ref(),
            PictureKind::Banner => self.banner.as_ref(),
        }
    }

    pub fn image_mut(&mut self, kind: PictureKind) -> &mut Option<content::ProfileImage> {
        match kind {
            PictureKind::Avatar => &mut self.avatar,
            PictureKind::Banner => &mut self.banner,
        }
    }
}

fn encode_image(i: Option<&content::ProfileImage>) -> Option<Vec<u8>> {
    i.map(prost::Message::encode_to_vec)
}

fn decode_image(b: Option<Vec<u8>>) -> Option<content::ProfileImage> {
    b.and_then(|b| <content::ProfileImage as prost::Message>::decode(b.as_slice()).ok())
}

/// Store a profile if it is newer than the one held. Returns whether it was.
pub fn upsert_profile(conn: &Connection, account: &[u8], p: &ProfileRow) -> Result<bool> {
    let n = conn.execute(
        "INSERT INTO profiles (account_id, version, display_name, typing_template, note_text, note_set_at, note_expires_at, arrival,
                               avatar, banner, banner_color)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (account_id) DO UPDATE SET
           version = excluded.version, display_name = excluded.display_name, typing_template = excluded.typing_template,
           note_text = excluded.note_text, note_set_at = excluded.note_set_at, note_expires_at = excluded.note_expires_at,
           arrival = excluded.arrival, avatar = excluded.avatar, banner = excluded.banner, banner_color = excluded.banner_color
         WHERE excluded.version > profiles.version",
        params![
            account,
            i64_of(p.version),
            p.display_name,
            p.typing_template,
            p.note_text,
            i64_of(p.note_set_at),
            i64_of(p.note_expires_at),
            p.arrival,
            encode_image(p.avatar.as_ref()),
            encode_image(p.banner.as_ref()),
            p.banner_color
        ],
    )?;
    Ok(n > 0)
}

const PROFILE_COLUMNS: &str =
    "version, display_name, typing_template, note_text, note_set_at, note_expires_at, arrival, avatar, banner, banner_color";

fn profile_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProfileRow> {
    profile_row_at(r, 0)
}

pub fn profile(conn: &Connection, account: &[u8]) -> Result<Option<ProfileRow>> {
    Ok(conn.query_row(&format!("SELECT {PROFILE_COLUMNS} FROM profiles WHERE account_id = ?1"), [account], profile_row).optional()?)
}

/// Every profile held, with its account.
pub fn all_profiles(conn: &Connection) -> Result<Vec<(Vec<u8>, ProfileRow)>> {
    let mut stmt = conn.prepare(&format!("SELECT account_id, {PROFILE_COLUMNS} FROM profiles"))?;
    let rows = stmt.query_map([], |r| {
        let account: Vec<u8> = r.get(0)?;
        // The profile's columns follow the account's.
        Ok((account, profile_row_at(r, 1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn profile_row_at(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<ProfileRow> {
    Ok(ProfileRow {
        version: r.get::<_, i64>(at)? as u64,
        display_name: r.get(at + 1)?,
        typing_template: r.get(at + 2)?,
        note_text: r.get(at + 3)?,
        note_set_at: r.get::<_, i64>(at + 4)? as u64,
        note_expires_at: r.get::<_, i64>(at + 5)? as u64,
        arrival: r.get(at + 6)?,
        avatar: decode_image(r.get(at + 7)?),
        banner: decode_image(r.get(at + 8)?),
        banner_color: r.get(at + 9)?,
    })
}

// ---------------------------------------------------------------- pictures

/// A picture fetched and opened, by the SHA-256 of its ciphertext.
pub fn picture(conn: &Connection, sha256: &[u8]) -> Result<Option<Vec<u8>>> {
    Ok(conn.query_row("SELECT data FROM pictures WHERE sha256 = ?1", [sha256], |r| r.get(0)).optional()?)
}

/// When the server will drop a picture held here, as last heard.
pub fn picture_expiry(conn: &Connection, sha256: &[u8]) -> Result<Option<u64>> {
    Ok(conn.query_row("SELECT expires_at FROM pictures WHERE sha256 = ?1", [sha256], |r| r.get::<_, i64>(0)).optional()?.map(|e| e as u64))
}

pub fn put_picture(conn: &Connection, sha256: &[u8], data: &[u8], expires_at: u64) -> Result<()> {
    conn.execute(
        "INSERT INTO pictures (sha256, data, expires_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (sha256) DO UPDATE SET expires_at = excluded.expires_at",
        params![sha256, data, i64_of(expires_at)],
    )?;
    conn.execute("DELETE FROM picture_misses WHERE sha256 = ?1", [sha256])?;
    Ok(())
}

/// The server will keep a picture until `expires_at`.
pub fn picture_renewed(conn: &Connection, sha256: &[u8], expires_at: u64) -> Result<()> {
    conn.execute("UPDATE pictures SET expires_at = ?1 WHERE sha256 = ?2", params![i64_of(expires_at), sha256])?;
    Ok(())
}

/// Forget pictures no profile shows any more.
pub fn forget_pictures_except(conn: &Connection, keep: &std::collections::HashSet<Vec<u8>>) -> Result<usize> {
    let held: Vec<Vec<u8>> =
        conn.prepare("SELECT sha256 FROM pictures")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut n = 0;
    for sha in held.iter().filter(|s| !keep.contains(*s)) {
        n += conn.execute("DELETE FROM pictures WHERE sha256 = ?1", [sha])?;
    }
    conn.execute("DELETE FROM picture_misses WHERE retry_at < ?1", [i64_of(now_ms())])?;
    Ok(n)
}

/// Whether a picture that could not be fetched should not be tried yet.
pub fn picture_waiting(conn: &Connection, sha256: &[u8]) -> Result<bool> {
    let retry: Option<i64> = conn.query_row("SELECT retry_at FROM picture_misses WHERE sha256 = ?1", [sha256], |r| r.get(0)).optional()?;
    Ok(retry.is_some_and(|t| t as u64 > now_ms()))
}

/// A picture could not be fetched: try again after `wait_ms`, longer each
/// time it fails.
pub fn picture_missed(conn: &Connection, sha256: &[u8], wait_ms: u64) -> Result<()> {
    let tries: u32 =
        conn.query_row("SELECT tries FROM picture_misses WHERE sha256 = ?1", [sha256], |r| r.get(0)).optional()?.unwrap_or(0);
    let wait = wait_ms.saturating_mul(1 << tries.min(5));
    conn.execute(
        "INSERT INTO picture_misses (sha256, retry_at, tries) VALUES (?1, ?2, ?3)
         ON CONFLICT (sha256) DO UPDATE SET retry_at = excluded.retry_at, tries = excluded.tries",
        params![sha256, i64_of(now_ms().saturating_add(wait)), tries + 1],
    )?;
    Ok(())
}

/// One of this device's own uploads.
pub struct OwnPicture {
    pub blob_id: Vec<u8>,
    pub sha256: Vec<u8>,
    pub renew_secret: Vec<u8>,
    pub expires_at: u64,
}

pub fn add_own_picture(conn: &Connection, p: &OwnPicture) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO own_pictures (blob_id, sha256, renew_secret, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![p.blob_id, p.sha256, p.renew_secret, i64_of(p.expires_at)],
    )?;
    Ok(())
}

pub fn own_pictures(conn: &Connection) -> Result<Vec<OwnPicture>> {
    let mut stmt = conn.prepare("SELECT blob_id, sha256, renew_secret, expires_at FROM own_pictures")?;
    let rows = stmt.query_map([], |r| {
        Ok(OwnPicture { blob_id: r.get(0)?, sha256: r.get(1)?, renew_secret: r.get(2)?, expires_at: r.get::<_, i64>(3)? as u64 })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn own_picture_renewed(conn: &Connection, blob_id: &[u8], expires_at: u64) -> Result<()> {
    conn.execute("UPDATE own_pictures SET expires_at = ?1 WHERE blob_id = ?2", params![i64_of(expires_at), blob_id])?;
    Ok(())
}

pub fn remove_own_picture(conn: &Connection, blob_id: &[u8]) -> Result<()> {
    conn.execute("DELETE FROM own_pictures WHERE blob_id = ?1", [blob_id])?;
    Ok(())
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

pub fn nickname(conn: &Connection, account: &[u8]) -> Result<Option<String>> {
    synced_get(conn, Synced::Nickname, account)
}

// ---------------------------------------------------------------- self-group sync

/// Values an account's devices keep in step through its self group.
///
/// Order follows the self group's log. A value's `seq` is the log position of
/// the message that set it. NULL means this device set it and the server has
/// not placed it yet; 0 means it was set with no self group to share it. A
/// value from the log replaces one with a lower position. A pending local
/// value is never replaced: its message will land after everything already
/// logged, so it is the newer one. An empty name clears a value and stays as
/// a marker, so an older value cannot come back.
#[derive(Clone, Copy, Debug)]
pub enum Synced {
    /// Private nicknames, keyed by account.
    Nickname,
    /// Device names, keyed by device.
    DeviceName,
    /// Verified safety numbers, keyed by account: [`VERIFIED`] or empty.
    Verified,
    /// Account-wide settings, keyed by name; empty is the default.
    Setting,
}

/// The value [`Synced::Verified`] holds for a verified account.
pub const VERIFIED: &str = "1";

impl Synced {
    const ALL: [Synced; 4] = [Synced::Nickname, Synced::DeviceName, Synced::Verified, Synced::Setting];

    fn table(self) -> (&'static str, &'static str) {
        match self {
            Synced::Nickname => ("nicknames", "account_id"),
            Synced::DeviceName => ("device_names", "device_id"),
            Synced::Verified => ("verified", "account_id"),
            Synced::Setting => ("synced_settings", "key"),
        }
    }
}

/// Whether this account sends read receipts (on unless turned off).
pub fn read_receipts_on(conn: &Connection) -> Result<bool> {
    Ok(synced_get(conn, Synced::Setting, crate::content::SETTING_READ_RECEIPTS.as_bytes())?.as_deref() != Some("0"))
}

// ---------------------------------------------------------------- receipts

/// Record an account's receipt for a group: its positions only move
/// forward. Returns whether either moved.
pub fn raise_receipt(conn: &Connection, group: &[u8], account: &[u8], delivered: u64, read: u64) -> Result<bool> {
    let n = conn.execute(
        "INSERT INTO receipts (group_id, account_id, delivered_seq, read_seq) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (group_id, account_id) DO UPDATE SET
             delivered_seq = MAX(delivered_seq, excluded.delivered_seq),
             read_seq = MAX(read_seq, excluded.read_seq)
         WHERE excluded.delivered_seq > delivered_seq OR excluded.read_seq > read_seq",
        params![group, account, i64_of(delivered), i64_of(read)],
    )?;
    Ok(n > 0)
}

/// An account's positions in a group, from its receipts: (delivered, read).
pub fn receipt(conn: &Connection, group: &[u8], account: &[u8]) -> Result<(u64, u64)> {
    let r: Option<(i64, i64)> = conn
        .query_row("SELECT delivered_seq, read_seq FROM receipts WHERE group_id = ?1 AND account_id = ?2", params![group, account], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let (d, rd) = r.unwrap_or((0, 0));
    Ok((u64::try_from(d).unwrap_or(0), u64::try_from(rd).unwrap_or(0)))
}

/// Set the positions every other member has reached, from their receipts:
/// the least of each over `members`. A member with no receipt yet holds the
/// ticks back, from `joined_at`: 0 for those who were there from the start
/// (or whose joining this device did not see), and the group's position for
/// members just added, who count as having what came before them. Returns
/// the old and new (delivered, read) floors; with no other members, both 0.
pub fn set_receipt_floors(conn: &Connection, group: &[u8], members: &[AccountId], joined_at: u64) -> Result<((u64, u64), (u64, u64))> {
    let old: (i64, i64) = conn
        .query_row("SELECT delivered_floor, read_floor FROM conversations WHERE group_id = ?1", [group], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .unwrap_or((0, 0));
    let mut floor: Option<(u64, u64)> = None;
    for a in members {
        conn.execute(
            "INSERT OR IGNORE INTO receipts (group_id, account_id, delivered_seq, read_seq) VALUES (?1, ?2, ?3, ?3)",
            params![group, &a.0[..], i64_of(joined_at)],
        )?;
        let (d, r) = receipt(conn, group, &a.0)?;
        floor = Some(floor.map_or((d, r), |(fd, fr)| (fd.min(d), fr.min(r))));
    }
    let new = floor.unwrap_or((0, 0));
    conn.execute(
        "UPDATE conversations SET delivered_floor = ?2, read_floor = ?3 WHERE group_id = ?1",
        params![group, i64_of(new.0), i64_of(new.1)],
    )?;
    let old = (u64::try_from(old.0).unwrap_or(0), u64::try_from(old.1).unwrap_or(0));
    Ok((old, new))
}

/// The newest log position of a message from someone else in a group: how
/// far this account has received it.
pub fn newest_received(conn: &Connection, group: &[u8]) -> Result<u64> {
    let n: Option<i64> = conn.query_row("SELECT MAX(seq) FROM messages WHERE group_id = ?1 AND outgoing = 0", [group], |r| r.get(0))?;
    Ok(n.and_then(|n| u64::try_from(n).ok()).unwrap_or(0))
}

/// How far this device has the group's log: its cursor, or a later message
/// of ours already placed.
pub fn group_position(conn: &Connection, group: &[u8]) -> Result<u64> {
    let n: Option<i64> = conn.query_row(
        "SELECT MAX(COALESCE((SELECT cursor FROM conversations WHERE group_id = ?1), 0), COALESCE((SELECT MAX(seq) FROM messages WHERE group_id = ?1), 0))",
        [group],
        |r| r.get(0),
    )?;
    Ok(n.and_then(|n| u64::try_from(n).ok()).unwrap_or(0))
}

/// The user's read position in a group.
pub fn read_position(conn: &Connection, group: &[u8]) -> Result<u64> {
    let n: Option<i64> = conn.query_row("SELECT read_seq FROM conversations WHERE group_id = ?1", [group], |r| r.get(0)).optional()?;
    Ok(n.and_then(|n| u64::try_from(n).ok()).unwrap_or(0))
}

/// A message's position in its group's log; `None` until the server has it.
pub fn message_seq(conn: &Connection, group: &[u8], id: &[u8]) -> Result<Option<u64>> {
    let n: Option<Option<i64>> =
        conn.query_row("SELECT seq FROM messages WHERE group_id = ?1 AND id = ?2", params![group, id], |r| r.get(0)).optional()?;
    Ok(n.flatten().and_then(|n| u64::try_from(n).ok()))
}

/// Our own messages in a group whose log positions fall in `(from, to]`,
/// newest first, at most `limit`: those whose ticks just changed.
pub fn own_messages_between(conn: &Connection, group: &[u8], from: u64, to: u64, limit: u32) -> Result<Vec<Vec<u8>>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM messages WHERE group_id = ?1 AND outgoing = 1 AND seq > ?2 AND seq <= ?3 AND deleted_at IS NULL
         ORDER BY seq DESC LIMIT ?4",
    )?;
    Ok(stmt.query_map(params![group, i64_of(from), i64_of(to), limit], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// Whether the user verified this account's safety number.
pub fn is_verified(conn: &Connection, account: &[u8]) -> Result<bool> {
    Ok(synced_get(conn, Synced::Verified, account)?.is_some())
}

/// Set a value on this device. With `msg_id`, the message carrying it to the
/// self group; without, it stays local.
pub fn synced_set_local(conn: &Connection, kind: Synced, key: &[u8], name: &str, msg_id: Option<&[u8]>) -> Result<()> {
    let (t, k) = kind.table();
    let seq: Option<i64> = if msg_id.is_some() { None } else { Some(0) };
    conn.execute(
        &format!(
            "INSERT INTO {t} ({k}, name, msg_id, seq) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT ({k}) DO UPDATE SET name = excluded.name, msg_id = excluded.msg_id, seq = excluded.seq"
        ),
        params![key, name, msg_id, seq],
    )?;
    Ok(())
}

/// Apply a value from the self group's log at position `seq`. Returns whether
/// it changed anything.
pub fn synced_apply(conn: &Connection, kind: Synced, key: &[u8], name: &str, seq: u64) -> Result<bool> {
    let (t, k) = kind.table();
    let n = conn.execute(
        &format!(
            "INSERT INTO {t} ({k}, name, msg_id, seq) VALUES (?1, ?2, NULL, ?3)
             ON CONFLICT ({k}) DO UPDATE SET name = excluded.name, msg_id = NULL, seq = excluded.seq
             WHERE {t}.seq IS NOT NULL AND {t}.seq < excluded.seq"
        ),
        params![key, name, i64_of(seq)],
    )?;
    Ok(n > 0)
}

/// Apply a value from a snapshot: only where this device has none.
pub fn synced_fill(conn: &Connection, kind: Synced, key: &[u8], name: &str, seq: u64) -> Result<bool> {
    let (t, k) = kind.table();
    let n = conn.execute(&format!("INSERT OR IGNORE INTO {t} ({k}, name, msg_id, seq) VALUES (?1, ?2, NULL, ?3)"), params![key, name, i64_of(seq)])?;
    Ok(n > 0)
}

pub fn synced_get(conn: &Connection, kind: Synced, key: &[u8]) -> Result<Option<String>> {
    let (t, k) = kind.table();
    Ok(conn.query_row(&format!("SELECT name FROM {t} WHERE {k} = ?1 AND name != ''"), [key], |r| r.get(0)).optional()?)
}

/// Every set value, for a snapshot.
pub fn synced_all(conn: &Connection, kind: Synced) -> Result<Vec<(Vec<u8>, String)>> {
    let (t, k) = kind.table();
    let mut stmt = conn.prepare(&format!("SELECT {k}, name FROM {t} WHERE name != ''"))?;
    Ok(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
}

/// The message carrying our own values landed at `seq`, or (`None`) failed for
/// good and stays local.
fn synced_placed(conn: &Connection, msg_id: &[u8], seq: Option<u64>) -> Result<()> {
    for kind in Synced::ALL {
        let (t, _) = kind.table();
        conn.execute(
            &format!("UPDATE {t} SET seq = ?1 WHERE msg_id = ?2 AND seq IS NULL"),
            params![i64_of(seq.unwrap_or(0)), msg_id],
        )?;
    }
    Ok(())
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
    add_notice_of(conn, group, "notice", text)
}

/// A notice of a kind (see [`NoticeRow::kind`]).
pub fn add_notice_of(conn: &Connection, group: Option<&[u8]>, kind: &str, text: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO notices (group_id, at, text, kind) VALUES (?1, ?2, ?3, ?4)",
        params![group, i64_of(now_ms()), text, kind],
    )?;
    Ok(())
}

// ---------------------------------------------------------------- announcements

/// The ID of the newest announcement fetched; fetching continues after it.
pub const META_ANNOUNCEMENTS_SEEN: &str = "announcements_seen";
/// The ID of the newest announcement the user has read.
pub const META_ANNOUNCEMENTS_READ: &str = "announcements_read";
/// "1" when this account is one of the server's hosts.
pub const META_HOST: &str = "host";
/// The maintenance planned or underway, as the server's protobuf; absent
/// when there is none.
pub const META_MAINTENANCE: &str = "maintenance";

/// One stored announcement, as the server sent it.
#[derive(Clone, Debug)]
pub struct AnnouncementRow {
    pub id: u64,
    pub created_at: u64,
    pub kind: i32,
    pub text: String,
    pub starts_at: u64,
    pub minutes: u32,
}

/// Store announcements just fetched, and continue after the last of them.
/// Returns how many were new.
pub fn add_announcements(conn: &Connection, entries: &[post_proto::Announcement]) -> Result<usize> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO announcements (id, created_at, kind, text, starts_at, minutes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut added = 0;
    for a in entries {
        added += stmt.execute(params![i64_of(a.id), i64_of(a.created_at_ms), a.kind, a.text, i64_of(a.starts_at_ms), a.minutes])?;
    }
    if let Some(last) = entries.last() {
        meta_put_u64(conn, META_ANNOUNCEMENTS_SEEN, last.id.max(meta_u64(conn, META_ANNOUNCEMENTS_SEEN)?))?;
    }
    Ok(added)
}

fn announcement_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AnnouncementRow> {
    Ok(AnnouncementRow {
        id: u64::try_from(r.get::<_, i64>(0)?).unwrap_or(0),
        created_at: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
        kind: r.get(2)?,
        text: r.get(3)?,
        starts_at: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
        minutes: r.get(5)?,
    })
}

/// The newest `limit` announcements, oldest first.
pub fn announcements(conn: &Connection, limit: u32) -> Result<Vec<AnnouncementRow>> {
    let mut stmt = conn.prepare(
        "SELECT * FROM (SELECT id, created_at, kind, text, starts_at, minutes FROM announcements ORDER BY id DESC LIMIT ?1) ORDER BY id",
    )?;
    Ok(stmt.query_map([limit], announcement_row)?.collect::<rusqlite::Result<_>>()?)
}

/// The newest announcement.
pub fn latest_announcement(conn: &Connection) -> Result<Option<AnnouncementRow>> {
    Ok(announcements(conn, 1)?.pop())
}

/// Announcements newer than the read position.
pub fn unread_announcements(conn: &Connection) -> Result<u32> {
    let read = i64_of(meta_u64(conn, META_ANNOUNCEMENTS_READ)?);
    let n: i64 = conn.query_row("SELECT count(*) FROM announcements WHERE id > ?1", [read], |r| r.get(0))?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Move the announcements read position forward to `id`. Returns whether
/// it moved.
pub fn raise_announcements_read(conn: &Connection, id: u64) -> Result<bool> {
    if id <= meta_u64(conn, META_ANNOUNCEMENTS_READ)? {
        return Ok(false);
    }
    meta_put_u64(conn, META_ANNOUNCEMENTS_READ, id)?;
    Ok(true)
}

/// Forget the server's announcements, when this profile's account goes.
pub fn forget_announcements(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM announcements", [])?;
    for k in [META_ANNOUNCEMENTS_SEEN, META_ANNOUNCEMENTS_READ, META_HOST, META_MAINTENANCE] {
        meta_delete(conn, k)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let mut c = Connection::open_in_memory().expect("memory db");
        migrate(&mut c).expect("migrate");
        c
    }

    #[test]
    fn version_one_databases_are_migrated() {
        // A profile written before schema versioning.
        let mut c = Connection::open_in_memory().expect("memory db");
        c.execute_batch(SCHEMA).expect("v1 schema");
        c.execute("INSERT INTO nicknames (account_id, name) VALUES (x'01', 'Ally')", []).expect("v1 row");
        migrate(&mut c).expect("migrate");
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).expect("version");
        assert_eq!(version as usize, MIGRATIONS.len() + 1);
        assert_eq!(nickname(&c, &[1]).expect("read").as_deref(), Some("Ally"));
        // A value set before syncing existed loses to anything from the log.
        assert!(synced_apply(&c, Synced::Nickname, &[1], "Al", 1).expect("apply"));
        // Running again changes nothing.
        migrate(&mut c).expect("migrate again");
        assert_eq!(nickname(&c, &[1]).expect("read").as_deref(), Some("Al"));
        // A newer schema is refused rather than misread.
        c.pragma_update(None, "user_version", 99).expect("set version");
        assert!(migrate(&mut c).is_err());
    }

    #[test]
    fn notices_by_conversation() {
        let c = db();
        add_notice(&c, Some(&[1]), "first in one").expect("add");
        add_notice(&c, None, "account wide").expect("add");
        add_notice(&c, Some(&[2]), "in two").expect("add");
        add_notice(&c, Some(&[1]), "second in one").expect("add");
        let texts = |groups: &[Vec<u8>], limit| notices(&c, groups, limit).expect("read").into_iter().map(|n| n.text).collect::<Vec<_>>();
        assert_eq!(texts(&[vec![1]], 10), ["first in one", "second in one"]);
        assert_eq!(texts(&[vec![1]], 1), ["second in one"], "the newest, when limited");
        assert_eq!(texts(&[], 10), ["account wide"]);
        assert_eq!(texts(&[vec![1], vec![2]], 10).len(), 3);
    }

    #[test]
    fn synced_values_follow_log_order() {
        let c = db();
        let get = |c: &Connection| nickname(c, &[1]).expect("read");

        // A pending local change is newer than anything already logged.
        synced_set_local(&c, Synced::Nickname, &[1], "mine", Some(b"m1")).expect("set");
        assert!(!synced_apply(&c, Synced::Nickname, &[1], "older", 4).expect("apply"));
        assert_eq!(get(&c).as_deref(), Some("mine"));

        // Once placed at 5, it loses to later entries only.
        synced_placed(&c, b"m1", Some(5)).expect("placed");
        assert!(!synced_apply(&c, Synced::Nickname, &[1], "older", 3).expect("apply"));
        assert!(synced_apply(&c, Synced::Nickname, &[1], "newer", 7).expect("apply"));
        assert_eq!(get(&c).as_deref(), Some("newer"));

        // A snapshot fills gaps and never overwrites.
        assert!(!synced_fill(&c, Synced::Nickname, &[1], "stale", 9).expect("fill"));
        assert!(synced_fill(&c, Synced::Nickname, &[2], "filled", 9).expect("fill"));
        assert_eq!(nickname(&c, &[2]).expect("read").as_deref(), Some("filled"));

        // Clearing leaves a marker, so an older value cannot return.
        assert!(synced_apply(&c, Synced::Nickname, &[1], "", 10).expect("clear"));
        assert_eq!(get(&c), None);
        assert!(!synced_fill(&c, Synced::Nickname, &[1], "stale", 11).expect("fill"));
        assert!(!synced_apply(&c, Synced::Nickname, &[1], "older", 8).expect("apply"));
        assert_eq!(synced_all(&c, Synced::Nickname).expect("all"), vec![(vec![2], "filled".to_string())]);

        // A change that failed to send stays, and gives way to the log.
        synced_set_local(&c, Synced::DeviceName, &[3], "laptop", Some(b"m2")).expect("set");
        synced_placed(&c, b"m2", None).expect("failed");
        assert!(synced_apply(&c, Synced::DeviceName, &[3], "work laptop", 1).expect("apply"));
    }
}
