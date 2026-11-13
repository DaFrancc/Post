//! Post client core (design section 11).
//!
//! Owns every key, all protocol state and the local database. User
//! interfaces (the CLI, the JSON-RPC daemon, later the Flutter app) are views
//! over this API.
//!
//! Concurrency: all protocol work goes through one engine behind an async
//! mutex, so MLS state and network ordering never race. Reads (conversation
//! lists, messages) use a second database connection and never wait for the
//! network.

pub mod keyfile;

mod blob;
mod calls;
mod content;
mod engine;
mod ephemeral;
mod link;
mod net;
pub mod picture;
mod provider;
mod store;
pub mod update;

use std::fmt;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use post_api_macros::api;
use rusqlite::Connection;
use tokio::sync::broadcast;
use zeroize::Zeroizing;

pub use content::{Fill, MAX_BIO, MAX_NOTE, MAX_TYPING_TEMPLATE, NAME_PLACEHOLDER, render_typing};
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
    /// This profile was closed (logging out), so it no longer connects.
    #[error("this profile is closed")]
    Closed,
    /// The server refused a request.
    #[error("server: {0}")]
    Server(post_proto::Error),
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
    Identity(#[from] post_identity::Error),
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
    /// Checking for, fetching or installing an app update failed; one
    /// sentence saying why.
    #[error("{0}")]
    Update(String),
    /// A picture could not be used: not an image, too big, or damaged on
    /// the way; one sentence saying why.
    #[error("{0}")]
    Picture(String),
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

        // In the local API: 32 hex characters, as written above.
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

hex_id!(ConversationId, "conversation");
hex_id!(MessageId, "message");
hex_id!(CallId, "call");

/// A call in a conversation, as this device knows it (design section 10).
/// Calls live only in memory: who was in a call is never stored.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// Its ID.
    pub id: CallId,
    /// Where.
    pub conversation: ConversationId,
    /// Who started it (username).
    pub started_by: String,
    /// When it started (Unix ms, the starter's clock).
    #[api(unix_ms)]
    pub started_at_ms: u64,
    /// Started as a video call.
    pub video: bool,
    /// Who is in it now.
    pub participants: Vec<CallParticipant>,
    /// It is calling the user: someone else started it moments ago, and none
    /// of the user's devices has joined or declined.
    pub ringing: bool,
    /// This device is in it.
    pub joined: bool,
    /// It is over.
    pub ended: bool,
}

/// Someone in a call.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallParticipant {
    /// Their random name in this call (hex): their identity in LiveKit.
    pub participant_id: String,
    /// Who they are.
    pub username: String,
    /// One of the user's own devices.
    pub you: bool,
    /// This device.
    pub this_device: bool,
}

/// What a device needs to join a call's room.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallJoin {
    /// The call, with this device in it.
    pub call: Call,
    /// This device's random name in the call (hex).
    pub participant_id: String,
    /// The LiveKit server's URL.
    pub url: String,
    /// A short-lived LiveKit access token.
    pub token: String,
    /// The host's limit on what each participant sends, kbit/s.
    pub max_bitrate_kbps: u32,
    /// The host's limit on participants.
    pub max_participants: u32,
}

/// The keys encrypting a call's media now: one per participant, derived
/// from the conversation's current MLS epoch, so they change when its
/// members do. Fetch them again when the conversation or the call changes.
#[derive(Clone, PartialEq, Eq)]
pub struct CallKeys {
    /// The slot to install them in (the epoch, modulo the key ring's size),
    /// so frames under the previous keys still decrypt during the change.
    pub key_index: u32,
    /// Each participant's key: (participant ID, hex; 32-byte key).
    pub keys: Vec<(String, Zeroizing<Vec<u8>>)>,
}

impl fmt::Debug for CallKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallKeys").field("key_index", &self.key_index).field("participants", &self.keys.len()).finish()
    }
}

/// The account on this device.
#[api]
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
#[api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationKind {
    /// A direct message between two people.
    Dm,
    /// A named group.
    Group,
}

/// A conversation as shown in a list.
#[api]
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
    #[api(unix_ms)]
    pub last_activity_ms: u64,
    /// False once this device was removed: still readable, no longer updated.
    pub following: bool,
    /// Messages from others since the user last read it.
    pub unread: u32,
    /// How many of those mention the user.
    pub mentions: u32,
    /// The newest message, for a preview.
    pub last_message: Option<Message>,
}

/// Delivery state of a message.
#[api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageState {
    /// Written locally, not yet accepted by the server.
    Pending,
    /// Accepted by the server: one tick.
    Sent,
    /// Your message reached every other member (one of each one's devices):
    /// two ticks. Members whose apps send no receipts are not waited for.
    Delivered,
    /// Every other member read your message: two blue ticks. Shown only
    /// while you send read receipts yourself, as in other messengers.
    Read,
    /// Received from someone else.
    Received,
    /// The server refused it.
    Failed,
}

/// A message as shown in a conversation.
#[api]
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
    #[api(unix_ms)]
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

/// Something the user was told, usually a security warning.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// When (Unix ms, this device's clock).
    #[api(unix_ms)]
    pub at_ms: u64,
    /// What happened, as a sentence starting in lower case.
    pub text: String,
    /// What it is about.
    pub kind: NoticeKind,
}

/// What a notice is about, for how it is shown.
#[api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// A security warning, or something else about the conversation.
    General,
    /// A call started or ended.
    Call,
    /// A call rang the user and none of their devices answered.
    MissedCall,
}

/// One of the server's announcements (design section 8, "Announcements").
/// Shown as from the server, whoever wrote it.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    /// Increasing; the server's own ID.
    pub id: u64,
    /// When it was posted (Unix ms, the server's clock).
    #[api(unix_ms)]
    pub at_ms: u64,
    /// What it is.
    pub kind: AnnouncementKind,
    /// A host's words for [`AnnouncementKind::Text`]; for the maintenance
    /// kinds, the reason given, if any.
    pub text: String,
    /// The maintenance kinds: when it starts or started (Unix ms). Show it
    /// in the reader's own time zone.
    #[api(unix_ms)]
    pub starts_at_ms: u64,
    /// The maintenance kinds: how long it should take.
    pub minutes: u32,
}

/// What an announcement is. All but [`AnnouncementKind::Text`] are posted
/// by the server itself, and say so.
#[api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnouncementKind {
    /// Written by a host.
    Text,
    /// Maintenance was planned.
    MaintenancePlanned,
    /// The planned maintenance starts soon: at `starts_at_ms`, which is
    /// how soon relative to `at_ms`.
    MaintenanceReminder,
    /// Maintenance has started.
    MaintenanceStarted,
    /// Maintenance is over.
    MaintenanceOver,
    /// The planned maintenance was cancelled.
    MaintenanceCancelled,
    /// A kind this version does not know, from a newer server.
    Unsupported,
}

impl AnnouncementKind {
    fn from_wire(kind: i32) -> Self {
        use post_proto::AnnouncementKind as K;
        match K::try_from(kind) {
            Ok(K::Text) => Self::Text,
            Ok(K::MaintenancePlanned) => Self::MaintenancePlanned,
            Ok(K::MaintenanceReminder) => Self::MaintenanceReminder,
            Ok(K::MaintenanceStarted) => Self::MaintenanceStarted,
            Ok(K::MaintenanceOver) => Self::MaintenanceOver,
            Ok(K::MaintenanceCancelled) => Self::MaintenanceCancelled,
            Ok(K::Unspecified) | Err(_) => Self::Unsupported,
        }
    }
}

