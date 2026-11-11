//! Postern client core (design section 11).
//!
//! Owns every key, all protocol state and the local database. User
//! interfaces (the CLI, the JSON-RPC daemon, later the Flutter app) are views
//! over this API.
//!
//! Concurrency: all protocol work goes through one engine behind an async
//! mutex, so MLS state and network ordering never race. Reads (conversation
//! lists, messages) use a second database connection and never wait for the
//! network.

mod content;
mod engine;
mod link;
mod net;
mod provider;
mod store;

use std::fmt;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tokio::sync::broadcast;
use zeroize::Zeroizing;

pub use content::{MAX_NOTE, MAX_TYPING_TEMPLATE, NAME_PLACEHOLDER, render_typing};
use engine::Engine;

/// Everything that can go wrong in the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No account on this profile yet.
    #[error("no account on this device yet; register first")]
    NotRegistered,
    /// This profile already has an account.
    #[error("this device already has an account")]
    AlreadyRegistered,
    /// Not connected to the server.
    #[error("not connected to the server")]
    NotConnected,
    /// The server refused a request.
    #[error("server: {0}")]
    Server(postern_proto::Error),
    /// The network failed.
    #[error("network: {0}")]
    Network(String),
    /// The server broke the protocol.
    #[error("protocol: {0}")]
    Protocol(String),
    /// The local database failed.
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    /// OpenMLS storage failed.
    #[error("storage: {0}")]
    Storage(String),
    /// The local database holds something invalid.
    #[error("local data is corrupt: {0}")]
    Corrupt(&'static str),
    /// The database key does not match.
    #[error("wrong database key")]
    WrongKey,
    /// An MLS operation failed.
    #[error("mls: {0}")]
    Mls(String),
    /// An identity operation failed.
    #[error("identity: {0}")]
    Identity(#[from] postern_identity::Error),
    /// A username now belongs to a different account than first seen.
    #[error("the account for {0} has changed; verify their safety number before trusting it")]
    IdentityChanged(String),
    /// No such user in the directory.
    #[error("no user named {0}")]
    UnknownUser(String),
    /// The other person has no device that can be added right now.
    #[error("{0} has no device available to start a conversation with; ask them to open the app")]
    PeerUnavailable(String),
    /// No such conversation or message.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// Not permitted.
    #[error("{0}")]
    NotAllowed(&'static str),
    /// Bad input from the user.
    #[error("{0}")]
    InvalidInput(String),
    /// No anonymous tokens left today.
    #[error("daily limit for starting conversations reached; try again tomorrow")]
    NoTokens,
    /// The operating system's random number generator failed.
    #[error("randomness unavailable")]
    Rng,
}

/// Result type for the core.
pub type Result<T> = std::result::Result<T, Error>;

/// The 32-byte key encrypting a profile's database.
pub struct DbKey(Zeroizing<[u8; 32]>);

impl DbKey {
    /// A key from raw bytes.
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(Zeroizing::new(b))
    }

    /// A new random key.
    pub fn generate() -> Result<Self> {
        let mut b = Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut b[..]).map_err(|_| Error::Rng)?;
        Ok(Self(b))
    }

    /// The raw key.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

macro_rules! hex_id {
    ($name:ident, $what:literal) => {
        #[doc = concat!("A ", $what, " ID: 16 bytes, written as 32 hex characters.")]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub [u8; 16]);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&hex::encode(self.0))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({self})", stringify!($name))
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                let mut b = [0u8; 16];
                hex::decode_to_slice(s.trim(), &mut b).map_err(|_| Error::InvalidInput(format!("not a {} id: {s}", $what)))?;
                Ok(Self(b))
            }
        }
    };
}

hex_id!(ConversationId, "conversation");
hex_id!(MessageId, "message");

/// The account on this device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Username.
    pub username: String,
    /// Account ID, hex.
    pub account_id: String,
    /// This device's ID, hex.
    pub device_id: String,
    /// Server URL.
    pub server: String,
    /// Whether this is a bot account.
    pub bot: bool,
}

/// What kind of conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationKind {
    /// A direct message between two people.
    Dm,
    /// A named group.
    Group,
}

/// A conversation as shown in a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversation {
    /// Its ID.
    pub id: ConversationId,
    /// DM or group.
    pub kind: ConversationKind,
    /// What to call it: the other person's shown name for a DM, the group name.
    pub title: String,
    /// The other person's username, for a DM.
    pub peer: Option<String>,
    /// Unix milliseconds of the latest activity.
    pub last_activity_ms: u64,
    /// False once this device was removed: still readable, no longer updated.
    pub following: bool,
}

/// Delivery state of a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageState {
    /// Written locally, not yet accepted by the server.
    Pending,
    /// Accepted by the server.
    Sent,
    /// Received from someone else.
    Received,
    /// The server refused it.
    Failed,
}

/// A message as shown in a conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// Its ID.
    pub id: MessageId,
    /// The conversation it is in.
    pub conversation: ConversationId,
    /// The name the viewer sees for the sender in this conversation.
    pub sender: String,
    /// The sender's username.
    pub sender_username: String,
    /// Sent by this account.
    pub outgoing: bool,
    /// When the sender sent it, by their clock (Unix ms).
    pub sent_at_ms: u64,
    /// The latest text. `None` if deleted or unsupported.
    pub body: Option<String>,
    /// Deleted by its sender. Every version is gone.
    pub deleted: bool,
    /// A message type this client does not understand.
    pub unsupported: bool,
    /// Edited at least once.
    pub edited: bool,
    /// Number of versions, the original included.
    pub versions: u32,
    /// Delivery state.
    pub state: MessageState,
}

/// One version of a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageVersion {
    /// 0 is the original.
    pub version: u32,
    /// The text of this version.
    pub body: String,
    /// When it was written, by the sender's clock (Unix ms).
    pub at_ms: u64,
    /// Not yet accepted by the server (own edits only).
    pub pending: bool,
}

/// What this device knows about a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    /// Username.
    pub username: String,
    /// The name shown for them outside any conversation.
    pub shown_name: String,
    /// The display name they chose; empty if none.
    pub display_name: String,
    /// The nickname this user gave them; empty if none.
    pub nickname: String,
    /// Their typing template; empty for the standard text.
    pub typing_template: String,
    /// Their note, if set and not expired.
    pub note: Option<String>,
}

/// A device asking to join this account, as shown before approving it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRequest {
    /// The new device's ID, hex.
    pub device_id: String,
    /// The name its user gave it; empty if none was valid.
    pub name: String,
}

/// A member of a conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Their username.
    pub username: String,
    /// The name you see for them in this conversation.
    pub shown_name: String,
    /// They may add and remove people and change the group's details.
    pub admin: bool,
    /// How many of their devices are in the conversation.
    pub devices: u32,
    /// It is you.
    pub you: bool,
}

/// One of this account's devices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Its ID, hex.
    pub id: String,
    /// The name it was given; empty if none.
    pub name: String,
    /// Whether it is the device this call runs on.
    pub this_device: bool,
}

/// A change to this account's profile. `None` leaves a field unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileUpdate {
    /// New global display name; empty clears it.
    pub display_name: Option<String>,
    /// New typing template; empty restores the standard text.
    pub typing_template: Option<String>,
    /// New note; empty clears it.
    pub note: Option<String>,
    /// With a new note: keep it until changed instead of 24 hours.
    pub note_keep: bool,
}

/// Something that happened, for live views.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A conversation was created or joined.
    ConversationAdded(Conversation),
    /// A conversation's details changed.
    ConversationChanged(Conversation),
    /// A new message.
    MessageAdded(Message),
    /// A message was edited or its delivery state changed.
    MessageChanged(Message),
    /// A message was deleted.
    MessageDeleted {
        /// Its conversation.
        conversation: ConversationId,
        /// Its ID.
        id: MessageId,
    },
    /// Someone's profile or the name shown for them changed.
    ProfileChanged {
        /// Their username.
        username: String,
    },
    /// An account added or removed devices. Contacts stay verified through
    /// this (the safety number does not change), but a device added to your
    /// own account by another of your devices deserves a look: if it was not
    /// you, remove it.
    DevicesChanged {
        /// Whose devices, possibly your own.
        username: String,
        /// How many devices were added.
        added: u32,
        /// How many were removed.
        removed: u32,
    },
    /// Something the user should know about, usually a security warning.
    Notice {
        /// The conversation it concerns, if any.
        conversation: Option<ConversationId>,
        /// What happened.
        text: String,
    },
}