/// Maintenance the server's hosts planned.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Maintenance {
    /// When it starts, or started (Unix ms).
    #[api(unix_ms)]
    pub starts_at_ms: u64,
    /// How long it should take.
    pub minutes: u32,
    /// Why, if a host said.
    pub reason: String,
    /// Started and not yet over.
    pub underway: bool,
}

/// A single-use invite code, made by a host.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// The code. Shown once: the server keeps only its hash.
    pub code: String,
    /// When it stops working (Unix ms).
    #[api(unix_ms)]
    pub expires_at_ms: u64,
    /// The address to sign up at with it: this account's server.
    pub server: String,
    /// It registers a bot account.
    pub bot: bool,
}

/// The announcements channel at a glance.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcements {
    /// The server they come from, as they are signed: its domain.
    pub server: String,
    /// Posted since the user last read them.
    pub unread: u32,
    /// The newest, for a preview.
    pub latest: Option<Announcement>,
    /// Whether this account is one of the server's hosts, who may announce
    /// and plan maintenance.
    pub host: bool,
    /// The maintenance planned or underway.
    pub maintenance: Option<Maintenance>,
}

fn announcement_from_row(r: store::AnnouncementRow) -> Announcement {
    Announcement {
        id: r.id,
        at_ms: r.created_at,
        kind: AnnouncementKind::from_wire(r.kind),
        text: r.text,
        starts_at_ms: r.starts_at,
        minutes: r.minutes,
    }
}

pub(crate) fn announcements_from(conn: &rusqlite::Connection) -> Result<Announcements> {
    let maintenance = match store::meta_get(conn, store::META_MAINTENANCE)? {
        Some(b) => {
            let m = <post_proto::Maintenance as prost::Message>::decode(b.as_slice()).map_err(|_| Error::Corrupt("maintenance"))?;
            Some(Maintenance {
                starts_at_ms: m.starts_at_ms,
                minutes: m.minutes,
                underway: m.state() == post_proto::MaintenanceState::Underway,
                reason: m.reason,
            })
        }
        None => None,
    };
    Ok(Announcements {
        server: store::meta_str(conn, "domain")?.unwrap_or_default(),
        unread: store::unread_announcements(conn)?,
        latest: store::latest_announcement(conn)?.map(announcement_from_row),
        host: store::meta_str(conn, store::META_HOST)?.as_deref() == Some("1"),
        maintenance,
    })
}

/// Who has one of your messages, person by person: what the ticks sum up.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageReceipts {
    /// Every other member now, by the name you see for them.
    pub people: Vec<PersonReceipt>,
    /// The conversation is too large for receipts: nobody sends any.
    pub too_large: bool,
    /// You have turned read receipts off, so you see deliveries only.
    pub read_receipts_off: bool,
}

/// How far one person has got with one of your messages.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonReceipt {
    /// Their username.
    pub username: String,
    /// The name you see for them here.
    pub shown_name: String,
    /// How far.
    pub state: ReceiptState,
}

/// How far one person has got with a message.
#[api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptState {
    /// Not on any of their devices yet (or their app sends no receipts).
    Waiting,
    /// On one of their devices.
    Delivered,
    /// Read.
    Read,
}

/// One version of a message.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageVersion {
    /// 0 is the original.
    pub version: u32,
    /// The text of this version.
    pub body: String,
    /// When it was written, by the sender's clock (Unix ms).
    #[api(unix_ms)]
    pub at_ms: u64,
    /// Not yet accepted by the server (own edits only).
    pub pending: bool,
}

/// What this device knows about a person.
#[api]
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
    /// You compared safety numbers with them.
    pub verified: bool,
    /// How their messages arrive on screen (an app animation preset);
    /// empty for the default.
    pub arrival: String,
    /// Their profile picture, for [`Core::picture`], once this device has
    /// it; until then (or with none), show the default.
    pub avatar: Option<String>,
    /// Their banner, for [`Core::picture`], once this device has it.
    pub banner: Option<String>,
    /// Their bio (about me), formatted like a message; empty if none.
    pub bio: String,
    /// Their banner's colour or gradient when they have no banner image, if
    /// they chose one.
    pub banner_fill: Option<Fill>,
    /// Their profile colours, if they chose them.
    pub theme: Option<Fill>,
    /// When their account began (Unix ms): signed into its device list and
    /// checked by their server when it registered.
    #[api(unix_ms)]
    pub since_ms: Option<u64>,
}

/// A device asking to join this account, as shown before approving it.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRequest {
    /// The new device's ID, hex.
    pub device_id: String,
    /// The name its user gave it; empty if none was valid.
    pub name: String,
}

/// What others see of someone (design section 7, "Presence").
#[api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Not seen for 25 minutes, signed off, or invisible.
    Offline,
    /// Using the app.
    Online,
    /// Set by hand, or idle for 10 minutes.
    Away,
    /// Not to be disturbed: their notifications are silenced.
    DoNotDisturb,
}

impl From<ephemeral::State> for Presence {
    fn from(s: ephemeral::State) -> Self {
        match s {
            ephemeral::State::Offline => Presence::Offline,
            ephemeral::State::Online => Presence::Online,
            ephemeral::State::Away => Presence::Away,
            ephemeral::State::DoNotDisturb => Presence::DoNotDisturb,
        }
    }
}

/// The presence you choose to show.
#[api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceSetting {
    /// Shown online.
    Online,
    /// Shown away.
    Away,
    /// Shown as not to be disturbed; your own notifications are silenced.
    DoNotDisturb,
    /// Shown offline. Nothing is sent, but you still receive everything.
    Invisible,
}

/// A member of a conversation.
#[api]
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
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Its ID, hex.
    pub id: String,
    /// The name it was given; empty if none.
    pub name: String,
    /// Whether it is the device this call runs on.
    pub this_device: bool,
}

/// What to search messages for: everything given must match. See
/// [`Core::search`].
#[api]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Words that must all appear, in any order, case or accents; each
    /// matches a word starting with it.
    #[api(default)]
    pub text: String,
    /// Only in this conversation.
    pub conversation: Option<ConversationId>,
    /// Only sent by this username.
    pub from: Option<String>,
    /// Only messages with a link.
    #[api(default)]
    pub has_link: bool,
    /// Only messages that mention this username.
    pub mentions: Option<String>,
    /// Only sent at or after this (Unix ms).
    #[api(unix_ms)]
    pub after_ms: Option<u64>,
    /// Only sent before this (Unix ms).
    #[api(unix_ms)]
    pub before_ms: Option<u64>,
    /// Oldest first, rather than newest.
    #[api(default)]
    pub oldest_first: bool,
    /// At most this many: the page size.
    pub limit: u32,
    /// Skipping this many first: where the page starts.
    #[api(default)]
    pub offset: u32,
}

/// What a search found.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResults {
    /// How many messages match in all.
    pub total: u32,
    /// The page asked for.
    pub messages: Vec<Message>,
}

/// A change to this account's profile. `None` leaves a field unchanged.
/// The banner's fill and the profile colours have their own setters,
/// [`Core::set_banner_fill`] and [`Core::set_theme`].
#[api]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileUpdate {
    /// New global display name; empty clears it.
    pub display_name: Option<String>,
    /// New typing template; empty restores the standard text.
    pub typing_template: Option<String>,
    /// New note; empty clears it.
    pub note: Option<String>,
    /// With a new note: keep it until changed instead of 24 hours.
    #[api(default)]
    pub note_keep: bool,
    /// New arrival preset for your messages; empty restores the default.
    pub arrival: Option<String>,
    /// New bio (about me); empty clears it.
    pub bio: Option<String>,
}