pub(crate) fn conversation_from_row(conn: &Connection, c: &store::ConversationRow) -> Result<Conversation> {
    let peer = match &c.peer_account {
        Some(p) => store::username_of(conn, p)?,
        None => None,
    };
    let title = match (&c.peer_account, c.kind) {
        (Some(p), store::KIND_DM) => store::shown_name(conn, None, p)?,
        _ => c.name.clone(),
    };
    Ok(Conversation {
        id: ConversationId(c.group_id),
        kind: if c.kind == store::KIND_DM { ConversationKind::Dm } else { ConversationKind::Group },
        title,
        peer,
        last_activity_ms: c.last_activity as u64,
        following: c.following && !c.leaving,
    })
}

pub(crate) fn message_from_row(conn: &Connection, m: &store::MessageRow) -> Result<Message> {
    let conversation = ConversationId(m.group_id.as_slice().try_into().map_err(|_| Error::Corrupt("group id"))?);
    Ok(Message {
        id: MessageId(m.id.as_slice().try_into().map_err(|_| Error::Corrupt("message id"))?),
        conversation,
        sender: store::shown_name(conn, Some(&m.group_id), &m.sender_account)?,
        sender_username: store::username_of(conn, &m.sender_account)?.unwrap_or_default(),
        outgoing: m.outgoing,
        sent_at_ms: m.sent_at as u64,
        body: if m.deleted { None } else { m.body.clone() },
        deleted: m.deleted,
        unsupported: m.unsupported,
        edited: m.versions > 1,
        versions: m.versions,
        state: match m.state {
            store::STATE_PENDING => MessageState::Pending,
            store::STATE_SENT => MessageState::Sent,
            store::STATE_FAILED => MessageState::Failed,
            _ => MessageState::Received,
        },
    })
}

/// A client: one account on one device.
#[derive(Clone)]
pub struct Core {
    engine: Arc<tokio::sync::Mutex<Engine>>,
    reader: Arc<Mutex<Connection>>,
    events: broadcast::Sender<Event>,
}