/// Something that happened, for live views.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A conversation was created or joined.
    ConversationAdded {
        /// The conversation.
        conversation: Conversation,
    },
    /// A conversation's details changed.
    ConversationChanged {
        /// The conversation, as it is now.
        conversation: Conversation,
    },
    /// A new message.
    MessageAdded {
        /// The message.
        message: Message,
    },
    /// A message was edited or its delivery state changed.
    MessageChanged {
        /// The message, as it is now.
        message: Message,
    },
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
    /// Someone is typing. Show `text` until another arrives, or for six
    /// seconds after the last one.
    Typing {
        /// Where.
        conversation: ConversationId,
        /// Who.
        username: String,
        /// Their typing template filled with the name you see for them.
        text: String,
    },
    /// Someone's presence changed.
    Presence {
        /// Whose.
        username: String,
        /// What it is now.
        presence: Presence,
    },
    /// Something the user should know about, usually a security warning.
    Notice {
        /// The conversation it concerns, if any.
        conversation: Option<ConversationId>,
        /// What happened.
        text: String,
    },
    /// A call started, rang, changed who is in it, or ended. Its timeline
    /// notices ("Alice started a call") may have changed too.
    CallChanged {
        /// The call, as it is now.
        call: Call,
    },
    /// The server posted an announcement.
    AnnouncementAdded {
        /// The announcement.
        announcement: Announcement,
    },
    /// The announcements channel changed: new ones, the read position,
    /// the maintenance plan, or whether this account is a host.
    AnnouncementsChanged {
        /// The channel, as it is now.
        summary: Announcements,
    },
    /// A subscriber fell behind and missed this many events: re-read what
    /// it shows. The core never sends this itself; the daemon and the app's
    /// bridge do, in place of what was missed.
    Lagged {
        /// How many.
        missed: u64,
    },
}

/// Settings live in the meta table under their own prefix, so they can
/// never touch the core's own entries. Keys are short: a-z, 0-9, `.`, `_`, `-`.
fn setting_key(key: &str) -> Result<String> {
    let ok = !key.is_empty() && key.len() <= 64 && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    if !ok {
        return Err(Error::InvalidInput(format!("not a setting name: {key}")));
    }
    Ok(format!("setting:{key}"))
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
    let groups = store::conversation_groups(conn, &c.group_id)?;
    let last_message = store::messages(conn, &groups, 1)?.first().map(|m| message_from_row(conn, m)).transpose()?;
    Ok(Conversation {
        unread: store::unread(conn, &groups)?,
        mentions: match store::meta_str(conn, "username")? {
            Some(me) => store::unread_mentions(conn, &groups, &me)?,
            None => 0,
        },
        last_message,
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
        state: match (m.state, m.receipt) {
            (store::STATE_PENDING, _) => MessageState::Pending,
            (store::STATE_SENT, store::RECEIPT_READ) if store::read_receipts_on(conn)? => MessageState::Read,
            (store::STATE_SENT, store::RECEIPT_READ | store::RECEIPT_DELIVERED) => MessageState::Delivered,
            (store::STATE_SENT, _) => MessageState::Sent,
            (store::STATE_FAILED, _) => MessageState::Failed,
            _ => MessageState::Received,
        },
    })
}

/// Make a profile readable by its owner only: the directory, so the database
/// and the journal files SQLite creates beside it are out of others' reach,
/// and the database file if it exists. The database is encrypted as well;
/// this keeps even the ciphertext private. Also fixes profiles made before.
fn owner_only(dir: &Path, db: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let set = |p: &Path, mode: u32| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
                .map_err(|e| Error::Storage(format!("restricting {}: {e}", p.display())))
        };
        set(dir, 0o700)?;
        if db.exists() {
            set(db, 0o600)?;
        } else {
            // Create it owner-only, rather than with the default mode.
            use std::os::unix::fs::OpenOptionsExt as _;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(db)
                .map_err(|e| Error::Storage(format!("creating {}: {e}", db.display())))?;
        }
    }
    #[cfg(not(unix))]
    let _ = (dir, db);
    Ok(())
}

/// The files [`Core::open`] makes in a profile directory, and the folders
/// kept beside them (`updates`: app updates being downloaded).
const PROFILE_FILES: [&str; 4] = ["profile.db", "profile.db-wal", "profile.db-shm", "profile.db-journal"];
const PROFILE_DIRS: [&str; 1] = ["updates"];

/// Delete a profile from this device, after logging out (or giving up an
/// account on its only device), once its `Core` is closed ([`Core::close`])
/// so nothing writes there again. (Unix deletes files still open; Windows
/// will need the `Core` dropped first.) Only what Post
/// keeps there is removed, and the directory only if nothing else is in it,
/// so a profile pointed at a folder holding other things leaves them alone.
pub fn delete_profile(dir: &Path) -> Result<()> {
    let storage = |p: &Path, e: std::io::Error| Error::Storage(format!("deleting {}: {e}", p.display()));
    for name in PROFILE_FILES {
        let p = dir.join(name);
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(storage(&p, e)),
        }
    }
    for name in PROFILE_DIRS {
        let p = dir.join(name);
        match std::fs::remove_dir_all(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(storage(&p, e)),
        }
    }
    match std::fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        // Something else lives there: leave it, and the folder.
        Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(e) => Err(storage(dir, e)),
    }
}

/// A client: one account on one device.
#[derive(Clone)]
pub struct Core {
    engine: Arc<tokio::sync::Mutex<Engine>>,
    reader: Arc<Mutex<Connection>>,
    events: broadcast::Sender<Event>,
    /// Set once, by [`Core::close`]: what runs for as long as the profile is
    /// open (`run`, an app's event stream) stops.
    closed: tokio::sync::watch::Sender<bool>,
}

impl Core {
    /// Open (creating if needed) the profile in `dir`, encrypted with `key`.
    pub fn open(dir: &Path, key: &DbKey) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| Error::Storage(format!("creating {}: {e}", dir.display())))?;
        let path = dir.join("profile.db");
        owner_only(dir, &path)?;
        let mut conn = store::open(&path, key)?;
        store::migrate(&mut conn)?;
        let reader = store::open(&path, key)?;
        let (events, _) = broadcast::channel(1024);
        let engine = Engine::new(conn, events.clone())?;
        Ok(Self {
            engine: Arc::new(tokio::sync::Mutex::new(engine)),
            reader: Arc::new(Mutex::new(reader)),
            events,
            closed: tokio::sync::watch::Sender::new(false),
        })
    }

    /// Close the profile for good: [`Core::run`] returns, both connections
    /// drop, and nothing connects again. Reading still works until the
    /// `Core` is dropped.
    pub async fn close(&self) {
        self.closed.send_replace(true);
        self.engine.lock().await.close();
    }

    /// Resolves once the profile is closed.
    pub async fn closed(&self) {
        let mut rx = self.closed.subscribe();
        // Only fails if the sender is gone, which is `self`.
        let _ = rx.wait_for(|c| *c).await;
    }

    /// Log out: this device removes itself from the account, then the
    /// profile closes. Refused on the account's only device, which would
    /// lose the account. Then delete the profile's files with
    /// [`delete_profile`].
    #[api(needs = connection)]
    pub async fn log_out(&self) -> Result<()> {
        self.engine.lock().await.log_out().await?;
        self.close().await;
        Ok(())
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
    #[api(needs = local)]
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
    #[api(needs = registration)]
    pub async fn register(
        &self,
        /// The server's URL, `wss://…`.
        server: &str,
        /// An invite code from one of the server's hosts.
        invite: &str,
        /// 3 to 32 of a-z, 0-9 and _, starting with a letter.
        username: &str,
        /// A bot account (the invite must be for one).
        #[api(default)]
        bot: bool,
    ) -> Result<Identity> {
        self.engine.lock().await.register(server, invite, username, bot).await?;
        self.identity().await.ok_or(Error::NotRegistered)
    }

    /// On a new device: offer it for linking to an existing account on
    /// `server`. Returns the code to enter on one of that account's devices;
    /// then call [`Core::finish_link`]. `bot` must match the account.
    #[api(needs = registration)]
    pub async fn begin_link(
        &self,
        /// The account's server, `wss://…`.
        server: &str,
        /// A name for this device, shown on the account's other devices.
        device_name: &str,
        /// The account is a bot.
        #[api(default)]
        bot: bool,
    ) -> Result<String> {
        Ok(self.engine.lock().await.begin_link(server, device_name, bot).await?.to_string())
    }

    /// On the new device: wait (up to ten minutes) for the code to be
    /// entered and approved, then log in and join the account's conversations.
    #[api(needs = registration)]
    pub async fn finish_link(&self) -> Result<Identity> {
        self.engine.lock().await.finish_link().await?;
        self.identity().await.ok_or(Error::NotRegistered)
    }

    /// On an existing device: which device a link code would add.
    #[api(needs = connection)]
    pub async fn link_inspect(
        &self,
        /// The code the new device shows.
        code: &str,
    ) -> Result<LinkRequest> {
        let offer = self.engine.lock().await.link_inspect(code).await?;
        Ok(LinkRequest { device_id: offer.entry.device_id().to_string(), name: offer.name })
    }

    /// On an existing device: add the device behind a link code to this
    /// account and to every conversation.
    #[api(needs = connection)]
    pub async fn link_approve(
        &self,
        /// The code the new device shows.
        code: &str,
    ) -> Result<LinkRequest> {
        let offer = self.engine.lock().await.link_approve(code).await?;
        Ok(LinkRequest { device_id: offer.entry.device_id().to_string(), name: offer.name })
    }

    /// This account's devices, oldest first.
    #[api(needs = local)]
    pub async fn devices(&self) -> Result<Vec<Device>> {
        let engine = self.engine.lock().await;
        let me = engine.me.as_ref().filter(|_| engine.registered).ok_or(Error::NotRegistered)?.device;
        Ok(engine.devices()?.into_iter().map(|(id, name)| Device { id: id.to_string(), name, this_device: id == me }).collect())
    }

    /// Name one of this account's devices, given its ID or an unambiguous
    /// prefix. The name is shared only with the account's own devices.
    #[api(needs = connection)]
    pub async fn rename_device(
        &self,
        /// The device's ID, or an unambiguous start of it.
        device: &str,
        /// Its new name.
        name: &str,
    ) -> Result<()> {
        let mut engine = self.engine.lock().await;
        let id = engine.resolve_device(device)?;
        engine.rename_device(id, name).await
    }

    /// Remove another of this account's devices, given its ID or an
    /// unambiguous prefix. It is cut off from the server and from every
    /// conversation; the account's contacts see that it was removed.
    #[api(needs = connection)]
    pub async fn remove_device(
        &self,
        /// The device's ID, or an unambiguous start of it.
        device: &str,
    ) -> Result<()> {
        let mut engine = self.engine.lock().await;
        let id = engine.resolve_device(device)?;
        engine.remove_device(id).await
    }

    /// Log in and catch up. Safe to call when already connected.
    pub async fn connect(&self) -> Result<()> {
        self.engine.lock().await.connect().await?;
        self.keep_pictures().await
    }

    /// Whether both server connections are up.
    #[api(needs = local)]
    pub async fn is_connected(&self) -> bool {
        self.engine.lock().await.is_connected()
    }

    /// Fetch and process everything new, and send what is queued.
    #[api(needs = connection)]
    pub async fn sync(&self) -> Result<()> {
        self.engine.lock().await.sync().await?;
        self.keep_pictures().await
    }

    /// After catching up: keep this account's pictures on the server, and
    /// fetch the pictures profiles name that are not here yet.
    async fn keep_pictures(&self) -> Result<()> {
        match self.engine.lock().await.renew_pictures().await {
            Ok(()) => {}
            Err(e @ (Error::NotConnected | Error::Network(_))) => return Err(e),
            Err(e) => tracing::warn!(error = %e, "renewing pictures failed"),
        }
        // Then pictures, a few at a time, letting other work in between: a
        // new device may have hundreds to fetch, and they never hold up a
        // message. A failure waits for the next sync.
        loop {
            match self.engine.lock().await.fetch_pictures().await {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(e @ (Error::NotConnected | Error::Network(_))) => return Err(e),
                Err(e) => {
                    tracing::warn!(error = %e, "fetching pictures failed");
                    return Ok(());
                }
            }
        }
    }

    /// Stay connected for as long as the returned future runs: connect, stay
    /// live, and reconnect with backoff (1 second doubling to a minute) when
    /// the connection drops. Waits quietly while there is no account yet.
    /// Cancel it by dropping or aborting the task running it.
    pub async fn run(&self) {
        tokio::select! {
            () = self.closed() => {}
            () = self.keep_connected() => {}
        }
    }

    async fn keep_connected(&self) {
        let mut delay = std::time::Duration::from_secs(1);
        loop {
            match self.connect().await {
                Ok(()) => {
                    delay = std::time::Duration::from_secs(1);
                    if let Err(e) = self.listen().await {
                        tracing::info!(error = %e, "connection lost");
                    }
                }
                Err(Error::NotRegistered) => {
                    // Nothing to connect to until an account exists.
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
                Err(e) => tracing::warn!(error = %e, retry_in = ?delay, "could not connect"),
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(std::time::Duration::from_secs(60));
        }
    }

    /// Stay live until a connection drops: process server wakes, relayed
    /// typing and presence, and repeat this account's presence every ten
    /// minutes. Returns [`Error::NotConnected`] when a connection drops; call
    /// [`Core::connect`] to resume.
    pub async fn listen(&self) -> Result<()> {
        const PRESENCE_EVERY: std::time::Duration = std::time::Duration::from_secs(600);
        self.engine.lock().await.announce_presence().await?;
        let mut next_presence = tokio::time::Instant::now() + PRESENCE_EVERY;
        loop {
            let engine::ListenHandles { wakes: [s_wake, a_wake], closed: [s_closed, a_closed], signals } =
                self.engine.lock().await.listen_handles()?;
            tokio::select! {
                () = s_wake.notified() => self.sync().await?,
                () = a_wake.notified() => self.sync().await?,
                () = signals.arrived.notified() => self.engine.lock().await.process_signals()?,
                () = tokio::time::sleep_until(next_presence) => {
                    self.engine.lock().await.announce_presence().await?;
                    next_presence = tokio::time::Instant::now() + PRESENCE_EVERY;
                }
                () = s_closed => return Err(Error::NotConnected),
                () = a_closed => return Err(Error::NotConnected),
            }
        }
    }

    /// Tell a conversation's online members you are typing. Call it as you
    /// type; it sends at most once every three seconds.
    #[api(needs = connection)]
    pub async fn send_typing(
        &self,
        /// Where you are typing.
        conversation: ConversationId,
    ) -> Result<()> {
        self.engine.lock().await.send_typing(conversation.0).await
    }

    /// Start a call in a conversation (design section 10), with this device
    /// in it. It rings `ring` (usernames); with nobody named, everyone in a
    /// DM or a group of up to eight, and nobody in a larger group, where the
    /// call shows to join. [`Event::CallChanged`] follows who joins.
    #[api(needs = connection)]
    pub async fn start_call(
        &self,
        /// Where.
        conversation: ConversationId,
        /// A video call, rather than voice only.
        #[api(default)]
        video: bool,
        /// Who to ring (usernames); none for the usual choice.
        #[api(default)]
        ring: &[String],
    ) -> Result<CallJoin> {
        self.engine.lock().await.start_call(conversation.0, video, ring).await
    }

    /// Ring people (usernames) into a live call: members not in it yet.
    #[api(needs = connection)]
    pub async fn ring_call(
        &self,
        /// The call's conversation.
        conversation: ConversationId,
        /// The call.
        call: CallId,
        /// Who to ring.
        usernames: &[String],
    ) -> Result<()> {
        self.engine.lock().await.ring_call(conversation.0, call.0, usernames).await
    }

    /// Join a live call from this device (answering it, if it is ringing).
    #[api(needs = connection)]
    pub async fn join_call(
        &self,
        /// The call's conversation.
        conversation: ConversationId,
        /// The call.
        call: CallId,
    ) -> Result<CallJoin> {
        self.engine.lock().await.join_call(conversation.0, call.0).await
    }

    /// Leave a call this device is in. In a DM, this ends the call.
    #[api(needs = connection)]
    pub async fn leave_call(
        &self,
        /// The call's conversation.
        conversation: ConversationId,
        /// The call.
        call: CallId,
    ) -> Result<()> {
        self.engine.lock().await.leave_call(conversation.0, call.0).await
    }

    /// Decline a ringing call: the user's other devices stop ringing, and in
    /// a DM the caller hears no.
    #[api(needs = connection)]
    pub async fn decline_call(
        &self,
        /// The call's conversation.
        conversation: ConversationId,
        /// The call.
        call: CallId,
    ) -> Result<()> {
        self.engine.lock().await.decline_call(conversation.0, call.0).await
    }

    /// The keys encrypting a call's media now. Fetch again on
    /// [`Event::CallChanged`] and [`Event::ConversationChanged`]: a new
    /// participant needs a key, and a membership change moves every key on.
    pub async fn call_keys(&self, conversation: ConversationId, call: CallId) -> Result<CallKeys> {
        self.engine.lock().await.call_keys(conversation.0, call.0)
    }

    /// The live calls, in every conversation.
    // A connection: calls live in memory, so a one-shot CLI must fetch first.
    #[api(needs = connection)]
    pub async fn calls(&self) -> Result<Vec<Call>> {
        self.engine.lock().await.calls()
    }

    /// Choose the presence others see. Kept on this device; announced now
    /// and every ten minutes while listening, except when invisible.
    #[api(needs = connection)]
    pub async fn set_presence(
        &self,
        /// What to show.
        setting: PresenceSetting,
    ) -> Result<()> {
        self.engine.lock().await.set_presence(setting).await
    }

    /// The presence you chose on this device.
    #[api(needs = local)]
    pub async fn own_presence(&self) -> Result<PresenceSetting> {
        self.engine.lock().await.own_presence()
    }

    /// What this device last heard of someone's presence.
    #[api(needs = local)]
    pub async fn presence(
        &self,
        /// Whose.
        username: &str,
    ) -> Result<Presence> {
        let mut engine = self.engine.lock().await;
        let account = engine.resolve(username).await?;
        Ok(engine.presence_of(&account))
    }

    /// Open a DM with `username`, creating it if needed.
    #[api(needs = connection)]
    pub async fn start_dm(
        &self,
        /// Who with.
        username: &str,
    ) -> Result<Conversation> {
        let id = self.engine.lock().await.start_dm(username).await?;
        self.conversation(id)
    }

    /// Create a named group with you as admin and these users as members.
    #[api(needs = connection)]
    pub async fn create_group(
        &self,
        /// The group's name.
        name: &str,
        /// Who else is in it.
        usernames: &[String],
    ) -> Result<Conversation> {
        let id = self.engine.lock().await.create_group(name, usernames).await?;
        self.conversation(id)
    }

    /// A conversation by ID.
    #[api(needs = local)]
    pub fn conversation(
        &self,
        /// Its ID.
        id: ConversationId,
    ) -> Result<Conversation> {
        self.read(|c| {
            let row = store::conversation(c, &id.0)?.filter(|r| r.kind != store::KIND_SELF).ok_or(Error::NotFound("conversation"))?;
            conversation_from_row(c, &row)
        })
    }

    /// As an admin: add people to a group, with all their devices.
    #[api(needs = connection)]
    pub async fn add_members(
        &self,
        /// The group.
        conversation: ConversationId,
        /// Who to add.
        usernames: &[String],
    ) -> Result<()> {
        self.engine.lock().await.add_members(conversation.0, usernames).await
    }

    /// As an admin: remove people from a group, with all their devices.
    #[api(needs = connection)]
    pub async fn remove_members(
        &self,
        /// The group.
        conversation: ConversationId,
        /// Who to remove.
        usernames: &[String],
    ) -> Result<()> {
        self.engine.lock().await.remove_members(conversation.0, usernames).await
    }

    /// As an admin: rename a group.
    #[api(needs = connection)]
    pub async fn rename_group(
        &self,
        /// The group.
        conversation: ConversationId,
        /// Its new name.
        name: &str,
    ) -> Result<()> {
        self.engine.lock().await.rename_group(conversation.0, name).await
    }

    /// As an admin: make a member an admin, or stop them being one. A group
    /// always keeps at least one admin.
    #[api(needs = connection)]
    pub async fn set_admin(
        &self,
        /// The group.
        conversation: ConversationId,
        /// The member.
        username: &str,
        /// True to make them an admin, false to remove the role.
        admin: bool,
    ) -> Result<()> {
        self.engine.lock().await.set_admin(conversation.0, username, admin).await
    }

    /// As an admin: whether new members are shown past messages, and how many
    /// days back (0 means all of them).
    #[api(needs = connection)]
    pub async fn set_history(
        &self,
        /// The group.
        conversation: ConversationId,
        /// New members see past messages.
        shared: bool,
        /// How many days back; 0 for all of them.
        days: u32,
    ) -> Result<()> {
        self.engine.lock().await.set_history(conversation.0, shared, days).await
    }

    /// Give this device fresh keys in every conversation now. This happens by
    /// itself weekly; doing it at once limits what a copy of the device's
    /// keys taken earlier can read from here on.
    #[api(needs = connection)]
    pub async fn refresh_keys(&self) -> Result<()> {
        self.engine.lock().await.refresh_keys().await
    }

    /// Leave a group, with all your devices. It shows as left at once; the
    /// other members' devices complete the removal.
    #[api(needs = connection)]
    pub async fn leave_group(
        &self,
        /// The group.
        conversation: ConversationId,
    ) -> Result<()> {
        self.engine.lock().await.leave_group(conversation.0).await
    }

    /// The members of a conversation, by the name you see for them.
    #[api(needs = local)]
    pub async fn members(
        &self,
        /// The conversation.
        conversation: ConversationId,
    ) -> Result<Vec<Member>> {
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
    #[api(needs = local)]
    pub fn conversations(&self) -> Result<Vec<Conversation>> {
        self.read(|c| store::conversations(c)?.iter().map(|r| conversation_from_row(c, r)).collect())
    }

    /// Mark a conversation read up to its newest message, on every one of
    /// this account's devices. Emits [`Event::ConversationChanged`] if its
    /// unread count changed.
    #[api(needs = connection)]
    pub async fn mark_read(
        &self,
        /// The conversation.
        conversation: ConversationId,
    ) -> Result<()> {
        self.engine.lock().await.mark_read(conversation.0).await
    }

    /// Whether you send read receipts: on unless turned off. Delivery
    /// receipts always go.
    #[api(needs = local)]
    pub fn read_receipts(&self) -> Result<bool> {
        self.read(store::read_receipts_on)
    }

    /// Turn read receipts on or off, on all your devices. Off, others do
    /// not see when you read their messages, and you do not see when they
    /// read yours.
    #[api(needs = connection)]
    pub async fn set_read_receipts(
        &self,
        /// Send them (the default), or not.
        on: bool,
    ) -> Result<()> {
        self.engine.lock().await.set_read_receipts(on).await
    }

    /// Record that you compared safety numbers with `username` (or, with
    /// `false`, withdraw it). Shared with your other devices only.
    #[api(needs = connection)]
    pub async fn set_verified(
        &self,
        /// Who with.
        username: &str,
        /// True to mark them verified, false to withdraw it.
        verified: bool,
    ) -> Result<()> {
        self.engine.lock().await.set_verified(username, verified).await
    }

    /// An app setting on this device, such as the spellcheck language. Kept
    /// in the encrypted profile, since some (a personal dictionary) say
    /// something about what the user writes.
    #[api(needs = local)]
    pub fn setting(
        &self,
        /// Its name: a-z, 0-9, `.`, `_` and `-`.
        key: &str,
    ) -> Result<Option<String>> {
        let k = setting_key(key)?;
        self.read(|c| store::meta_str(c, &k))
    }

    /// Set (or with `None` clear) an app setting on this device.
    #[api(needs = local)]
    pub async fn set_setting(
        &self,
        /// Its name: a-z, 0-9, `.`, `_` and `-`.
        key: &str,
        /// Its value; none clears it.
        value: Option<&str>,
    ) -> Result<()> {
        const MAX: usize = 256 * 1024;
        let k = setting_key(key)?;
        let engine = self.engine.lock().await;
        match value {
            Some(v) if v.len() > MAX => Err(Error::InvalidInput("setting is too long".into())),
            Some(v) => store::meta_put(&engine.conn, &k, v.as_bytes()),
            None => store::meta_delete(&engine.conn, &k),
        }
    }

    /// The newest `limit` notices about a conversation, or with `None` about
    /// the account as a whole, oldest first. Notices are what
    /// [`Event::Notice`] announced, kept.
    #[api(needs = local)]
    pub fn notices(
        &self,
        /// The conversation; none for the account's own notices.
        conversation: Option<ConversationId>,
        /// At most this many, the newest.
        #[api(default = 50)]
        limit: u32,
    ) -> Result<Vec<Notice>> {
        self.read(|c| {
            let groups = match conversation {
                Some(id) => {
                    let g = store::conversation_groups(c, &id.0)?;
                    if g.is_empty() {
                        return Err(Error::NotFound("conversation"));
                    }
                    g
                }
                None => Vec::new(),
            };
            Ok(store::notices(c, &groups, limit)?
                .into_iter()
                .map(|n| Notice {
                    at_ms: n.at as u64,
                    kind: match n.kind.as_str() {
                        "call" => NoticeKind::Call,
                        "missed_call" => NoticeKind::MissedCall,
                        _ => NoticeKind::General,
                    },
                    text: n.text,
                })
                .collect())
        })
    }

    /// The announcements channel at a glance: unread count, the newest,
    /// whether you are a host, and the maintenance plan.
    // A connection, so a one-shot CLI shows the server's newest.
    #[api(needs = connection)]
    pub fn announcement_summary(&self) -> Result<Announcements> {
        self.read(announcements_from)
    }

    /// The newest `limit` announcements, oldest first. They are kept for
    /// good: the server never deletes them, and neither does this device.
    #[api(needs = connection)]
    pub fn announcements(
        &self,
        /// At most this many, the newest.
        #[api(default = 50)]
        limit: u32,
    ) -> Result<Vec<Announcement>> {
        self.read(|c| Ok(store::announcements(c, limit)?.into_iter().map(announcement_from_row).collect()))
    }

    /// Mark every announcement read, on all your devices.
    #[api(needs = connection)]
    pub async fn mark_announcements_read(&self) -> Result<()> {
        self.engine.lock().await.mark_announcements_read().await
    }

    /// Post an announcement to everyone on the server. Hosts only; needs a
    /// connection, and fails rather than waiting for one.
    #[api(needs = connection)]
    pub async fn announce(
        &self,
        /// What to say.
        text: &str,
    ) -> Result<()> {
        self.engine.lock().await.post_announcement(text).await
    }

    /// Plan maintenance (hosts only): everyone is told now unless `quiet`,
    /// reminded a day, an hour, 30 minutes and 5 minutes before, and told
    /// when it starts and when it is over.
    #[api(needs = connection)]
    pub async fn plan_maintenance(
        &self,
        /// When it starts; none to start it now.
        #[api(unix_ms)]
        starts_at_ms: Option<u64>,
        /// How long it should take.
        minutes: u32,
        /// Why, shown to everyone; empty for no reason given.
        #[api(default)]
        reason: &str,
        /// Do not announce the plan now; the reminders still go out.
        #[api(default)]
        quiet: bool,
    ) -> Result<()> {
        self.engine.lock().await.plan_maintenance(starts_at_ms.unwrap_or(0), minutes, reason, quiet).await
    }

    /// Make a single-use invite code for someone to create an account with
    /// (hosts only), valid for `days` (1 to 30).
    #[api(needs = connection)]
    pub async fn create_invite(
        &self,
        /// For a bot account.
        #[api(default)]
        bot: bool,
        /// Days until it expires, 1 to 30.
        #[api(default = 7)]
        days: u32,
    ) -> Result<Invite> {
        let mut e = self.engine.lock().await;
        let (code, expires_at_ms) = e.create_invite(bot, days).await?;
        let server = e.me.as_ref().ok_or(Error::NotRegistered)?.url.clone();
        Ok(Invite { code, expires_at_ms, server, bot })
    }

    /// Cancel the planned maintenance, or announce the one underway is over
    /// (hosts only).
    #[api(needs = connection)]
    pub async fn end_maintenance(&self) -> Result<()> {
        self.engine.lock().await.end_maintenance().await
    }

    /// The newest `limit` messages of a conversation, oldest first.
    #[api(needs = local)]
    pub fn messages(
        &self,
        /// The conversation.
        conversation: ConversationId,
        /// At most this many, the newest.
        #[api(default = 50)]
        limit: u32,
    ) -> Result<Vec<Message>> {
        self.read(|c| {
            let groups = store::conversation_groups(c, &conversation.0)?;
            if groups.is_empty() {
                return Err(Error::NotFound("conversation"));
            }
            store::messages(c, &groups, limit)?.iter().map(|m| message_from_row(c, m)).collect()
        })
    }

    /// Messages matching `query`, from this device's search index: nothing
    /// is asked of the server. Each word matches a word starting with it,
    /// ignoring case and accents ("ecole" finds "École"). Deleted messages
    /// are never found, and an edited one is found by its latest text.
    /// Gives how many match in all, and the page asked for.
    #[api(needs = local)]
    pub fn search(
        &self,
        /// What to find.
        query: &SearchQuery,
    ) -> Result<SearchResults> {
        let words: Vec<String> = query.text.split_whitespace().map(str::to_owned).collect();
        let nothing = SearchResults { total: 0, messages: Vec::new() };
        if words.is_empty()
            && query.from.is_none()
            && query.mentions.is_none()
            && !query.has_link
            && query.after_ms.is_none()
            && query.before_ms.is_none()
        {
            return Ok(nothing);
        }
        self.read(|c| {
            let groups = match query.conversation {
                Some(id) => Some(store::conversation_groups(c, &id.0)?),
                None => None,
            };
            let sender = match &query.from {
                // Someone this device has never heard of sent nothing here.
                Some(u) => match store::account_by_username(c, u)? {
                    Some((account, _)) => Some(account),
                    None => return Ok(nothing),
                },
                None => None,
            };
            let filter = store::SearchFilter {
                words,
                sender: sender.as_ref().map(|a| a.0.as_slice()),
                groups: groups.as_deref(),
                after: query.after_ms.map(store::i64_of),
                before: query.before_ms.map(store::i64_of),
                has_link: query.has_link,
                mentions: query.mentions.as_deref(),
                oldest_first: query.oldest_first,
            };
            let (total, rows) = store::search(c, &filter, query.limit, query.offset)?;
            Ok(SearchResults { total, messages: rows.iter().map(|m| message_from_row(c, m)).collect::<Result<_>>()? })
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
    #[api(needs = local)]
    pub fn message(
        &self,
        /// Its conversation.
        conversation: ConversationId,
        /// The message.
        id: MessageId,
    ) -> Result<Message> {
        self.read(|c| message_from_row(c, &Self::find_message(c, conversation, id)?))
    }

    /// Who has one of your messages: each other member, and whether it is on
    /// one of their devices or read. Read shows only while you send read
    /// receipts yourself.
    #[api(needs = local)]
    pub async fn message_receipts(
        &self,
        /// Its conversation.
        conversation: ConversationId,
        /// The message: one of yours.
        id: MessageId,
    ) -> Result<MessageReceipts> {
        let me = self.engine.lock().await.me.as_ref().ok_or(Error::NotRegistered)?.account;
        self.read(|c| {
            let m = Self::find_message(c, conversation, id)?;
            if !m.outgoing {
                return Err(Error::NotAllowed("receipts are for your own messages"));
            }
            let seq = store::message_seq(c, &m.group_id, &m.id)?;
            let read_on = store::read_receipts_on(c)?;
            let (_, members) = engine::group_members(c, &m.group_id)?;
            let mut people = Vec::new();
            for (account, _) in members.iter().filter(|(a, _)| *a != me) {
                let (delivered, read) = store::receipt(c, &m.group_id, &account.0)?;
                let state = match seq {
                    Some(s) if read_on && read >= s => ReceiptState::Read,
                    Some(s) if delivered >= s => ReceiptState::Delivered,
                    _ => ReceiptState::Waiting,
                };
                people.push(PersonReceipt {
                    username: store::username_of(c, &account.0)?.unwrap_or_default(),
                    shown_name: store::shown_name(c, Some(&m.group_id), &account.0)?,
                    state,
                });
            }
            people.sort_by_key(|p| p.shown_name.to_lowercase());
            Ok(MessageReceipts { too_large: members.len() > engine::MAX_RECEIPT_ACCOUNTS, read_receipts_off: !read_on, people })
        })
    }

    /// Every version of a message, the original first.
    #[api(needs = local)]
    pub fn message_versions(
        &self,
        /// Its conversation.
        conversation: ConversationId,
        /// The message.
        id: MessageId,
    ) -> Result<Vec<MessageVersion>> {
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
    #[api(needs = connection)]
    pub async fn send_text(
        &self,
        /// Where.
        conversation: ConversationId,
        /// The text.
        text: &str,
    ) -> Result<Message> {
        let body = content::Body::Text(content::Text { body: text.into() });
        let id = self.engine.lock().await.send_body(conversation.0, body).await?;
        self.message(conversation, id)
    }

    /// Edit one of your own messages. Every earlier version is kept.
    #[api(needs = connection)]
    pub async fn edit_message(
        &self,
        /// Its conversation.
        conversation: ConversationId,
        /// The message: one of yours.
        id: MessageId,
        /// The new text.
        text: &str,
    ) -> Result<Message> {
        let group = self.read(|c| Ok(Self::find_message(c, conversation, id)?.group_id))?;
        let gid = group.as_slice().try_into().map_err(|_| Error::Corrupt("group id"))?;
        let body = content::Body::Edit(content::Edit { target: id.0.to_vec(), body: text.into() });
        self.engine.lock().await.send_body(gid, body).await?;
        self.message(conversation, id)
    }

    /// Delete one of your own messages, with all its versions, for everyone.
    #[api(needs = connection)]
    pub async fn delete_message(
        &self,
        /// Its conversation.
        conversation: ConversationId,
        /// The message: one of yours.
        id: MessageId,
    ) -> Result<()> {
        let group = self.read(|c| Ok(Self::find_message(c, conversation, id)?.group_id))?;
        let gid = group.as_slice().try_into().map_err(|_| Error::Corrupt("group id"))?;
        self.engine.lock().await.send_body(gid, content::Body::Delete(content::Delete { target: id.0.to_vec() })).await?;
        Ok(())
    }

    /// Change your profile from what it is now, and share it with every
    /// conversation. The engine checks every field of the result.
    async fn change_profile(&self, change: impl FnOnce(store::ProfileRow) -> store::ProfileRow) -> Result<()> {
        let mut engine = self.engine.lock().await;
        let mine = engine.me.as_ref().ok_or(Error::NotRegistered)?.account;
        let current = store::profile(&engine.conn, &mine.0)?.unwrap_or_default();
        engine.set_profile(store::ProfileRow { version: 0, ..change(current) }).await
    }

    /// Change your profile and share it with every conversation.
    #[api(needs = connection)]
    pub async fn set_profile(
        &self,
        /// What to change; what is left out stays as it is.
        update: ProfileUpdate,
    ) -> Result<()> {
        let now = store::now_ms();
        self.change_profile(|current| {
            let (note_text, note_set_at, note_expires_at) = match update.note {
                Some(n) if n.is_empty() => (String::new(), 0, 0),
                Some(n) => (n, now, if update.note_keep { 0 } else { now + content::NOTE_DEFAULT_MS }),
                None => (current.note_text.clone(), current.note_set_at, current.note_expires_at),
            };
            store::ProfileRow {
                display_name: update.display_name.unwrap_or_else(|| current.display_name.clone()),
                typing_template: update.typing_template.unwrap_or_else(|| current.typing_template.clone()),
                note_text,
                note_set_at,
                note_expires_at,
                arrival: update.arrival.unwrap_or_else(|| current.arrival.clone()),
                bio: update.bio.unwrap_or_else(|| current.bio.clone()),
                ..current
            }
        })
        .await
    }

    /// Set your banner's colour or gradient, shown when you have no banner
    /// image, or clear it for the app's choice.
    #[api(needs = connection)]
    pub async fn set_banner_fill(
        &self,
        /// The fill; none for the app's choice.
        fill: Option<Fill>,
    ) -> Result<()> {
        self.change_profile(|current| store::ProfileRow { banner_fill: fill, ..current }).await
    }

    /// Set your profile colours, or clear them for the app's own look.
    #[api(needs = connection)]
    pub async fn set_theme(
        &self,
        /// The fill; none for the app's own look.
        theme: Option<Fill>,
    ) -> Result<()> {
        self.change_profile(|current| store::ProfileRow { theme, ..current }).await
    }

    /// Set your profile picture or banner from an image file on this
    /// computer (PNG, JPEG, WebP or GIF; its middle is used, in shape), or
    /// remove it. As [`Core::set_picture`], for callers without the bytes
    /// in hand, such as bots.
    #[api(needs = connection)]
    pub async fn set_picture_file(
        &self,
        /// Which picture.
        kind: picture::PictureKind,
        /// The image file; none removes the picture.
        path: Option<&str>,
    ) -> Result<()> {
        let source = match path {
            Some(path) => {
                let size = std::fs::metadata(path).map_err(|e| Error::InvalidInput(format!("reading {path}: {e}")))?.len();
                if size > picture::MAX_SOURCE_BYTES as u64 {
                    return Err(Error::InvalidInput(format!("{path} is over {} MB", picture::MAX_SOURCE_BYTES / 1024 / 1024)));
                }
                Some((std::fs::read(path).map_err(|e| Error::InvalidInput(format!("reading {path}: {e}")))?, None))
            }
            None => None,
        };
        self.set_picture(kind, source).await
    }

    /// Write a picture this device has to a new file, by the ID a
    /// [`Person`] gives for it.
    #[api(needs = local)]
    pub fn save_picture_file(
        &self,
        /// The picture's ID, from a person's `avatar` or `banner`.
        id: &str,
        /// Where to write it. It must not exist yet.
        path: &str,
    ) -> Result<()> {
        let bytes = self.picture(id)?.ok_or(Error::NotFound("picture"))?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, &bytes))
            .map_err(|e| Error::InvalidInput(format!("writing {path}: {e}")))
    }

    /// Set your profile picture or banner from an image file's bytes,
    /// cropped to `crop` (else its middle), or remove it (`None`). The
    /// image is made into a picture within the limits here, encrypted,
    /// uploaded, and shared like the rest of your profile. Needs a
    /// connection.
    pub async fn set_picture(&self, kind: picture::PictureKind, source: Option<(Vec<u8>, Option<picture::Crop>)>) -> Result<()> {
        // Decoding and shrinking a large photo takes a moment: not on the
        // engine, which would hold up messages meanwhile.
        let prepared = match source {
            Some((bytes, crop)) => Some(
                tokio::task::spawn_blocking(move || picture::prepare(kind, &bytes, crop))
                    .await
                    .map_err(|_| Error::Picture("preparing the picture failed".into()))??,
            ),
            None => None,
        };
        self.engine.lock().await.set_picture(kind, prepared).await
    }

    /// A picture (profile picture or banner) this device has fetched, by
    /// the ID a [`Person`] gives for it.
    pub fn picture(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let Ok(sha) = hex::decode(id) else { return Ok(None) };
        self.read(|c| store::picture(c, &sha))
    }

    /// A picture this device has, ready to draw at exactly `width`×`height`
    /// pixels (see [`picture::scaled`]): RGBA, row by row.
    pub async fn picture_scaled(&self, id: &str, width: u32, height: u32) -> Result<Option<Vec<u8>>> {
        let Some(bytes) = self.picture(id)? else { return Ok(None) };
        tokio::task::spawn_blocking(move || picture::scaled(&bytes, width, height))
            .await
            .map_err(|_| Error::Picture("scaling a picture failed".into()))?
            .map(Some)
    }

    /// Set (or clear, with an empty name) your display name in one conversation.
    #[api(needs = connection)]
    pub async fn set_conversation_name(
        &self,
        /// The conversation.
        conversation: ConversationId,
        /// The name; empty clears it.
        name: &str,
    ) -> Result<()> {
        let body = content::Body::ConversationName(content::ConversationName { name: name.into() });
        self.engine.lock().await.send_body(conversation.0, body).await?;
        Ok(())
    }

    /// Set (or clear, with an empty name) a private nickname for someone.
    #[api(needs = local)]
    pub async fn set_nickname(
        &self,
        /// Whose.
        username: &str,
        /// The nickname; empty clears it.
        nickname: &str,
    ) -> Result<()> {
        self.engine.lock().await.set_nickname(username, nickname).await
    }

    /// What this device knows about a person.
    #[api(needs = local)]
    pub async fn person(
        &self,
        /// Who.
        username: &str,
    ) -> Result<Person> {
        let (account, since_ms) = {
            let mut engine = self.engine.lock().await;
            let account = engine.resolve(username).await?;
            (account, engine.account_created_ms(&account))
        };
        self.read(|c| {
            let p = store::profile(c, &account.0)?;
            let now = store::now_ms();
            // A picture shows once it is here; until then, the default.
            let held = |kind| -> Result<Option<String>> {
                match p.as_ref().and_then(|p| p.image(kind)) {
                    Some(i) if store::picture_expiry(c, &i.sha256)?.is_some() => Ok(Some(hex::encode(&i.sha256))),
                    _ => Ok(None),
                }
            };
            Ok(Person {
                username: username.into(),
                shown_name: store::shown_name(c, None, &account.0)?,
                display_name: p.as_ref().map(|p| p.display_name.clone()).unwrap_or_default(),
                nickname: store::nickname(c, &account.0)?.unwrap_or_default(),
                typing_template: p.as_ref().map(|p| p.typing_template.clone()).unwrap_or_default(),
                verified: store::is_verified(c, &account.0)?,
                arrival: p.as_ref().map(|p| p.arrival.clone()).unwrap_or_default(),
                avatar: held(picture::PictureKind::Avatar)?,
                banner: held(picture::PictureKind::Banner)?,
                bio: p.as_ref().map(|p| p.bio.clone()).unwrap_or_default(),
                banner_fill: p.as_ref().and_then(|p| p.banner_fill.clone()),
                theme: p.as_ref().and_then(|p| p.theme.clone()),
                since_ms,
                note: p.filter(|p| !p.note_text.is_empty() && (p.note_expires_at == 0 || p.note_expires_at > now)).map(|p| p.note_text),
            })
        })
    }

    /// The safety number between you and `username`, as 12 groups of 5 digits.
    #[api(needs = local)]
    pub async fn safety_number(
        &self,
        /// Who with.
        username: &str,
    ) -> Result<String> {
        let mut engine = self.engine.lock().await;
        let peer = engine.resolve(username).await?;
        let mine = engine.me.as_ref().ok_or(Error::NotRegistered)?.account;
        Ok(post_identity::safety_number(&mine, &peer).to_string())
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
    send(c.run());
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
    send(c.send_typing(conv));
    send(c.set_presence(PresenceSetting::Away));
    send(c.own_presence());
    send(c.presence(""));
    send(c.send_text(conv, ""));
    send(c.mark_read(conv));
    send(c.set_verified("", true));
    send(c.set_setting("", None));
    send(c.edit_message(conv, msg, ""));
    send(c.delete_message(conv, msg));
    send(c.set_profile(ProfileUpdate::default()));
    send(c.set_banner_fill(None));
    send(c.set_theme(None));
    send(c.set_picture_file(picture::PictureKind::Avatar, None));
    send(c.plan_maintenance(None, 0, "", false));
    send(c.set_conversation_name(conv, ""));
    send(c.set_nickname("", ""));
    send(c.person(""));
    send(c.safety_number(""));
}