impl Core {
    /// Open (creating if needed) the profile in `dir`, encrypted with `key`.
    pub fn open(dir: &Path, key: &DbKey) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| Error::Storage(format!("creating {}: {e}", dir.display())))?;
        let path = dir.join("profile.db");
        let mut conn = store::open(&path, key)?;
        store::migrate(&mut conn)?;
        let reader = store::open(&path, key)?;
        let (events, _) = broadcast::channel(1024);
        let engine = Engine::new(conn, events.clone())?;
        Ok(Self { engine: Arc::new(tokio::sync::Mutex::new(engine)), reader: Arc::new(Mutex::new(reader)), events })
    }

    fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.reader.lock().unwrap_or_else(|p| p.into_inner());
        f(&conn)
    }

    /// Live events. Lagging receivers miss events but can re-read state.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// The account on this device, if registered.
    pub async fn identity(&self) -> Option<Identity> {
        let e = self.engine.lock().await;
        let me = e.me.as_ref().filter(|_| e.registered)?;
        Some(Identity {
            username: me.username.clone(),
            account_id: me.account.to_string(),
            device_id: me.device.to_string(),
            server: me.url.clone(),
            bot: me.bot,
        })
    }

    /// Create an account. `server` is a `wss://` URL.
    pub async fn register(&self, server: &str, invite: &str, username: &str, bot: bool) -> Result<Identity> {
        self.engine.lock().await.register(server, invite, username, bot).await?;
        self.identity().await.ok_or(Error::NotRegistered)
    }

    /// On a new device: offer it for linking to an existing account on
    /// `server`. Returns the code to enter on one of that account's devices;
    /// then call [`Core::finish_link`]. `bot` must match the account.
    pub async fn begin_link(&self, server: &str, device_name: &str, bot: bool) -> Result<String> {
        Ok(self.engine.lock().await.begin_link(server, device_name, bot).await?.to_string())
    }

    /// On the new device: wait (up to ten minutes) for the code to be
    /// entered and approved, then log in and join the account's conversations.
    pub async fn finish_link(&self) -> Result<Identity> {
        self.engine.lock().await.finish_link().await?;
        self.identity().await.ok_or(Error::NotRegistered)
    }

    /// On an existing device: which device a link code would add.
    pub async fn link_inspect(&self, code: &str) -> Result<LinkRequest> {
        let offer = self.engine.lock().await.link_inspect(code).await?;
        Ok(LinkRequest { device_id: offer.entry.device_id().to_string(), name: offer.name })
    }

    /// On an existing device: add the device behind a link code to this
    /// account and to every conversation.
    pub async fn link_approve(&self, code: &str) -> Result<LinkRequest> {
        let offer = self.engine.lock().await.link_approve(code).await?;
        Ok(LinkRequest { device_id: offer.entry.device_id().to_string(), name: offer.name })
    }

    /// This account's devices, oldest first.
    pub async fn devices(&self) -> Result<Vec<Device>> {
        let engine = self.engine.lock().await;
        let me = engine.me.as_ref().filter(|_| engine.registered).ok_or(Error::NotRegistered)?.device;
        Ok(engine
            .devices()?
            .into_iter()
            .map(|(id, name)| Device { id: id.to_string(), name, this_device: id == me })
            .collect())
    }

    /// Name one of this account's devices, given its ID or an unambiguous
    /// prefix. The name is shared only with the account's own devices.
    pub async fn rename_device(&self, device: &str, name: &str) -> Result<()> {
        let mut engine = self.engine.lock().await;
        let id = engine.resolve_device(device)?;
        engine.rename_device(id, name).await
    }

    /// Remove another of this account's devices, given its ID or an
    /// unambiguous prefix. It is cut off from the server and from every
    /// conversation; the account's contacts see that it was removed.
    pub async fn remove_device(&self, device: &str) -> Result<()> {
        let mut engine = self.engine.lock().await;
        let id = engine.resolve_device(device)?;
        engine.remove_device(id).await
    }

    /// Log in and catch up. Safe to call when already connected.
    pub async fn connect(&self) -> Result<()> {
        self.engine.lock().await.connect().await
    }

    /// Whether both server connections are up.
    pub async fn is_connected(&self) -> bool {
        self.engine.lock().await.is_connected()
    }

    /// Fetch and process everything new, and send what is queued.
    pub async fn sync(&self) -> Result<()> {
        self.engine.lock().await.sync().await
    }

    /// Process server wakes until a connection drops. Returns
    /// [`Error::NotConnected`] then; call [`Core::connect`] to resume.
    pub async fn listen(&self) -> Result<()> {
        loop {
            let [(s_wake, s_closed), (a_wake, a_closed)] = self.engine.lock().await.signals()?;
            tokio::select! {
                () = s_wake.notified() => {}
                () = a_wake.notified() => {}
                () = s_closed => return Err(Error::NotConnected),
                () = a_closed => return Err(Error::NotConnected),
            }
            self.sync().await?;
        }
    }

    /// Open a DM with `username`, creating it if needed.
    pub async fn start_dm(&self, username: &str) -> Result<Conversation> {
        let id = self.engine.lock().await.start_dm(username).await?;
        self.conversation(id)
    }

    /// Create a named group with you as admin and these users as members.
    pub async fn create_group(&self, name: &str, usernames: &[String]) -> Result<Conversation> {
        let id = self.engine.lock().await.create_group(name, usernames).await?;
        self.conversation(id)
    }

    /// A conversation by ID.
    pub fn conversation(&self, id: ConversationId) -> Result<Conversation> {
        self.read(|c| {
            let row = store::conversation(c, &id.0)?.filter(|r| r.kind != store::KIND_SELF).ok_or(Error::NotFound("conversation"))?;
            conversation_from_row(c, &row)
        })
    }

    /// As an admin: add people to a group, with all their devices.
    pub async fn add_members(&self, conversation: ConversationId, usernames: &[String]) -> Result<()> {
        self.engine.lock().await.add_members(conversation.0, usernames).await
    }

    /// As an admin: remove people from a group, with all their devices.
    pub async fn remove_members(&self, conversation: ConversationId, usernames: &[String]) -> Result<()> {
        self.engine.lock().await.remove_members(conversation.0, usernames).await
    }

    /// As an admin: rename a group.
    pub async fn rename_group(&self, conversation: ConversationId, name: &str) -> Result<()> {
        self.engine.lock().await.rename_group(conversation.0, name).await
    }

    /// As an admin: make a member an admin, or stop them being one. A group
    /// always keeps at least one admin.
    pub async fn set_admin(&self, conversation: ConversationId, username: &str, admin: bool) -> Result<()> {
        self.engine.lock().await.set_admin(conversation.0, username, admin).await
    }

    /// As an admin: whether new members are shown past messages, and how many
    /// days back (0 means all of them).
    pub async fn set_history(&self, conversation: ConversationId, shared: bool, days: u32) -> Result<()> {
        self.engine.lock().await.set_history(conversation.0, shared, days).await
    }

    /// Give this device fresh keys in every conversation now. This happens by
    /// itself weekly; doing it at once limits what a copy of the device's
    /// keys taken earlier can read from here on.
    pub async fn refresh_keys(&self) -> Result<()> {
        self.engine.lock().await.refresh_keys().await
    }

    /// Leave a group, with all your devices. It shows as left at once; the
    /// other members' devices complete the removal.
    pub async fn leave_group(&self, conversation: ConversationId) -> Result<()> {
        self.engine.lock().await.leave_group(conversation.0).await
    }

    /// The members of a conversation, by the name you see for them.
    pub async fn members(&self, conversation: ConversationId) -> Result<Vec<Member>> {
        let me = self.engine.lock().await.me.as_ref().ok_or(Error::NotRegistered)?.account;
        self.read(|c| {
            store::conversation(c, &conversation.0)?.filter(|r| r.kind != store::KIND_SELF).ok_or(Error::NotFound("conversation"))?;
            let (meta, accounts) = engine::group_members(c, &conversation.0)?;
            let mut out = accounts
                .into_iter()
                .map(|(account, devices)| {
                    Ok(Member {
                        username: store::username_of(c, &account.0)?.unwrap_or_default(),
                        shown_name: store::shown_name(c, Some(&conversation.0), &account.0)?,
                        admin: meta.admins.iter().any(|a| a[..] == account.0),
                        devices,
                        you: account == me,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            // You first, then everyone by the name you see.
            out.sort_by_key(|m| (!m.you, m.shown_name.to_lowercase()));
            Ok(out)
        })
    }

    /// Live conversations, most recent first.
    pub fn conversations(&self) -> Result<Vec<Conversation>> {
        self.read(|c| store::conversations(c)?.iter().map(|r| conversation_from_row(c, r)).collect())
    }

    /// The newest `limit` messages of a conversation, oldest first.
    pub fn messages(&self, conversation: ConversationId, limit: u32) -> Result<Vec<Message>> {
        self.read(|c| {
            let groups = store::conversation_groups(c, &conversation.0)?;
            if groups.is_empty() {
                return Err(Error::NotFound("conversation"));
            }
            store::messages(c, &groups, limit)?.iter().map(|m| message_from_row(c, m)).collect()
        })
    }

    fn find_message(c: &Connection, conversation: ConversationId, id: MessageId) -> Result<store::MessageRow> {
        for g in store::conversation_groups(c, &conversation.0)? {
            if let Some(m) = store::message(c, &g, &id.0)? {
                return Ok(m);
            }
        }
        Err(Error::NotFound("message"))
    }

    /// One message.
    pub fn message(&self, conversation: ConversationId, id: MessageId) -> Result<Message> {
        self.read(|c| message_from_row(c, &Self::find_message(c, conversation, id)?))
    }

    /// Every version of a message, the original first.
    pub fn message_versions(&self, conversation: ConversationId, id: MessageId) -> Result<Vec<MessageVersion>> {
        self.read(|c| {
            let m = Self::find_message(c, conversation, id)?;
            Ok(store::versions(c, &m.group_id, &m.id)?
                .into_iter()
                .enumerate()
                .map(|(i, v)| MessageVersion { version: i as u32, body: v.body, at_ms: v.at as u64, pending: v.pending })
                .collect())
        })
    }

    /// Send a text message.
    pub async fn send_text(&self, conversation: ConversationId, text: &str) -> Result<Message> {
        let body = content::Body::Text(content::Text { body: text.into() });
        let id = self.engine.lock().await.send_body(conversation.0, body).await?;
        self.message(conversation, id)
    }

    /// Edit one of your own messages. Every earlier version is kept.
    pub async fn edit_message(&self, conversation: ConversationId, id: MessageId, text: &str) -> Result<Message> {
        let group = self.read(|c| Ok(Self::find_message(c, conversation, id)?.group_id))?;
        let gid = group.as_slice().try_into().map_err(|_| Error::Corrupt("group id"))?;
        let body = content::Body::Edit(content::Edit { target: id.0.to_vec(), body: text.into() });
        self.engine.lock().await.send_body(gid, body).await?;
        self.message(conversation, id)
    }

    /// Delete one of your own messages, with all its versions, for everyone.
    pub async fn delete_message(&self, conversation: ConversationId, id: MessageId) -> Result<()> {
        let group = self.read(|c| Ok(Self::find_message(c, conversation, id)?.group_id))?;
        let gid = group.as_slice().try_into().map_err(|_| Error::Corrupt("group id"))?;
        self.engine.lock().await.send_body(gid, content::Body::Delete(content::Delete { target: id.0.to_vec() })).await?;
        Ok(())
    }

    /// Change your profile and share it with every conversation.
    pub async fn set_profile(&self, update: ProfileUpdate) -> Result<()> {
        let mut engine = self.engine.lock().await;
        let mine = engine.me.as_ref().ok_or(Error::NotRegistered)?.account;
        let current = store::profile(&engine.conn, &mine.0)?.unwrap_or(store::ProfileRow {
            version: 0,
            display_name: String::new(),
            typing_template: String::new(),
            note_text: String::new(),
            note_set_at: 0,
            note_expires_at: 0,
        });
        let now = store::now_ms();
        let (note_text, note_set_at, note_expires_at) = match update.note {
            Some(n) if n.is_empty() => (String::new(), 0, 0),
            Some(n) => (n, now, if update.note_keep { 0 } else { now + content::NOTE_DEFAULT_MS }),
            None => (current.note_text, current.note_set_at, current.note_expires_at),
        };
        engine
            .set_profile(store::ProfileRow {
                version: 0,
                display_name: update.display_name.unwrap_or(current.display_name),
                typing_template: update.typing_template.unwrap_or(current.typing_template),
                note_text,
                note_set_at,
                note_expires_at,
            })
            .await
    }

    /// Set (or clear, with an empty name) your display name in one conversation.
    pub async fn set_conversation_name(&self, conversation: ConversationId, name: &str) -> Result<()> {
        let body = content::Body::ConversationName(content::ConversationName { name: name.into() });
        self.engine.lock().await.send_body(conversation.0, body).await?;
        Ok(())
    }

    /// Set (or clear, with an empty name) a private nickname for someone.
    pub async fn set_nickname(&self, username: &str, nickname: &str) -> Result<()> {
        self.engine.lock().await.set_nickname(username, nickname).await
    }

    /// What this device knows about a person.
    pub async fn person(&self, username: &str) -> Result<Person> {
        let account = self.engine.lock().await.resolve(username).await?;
        self.read(|c| {
            let p = store::profile(c, &account.0)?;
            let now = store::now_ms();
            Ok(Person {
                username: username.into(),
                shown_name: store::shown_name(c, None, &account.0)?,
                display_name: p.as_ref().map(|p| p.display_name.clone()).unwrap_or_default(),
                nickname: store::nickname(c, &account.0)?.unwrap_or_default(),
                typing_template: p.as_ref().map(|p| p.typing_template.clone()).unwrap_or_default(),
                note: p.filter(|p| !p.note_text.is_empty() && (p.note_expires_at == 0 || p.note_expires_at > now)).map(|p| p.note_text),
            })
        })
    }

    /// The safety number between you and `username`, as 12 groups of 5 digits.
    pub async fn safety_number(&self, username: &str) -> Result<String> {
        let mut engine = self.engine.lock().await;
        let peer = engine.resolve(username).await?;
        let mine = engine.me.as_ref().ok_or(Error::NotRegistered)?.account;
        Ok(postern_identity::safety_number(&mine, &peer).to_string())
    }
}

/// Every `Core` future must be `Send`, so callers can spawn it. A database
/// transaction held across an `.await` breaks this; this function makes that
/// a compile error here rather than in whichever crate spawns first.
#[allow(dead_code, clippy::let_underscore_future)]
fn core_futures_are_send(c: &Core, conv: ConversationId, msg: MessageId) {
    fn send<T: Send>(_: T) {}
    send(c.identity());
    send(c.register("", "", "", false));
    send(c.begin_link("", "", false));
    send(c.finish_link());
    send(c.link_inspect(""));
    send(c.link_approve(""));
    send(c.devices());
    send(c.rename_device("", ""));
    send(c.remove_device(""));
    send(c.connect());
    send(c.is_connected());
    send(c.sync());
    send(c.listen());
    send(c.start_dm(""));
    send(c.create_group("", &[]));
    send(c.members(conv));
    send(c.add_members(conv, &[]));
    send(c.remove_members(conv, &[]));
    send(c.rename_group(conv, ""));
    send(c.set_admin(conv, "", true));
    send(c.set_history(conv, true, 0));
    send(c.leave_group(conv));
    send(c.refresh_keys());
    send(c.send_text(conv, ""));
    send(c.edit_message(conv, msg, ""));
    send(c.delete_message(conv, msg));
    send(c.set_profile(ProfileUpdate::default()));
    send(c.set_conversation_name(conv, ""));
    send(c.set_nickname("", ""));
    send(c.person(""));
    send(c.safety_number(""));
}
