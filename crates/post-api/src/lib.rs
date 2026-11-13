//! Post's local API (design section 11, "Command-line client and local
//! API").
//!
//! One set of named methods with typed parameters and results. The CLI maps
//! each subcommand to one method; the daemon serves the same methods as
//! JSON-RPC 2.0. Both go through [`call`], so they cannot drift apart.
//!
//! Versioned as `post.v1`: within a version, changes are additive only.
//! Parameters reject unknown fields, so a typo fails instead of being ignored.

use post_core::{self as core, ConversationId, Core, MessageId};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The API version.
pub const VERSION: &str = "post.v1";

// ---------------------------------------------------------------- result types

/// The account on this device.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Identity {
    /// Username.
    pub username: String,
    /// Account ID, hex. The safety number is derived from it.
    pub account_id: String,
    /// This device's ID, hex.
    pub device_id: String,
    /// Server URL.
    pub server: String,
    /// Whether this is a bot account.
    pub bot: bool,
}

/// A conversation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Conversation {
    /// Conversation ID, hex.
    pub id: String,
    /// `dm` or `group`.
    pub kind: String,
    /// What to call it.
    pub title: String,
    /// The other person's username, for a DM.
    pub peer: Option<String>,
    /// Unix milliseconds of the latest activity.
    pub last_activity_ms: u64,
    /// False once this device was removed from it.
    pub following: bool,
    /// Messages from others since you last marked it read.
    pub unread: u32,
    /// How many of those mention you.
    pub mentions: u32,
}

/// A message.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Message {
    /// Message ID, hex.
    pub id: String,
    /// Conversation ID, hex.
    pub conversation: String,
    /// The name you see for the sender in this conversation.
    pub sender: String,
    /// The sender's username.
    pub sender_username: String,
    /// Sent by you.
    pub outgoing: bool,
    /// When the sender sent it, by their clock (Unix ms).
    pub sent_at_ms: u64,
    /// Latest text; null if deleted or of an unsupported type.
    pub body: Option<String>,
    /// Deleted by its sender.
    pub deleted: bool,
    /// A type this client does not understand.
    pub unsupported: bool,
    /// Edited at least once.
    pub edited: bool,
    /// Number of versions, the original included.
    pub versions: u32,
    /// Yours: `pending`, `sent` (the server has it), `delivered` (everyone
    /// else has it), `read` (everyone else read it) or `failed`. Others':
    /// `received`.
    pub state: String,
}

/// One version of a message.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct MessageVersion {
    /// 0 is the original.
    pub version: u32,
    /// Text of this version.
    pub body: String,
    /// When it was written (Unix ms, sender's clock).
    pub at_ms: u64,
    /// Not yet accepted by the server.
    pub pending: bool,
}

/// What you know about a person.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Person {
    /// Username.
    pub username: String,
    /// The name shown for them.
    pub shown_name: String,
    /// The display name they chose.
    pub display_name: String,
    /// Your private nickname for them.
    pub nickname: String,
    /// Their typing template.
    pub typing_template: String,
    /// Their current note, if any.
    pub note: Option<String>,
    /// You compared safety numbers with them.
    pub verified: bool,
    /// How their messages arrive on screen in the app (a preset's name);
    /// empty for the default.
    pub arrival: String,
    /// Their profile picture's ID, for `save_picture`, once this device
    /// has it.
    pub picture: Option<String>,
    /// Their banner's ID, for `save_picture`, once this device has it.
    pub banner: Option<String>,
    /// Their banner colour, `#rrggbb`, if they chose one.
    pub banner_color: Option<String>,
}

/// A safety number.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SafetyNumber {
    /// 12 groups of 5 digits.
    pub safety_number: String,
}

/// A member of a conversation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Member {
    /// Their username.
    pub username: String,
    /// The name you see for them here.
    pub shown_name: String,
    /// They may add and remove people and change the group's details.
    pub admin: bool,
    /// How many of their devices are in the conversation.
    pub devices: u32,
    /// It is you.
    pub you: bool,
}

/// A link code to enter on another device.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LinkCode {
    /// 25 characters in groups of five. Case, spaces and dashes do not matter.
    pub code: String,
}

/// A device being linked.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LinkRequest {
    /// The new device's ID, hex.
    pub device_id: String,
    /// The name it was given; empty if none.
    pub name: String,
}

/// One of your devices.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Device {
    /// Device ID, hex.
    pub id: String,
    /// Its name; empty if none.
    pub name: String,
    /// The device answering this call.
    pub this_device: bool,
}

/// An empty result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Done {}

/// A live event, sent by the daemon as a JSON-RPC notification named `event`
/// after `subscribe`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A conversation was created or joined.
    ConversationAdded {
        /// The conversation.
        conversation: Conversation,
    },
    /// A conversation's details changed.
    ConversationChanged {
        /// The conversation.
        conversation: Conversation,
    },
    /// A new message.
    MessageAdded {
        /// The message.
        message: Message,
    },
    /// A message was edited or its delivery state changed.
    MessageChanged {
        /// The message.
        message: Message,
    },
    /// A message was deleted.
    MessageDeleted {
        /// Conversation ID, hex.
        conversation: String,
        /// Message ID, hex.
        message: String,
    },
    /// Someone's profile, or the name you see for them, changed.
    ProfileChanged {
        /// Their username.
        username: String,
    },
    /// An account added or removed devices. If it is your own account and the
    /// change was not yours, remove the device from another of your devices.
    DevicesChanged {
        /// Whose devices, possibly yours.
        username: String,
        /// Devices added.
        added: u32,
        /// Devices removed.
        removed: u32,
    },
    /// Someone is typing. Show `text` until another arrives, or for six
    /// seconds after the last one.
    Typing {
        /// Conversation ID, hex.
        conversation: String,
        /// Who.
        username: String,
        /// Their typing template filled with the name you see for them.
        text: String,
    },
    /// Someone's presence changed.
    Presence {
        /// Whose.
        username: String,
        /// `online`, `away`, `dnd` or `offline`.
        presence: String,
    },
    /// Something you should know, usually a security warning.
    Notice {
        /// Conversation ID, if it concerns one.
        conversation: Option<String>,
        /// What happened.
        text: String,
    },
    /// A call started, rang, changed who is in it, or ended. The CLI does not
    /// join calls; this says they happen.
    Call {
        /// Conversation ID.
        conversation: String,
        /// Call ID.
        call: String,
        /// Who started it.
        started_by: String,
        /// Usernames in it now.
        participants: Vec<String>,
        /// It is calling you.
        ringing: bool,
        /// It is over.
        ended: bool,
    },
    /// The server posted an announcement.
    AnnouncementAdded {
        /// The announcement.
        announcement: Announcement,
    },
    /// The announcements changed: new ones, what you have read, the
    /// maintenance plan, or whether you are a host.
    AnnouncementsChanged {
        /// How they stand now.
        summary: AnnouncementSummary,
    },
}

/// One of the server's announcements, from the server whoever wrote it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Announcement {
    /// Increasing ID.
    pub id: u64,
    /// When it was posted, Unix ms.
    pub at_ms: u64,
    /// `text` (a host wrote it), or one the server posts itself:
    /// `maintenance_planned`, `maintenance_reminder`, `maintenance_started`,
    /// `maintenance_over`, `maintenance_cancelled`. `unsupported` for a kind
    /// this version does not know.
    pub kind: String,
    /// The host's words, or for maintenance the reason given (may be empty).
    pub text: String,
    /// Maintenance: when it starts or started, Unix ms.
    pub starts_at_ms: Option<u64>,
    /// Maintenance: how long it should take.
    pub minutes: Option<u32>,
}

/// Maintenance planned or underway.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct MaintenanceInfo {
    /// When it starts or started, Unix ms.
    pub starts_at_ms: u64,
    /// How long it should take.
    pub minutes: u32,
    /// Why, if given.
    pub reason: String,
    /// It has started.
    pub underway: bool,
}

/// The announcements at a glance.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AnnouncementSummary {
    /// The server's domain, which signs them.
    pub server: String,
    /// Posted since you last read them.
    pub unread: u32,
    /// The newest.
    pub latest: Option<Announcement>,
    /// You are one of the server's hosts: you may announce and plan
    /// maintenance.
    pub host: bool,
    /// Maintenance planned or underway.
    pub maintenance: Option<MaintenanceInfo>,
}

fn announcement(a: core::Announcement) -> Announcement {
    use core::AnnouncementKind as K;
    let kind = match a.kind {
        K::Text => "text",
        K::MaintenancePlanned => "maintenance_planned",
        K::MaintenanceReminder => "maintenance_reminder",
        K::MaintenanceStarted => "maintenance_started",
        K::MaintenanceOver => "maintenance_over",
        K::MaintenanceCancelled => "maintenance_cancelled",
        K::Unsupported => "unsupported",
    };
    let maintenance = !matches!(a.kind, K::Text | K::Unsupported);
    Announcement {
        id: a.id,
        at_ms: a.at_ms,
        kind: kind.into(),
        text: a.text,
        starts_at_ms: maintenance.then_some(a.starts_at_ms),
        minutes: maintenance.then_some(a.minutes),
    }
}

fn announcement_summary(s: core::Announcements) -> AnnouncementSummary {
    AnnouncementSummary {
        server: s.server,
        unread: s.unread,
        latest: s.latest.map(announcement),
        host: s.host,
        maintenance: s.maintenance.map(|m| MaintenanceInfo {
            starts_at_ms: m.starts_at_ms,
            minutes: m.minutes,
            reason: m.reason,
            underway: m.underway,
        }),
    }
}

fn conversation(c: core::Conversation) -> Conversation {
    Conversation {
        id: c.id.to_string(),
        kind: match c.kind {
            core::ConversationKind::Dm => "dm",
            core::ConversationKind::Group => "group",
        }
        .into(),
        title: c.title,
        peer: c.peer,
        last_activity_ms: c.last_activity_ms,
        following: c.following,
        unread: c.unread,
        mentions: c.mentions,
    }
}

fn message(m: core::Message) -> Message {
    Message {
        id: m.id.to_string(),
        conversation: m.conversation.to_string(),
        sender: m.sender,
        sender_username: m.sender_username,
        outgoing: m.outgoing,
        sent_at_ms: m.sent_at_ms,
        body: m.body,
        deleted: m.deleted,
        unsupported: m.unsupported,
        edited: m.edited,
        versions: m.versions,
        state: match m.state {
            core::MessageState::Pending => "pending",
            core::MessageState::Sent => "sent",
            core::MessageState::Delivered => "delivered",
            core::MessageState::Read => "read",
            core::MessageState::Received => "received",
            core::MessageState::Failed => "failed",
        }
        .into(),
    }
}

fn identity(i: core::Identity) -> Identity {
    Identity { username: i.username, account_id: i.account_id, device_id: i.device_id, server: i.server, bot: i.bot }
}

fn presence_word(p: core::Presence) -> &'static str {
    match p {
        core::Presence::Offline => "offline",
        core::Presence::Online => "online",
        core::Presence::Away => "away",
        core::Presence::DoNotDisturb => "dnd",
    }
}

/// A live call (design section 10). The CLI and daemon carry calls'
/// signalling only, not their audio.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CallInfo {
    /// Call ID, hex.
    pub id: String,
    /// Conversation ID, hex.
    pub conversation: String,
    /// Who started it.
    pub started_by: String,
    /// Usernames in it now.
    pub participants: Vec<String>,
    /// It is calling you.
    pub ringing: bool,
    /// This device is in it.
    pub joined: bool,
}

/// What joining a call's room takes, for a program that brings its own
/// audio.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CallJoined {
    /// The call.
    pub call: CallInfo,
    /// This device's identity in the room (hex).
    pub participant_id: String,
    /// The LiveKit server's URL.
    pub url: String,
    /// A short-lived LiveKit access token.
    pub token: String,
}

fn call_info(c: core::Call) -> CallInfo {
    CallInfo {
        id: c.id.to_string(),
        conversation: c.conversation.to_string(),
        started_by: c.started_by,
        participants: c.participants.into_iter().map(|p| p.username).collect(),
        ringing: c.ringing,
        joined: c.joined,
    }
}

fn call_joined(j: core::CallJoin) -> CallJoined {
    CallJoined { call: call_info(j.call), participant_id: j.participant_id, url: j.url, token: j.token }
}

/// `call_start`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallStartParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// Start as a video call.
    #[serde(default)]
    pub video: bool,
    /// Ring only these usernames. None: everyone in a DM or small group.
    #[serde(default)]
    pub ring: Vec<String>,
}

/// `call_ring`: ring people into a live call.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallRingParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// Call ID, hex.
    pub call: String,
    /// Whom to ring.
    pub usernames: Vec<String>,
}

/// A call in a conversation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// Call ID, hex.
    pub call: String,
}

/// Someone's presence, as last heard.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PresenceResult {
    /// `online`, `away`, `dnd` or `offline`.
    pub presence: String,
}

/// Convert a core event for the API.
pub fn event(e: core::Event) -> Event {
    match e {
        core::Event::ConversationAdded(c) => Event::ConversationAdded { conversation: conversation(c) },
        core::Event::ConversationChanged(c) => Event::ConversationChanged { conversation: conversation(c) },
        core::Event::MessageAdded(m) => Event::MessageAdded { message: message(m) },
        core::Event::MessageChanged(m) => Event::MessageChanged { message: message(m) },
        core::Event::MessageDeleted { conversation, id } => {
            Event::MessageDeleted { conversation: conversation.to_string(), message: id.to_string() }
        }
        core::Event::ProfileChanged { username } => Event::ProfileChanged { username },
        core::Event::DevicesChanged { username, added, removed } => Event::DevicesChanged { username, added, removed },
        core::Event::Typing { conversation, username, text } => Event::Typing { conversation: conversation.to_string(), username, text },
        core::Event::Presence { username, presence } => Event::Presence { username, presence: presence_word(presence).into() },
        core::Event::Notice { conversation, text } => Event::Notice { conversation: conversation.map(|c| c.to_string()), text },
        core::Event::CallChanged(c) => Event::Call {
            conversation: c.conversation.to_string(),
            call: c.id.to_string(),
            started_by: c.started_by,
            participants: c.participants.into_iter().map(|p| p.username).collect(),
            ringing: c.ringing,
            ended: c.ended,
        },
        core::Event::AnnouncementAdded(a) => Event::AnnouncementAdded { announcement: announcement(a) },
        core::Event::AnnouncementsChanged(s) => Event::AnnouncementsChanged { summary: announcement_summary(s) },
    }
}

// ---------------------------------------------------------------- parameters

/// `register`: create an account with an invite.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterParams {
    /// Server URL, `wss://...`.
    pub server: String,
    /// Invite code from the server admin.
    pub invite: String,
    /// Username: 3 to 32 of a-z, 0-9 and _, starting with a letter.
    pub username: String,
    /// Register a bot account (needs a bot invite).
    #[serde(default)]
    pub bot: bool,
}

/// `create_group`: a named group with you as admin.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateGroupParams {
    /// The group's name.
    pub name: String,
    /// Usernames of the other members.
    pub members: Vec<String>,
}

/// People to add to or remove from a group.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MembersParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// Usernames.
    pub members: Vec<String>,
}

/// `rename_group`: a group's new name.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameGroupParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// The new name.
    pub name: String,
}

/// `set_admin`: make a member an admin, or stop them being one.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetAdminParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// The member's username.
    pub username: String,
    /// True to make them an admin, false to remove the role.
    pub admin: bool,
}

/// `set_history`: whether new members are shown past messages.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetHistoryParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// Share past messages with new members.
    pub shared: bool,
    /// With `shared`: how many days back; 0 means all of them.
    #[serde(default)]
    pub days: u32,
}

/// A conversation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationParams {
    /// Conversation ID, hex.
    pub conversation: String,
}

/// The presence you choose to show.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PresenceChoice {
    /// Shown online.
    Online,
    /// Shown away.
    Away,
    /// Shown as do not disturb.
    Dnd,
    /// Shown offline; nothing is sent, and you still receive everything.
    Invisible,
}

/// `set_presence`: what others see.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPresenceParams {
    /// The presence to show.
    pub presence: PresenceChoice,
}

/// `link_begin`: offer this new device for linking to an existing account.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkBeginParams {
    /// Server URL, `wss://...`.
    pub server: String,
    /// A name for this device, shown only on your own devices.
    pub name: String,
    /// The account is a bot account.
    #[serde(default)]
    pub bot: bool,
}

/// A link code shown by a new device.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeParams {
    /// The code.
    pub code: String,
}

/// One of your devices.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceParams {
    /// Device ID, or an unambiguous prefix of at least four characters.
    pub device: String,
}

/// `rename_device`: name one of your devices.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameDeviceParams {
    /// Device ID, or an unambiguous prefix of at least four characters.
    pub device: String,
    /// The new name.
    pub name: String,
}

/// A method with no parameters.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

/// A username.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserParams {
    /// Username.
    pub username: String,
}

/// `messages`: the newest messages of a conversation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessagesParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// How many, newest first; returned oldest first. Default 50.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `search`: messages matching every word of `text` (each the start of a
/// word, ignoring case and accents) and every filter given. Searches this
/// device's index only.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    /// Words that must all appear.
    #[serde(default)]
    pub text: String,
    /// Only in this conversation (ID, hex).
    #[serde(default)]
    pub conversation: Option<String>,
    /// Only sent by this username.
    #[serde(default)]
    pub from: Option<String>,
    /// Only messages with a link.
    #[serde(default)]
    pub has_link: bool,
    /// Only messages that mention this username.
    #[serde(default)]
    pub mentions: Option<String>,
    /// Only sent at or after this (Unix ms).
    #[serde(default)]
    pub after_ms: Option<u64>,
    /// Only sent before this (Unix ms).
    #[serde(default)]
    pub before_ms: Option<u64>,
    /// Oldest first, rather than newest.
    #[serde(default)]
    pub oldest_first: bool,
    /// At most this many. Default 50.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Skip this many first. Default 0.
    #[serde(default)]
    pub offset: Option<u32>,
}

/// What `search` found.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchResults {
    /// How many messages match in all.
    pub total: u32,
    /// The page asked for.
    pub messages: Vec<Message>,
}

/// `send`: send a text message to a conversation, or to a user's DM.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendParams {
    /// Conversation ID, hex. Give this or `to`.
    #[serde(default)]
    pub conversation: Option<String>,
    /// Username to DM, opening the DM if needed. Give this or `conversation`.
    #[serde(default)]
    pub to: Option<String>,
    /// The text.
    pub text: String,
}

/// `edit`: replace the text of one of your messages. Earlier versions stay visible.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// Message ID, hex.
    pub message: String,
    /// The new text.
    pub text: String,
}

/// A message.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// Message ID, hex.
    pub message: String,
}

/// `set_profile`: change your profile. Omitted fields stay as they are.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileParams {
    /// Global display name; empty clears it.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Typing template containing `$$NAME$$`; empty restores the standard text.
    #[serde(default)]
    pub typing_template: Option<String>,
    /// Note (up to 60 characters); empty clears it.
    #[serde(default)]
    pub note: Option<String>,
    /// Keep the new note until changed, instead of 24 hours.
    #[serde(default)]
    pub note_keep: bool,
    /// How your messages arrive on screen in the app: a preset's name
    /// (bubble, rise, decode, typewriter, focus, wave, instant); empty
    /// restores the default.
    #[serde(default)]
    pub arrival: Option<String>,
    /// Banner colour shown when you have no banner image, `#rrggbb`; empty
    /// restores the app's choice.
    #[serde(default)]
    pub banner_color: Option<String>,
}

/// `set_picture`: your profile picture or banner.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPictureParams {
    /// `picture` or `banner`.
    pub kind: String,
    /// An image file (PNG, JPEG, WebP or GIF) on this computer; its middle
    /// is used, in shape. Omitted, the picture is removed.
    #[serde(default)]
    pub file: Option<String>,
}

/// `save_picture`: write a picture this device has to a file.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavePictureParams {
    /// The picture's ID, from a person's `picture` or `banner`.
    pub id: String,
    /// Where to write it. It must not exist.
    pub file: String,
}

/// `set_conversation_name`: your display name in one conversation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationNameParams {
    /// Conversation ID, hex.
    pub conversation: String,
    /// The name; empty clears it.
    pub name: String,
}

/// `set_nickname`: your private nickname for someone.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NicknameParams {
    /// Their username.
    pub username: String,
    /// The nickname; empty clears it.
    pub nickname: String,
}

/// `set_verified`: record that you compared safety numbers with someone.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifiedParams {
    /// Their username.
    pub username: String,
    /// True to mark them verified, false to withdraw it.
    pub verified: bool,
}

/// Who has one of your messages.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct MessageReceipts {
    /// Every other member, and how far they have got.
    pub people: Vec<PersonReceipt>,
    /// The conversation is too large for receipts.
    pub too_large: bool,
    /// Your read receipts are off, so you see deliveries only.
    pub read_receipts_off: bool,
}

/// How far one person has got with your message.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PersonReceipt {
    /// Their username.
    pub username: String,
    /// The name you see for them.
    pub shown_name: String,
    /// `waiting`, `delivered` or `read`.
    pub state: String,
}

/// `create_invite`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInviteParams {
    /// For a bot account.
    #[serde(default)]
    pub bot: bool,
    /// Days until it expires, 1 to 30; default 7.
    #[serde(default)]
    pub days: Option<u32>,
}

/// A single-use invite code.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct InviteCode {
    /// The code.
    pub code: String,
    /// The server address to sign up at with it.
    pub server: String,
    /// When it stops working, Unix ms.
    pub expires_at_ms: u64,
    /// It registers a bot account.
    pub bot: bool,
}

/// `set_read_receipts`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadReceiptsParams {
    /// Send read receipts (the default), or not. Off, you also do not see
    /// when others read yours. Delivery receipts always go.
    pub on: bool,
}

/// Whether read receipts are on.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ReadReceipts {
    /// On.
    pub on: bool,
}

/// `announcements`: the newest announcements.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnnouncementsParams {
    /// How many, newest; default 50.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `announce`: post to everyone on the server.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnnounceParams {
    /// What to say.
    pub text: String,
}

/// `plan_maintenance`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanMaintenanceParams {
    /// When it starts, Unix ms. Leave it out to start now.
    #[serde(default)]
    pub starts_at_ms: Option<u64>,
    /// How long it should take.
    pub minutes: u32,
    /// Why, shown to everyone.
    #[serde(default)]
    pub reason: String,
    /// Do not announce the plan now; the reminders still go out.
    #[serde(default)]
    pub quiet: bool,
}

// ---------------------------------------------------------------- methods

/// Whether a method needs a server connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    /// Works offline, from local data.
    Local,
    /// Needs the account and a connection.
    Connection,
    /// Creates the account; needs no prior connection.
    Registration,
}

/// A method's description, for the schema and the CLI.
pub struct MethodInfo {
    /// Its name.
    pub name: &'static str,
    /// One-line summary.
    pub summary: &'static str,
    /// What it needs.
    pub needs: Needs,
    params: fn() -> Value,
    result: fn() -> Value,
}

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null)
}

macro_rules! methods {
    ($( $name:literal, $needs:ident, $params:ty, $result:ty, $summary:literal; )*) => {
        /// Every method, in a stable order.
        pub const METHODS: &[MethodInfo] = &[
            $( MethodInfo { name: $name, summary: $summary, needs: Needs::$needs, params: schema::<$params>, result: schema::<$result> }, )*
        ];
    };
}

methods! {
    "register", Registration, RegisterParams, Identity, "Create an account with an invite.";
    "link_begin", Registration, LinkBeginParams, LinkCode, "On a new device: get a code to link it to your account from another device.";
    "link_finish", Registration, NoParams, Identity, "On the new device: wait for the code to be approved, then join.";
    "link_inspect", Connection, CodeParams, LinkRequest, "On one of your devices: which device a link code would add.";
    "link_approve", Connection, CodeParams, LinkRequest, "On one of your devices: add the device behind a link code.";
    "devices", Local, NoParams, Vec<Device>, "Your devices, oldest first.";
    "refresh_keys", Connection, NoParams, Done, "Give this device fresh keys in every conversation now (otherwise weekly).";
    "rename_device", Connection, RenameDeviceParams, Done, "Name one of your devices.";
    "remove_device", Connection, DeviceParams, Done, "Remove another of your devices from your account and every conversation.";
    "whoami", Local, NoParams, Option<Identity>, "The account on this device, if any.";
    "sync", Connection, NoParams, Done, "Fetch everything new and send what is queued.";
    "start_dm", Connection, UserParams, Conversation, "Open a DM with a user, creating it if needed.";
    "create_group", Connection, CreateGroupParams, Conversation, "Create a named group with you as admin.";
    "members", Local, ConversationParams, Vec<Member>, "The members of a conversation.";
    "add_members", Connection, MembersParams, Done, "As an admin: add people to a group.";
    "remove_members", Connection, MembersParams, Done, "As an admin: remove people from a group.";
    "rename_group", Connection, RenameGroupParams, Done, "As an admin: rename a group.";
    "set_admin", Connection, SetAdminParams, Done, "As an admin: make a member an admin, or stop them being one.";
    "set_history", Connection, SetHistoryParams, Done, "As an admin: whether new members are shown past messages.";
    "leave_group", Connection, ConversationParams, Done, "Leave a group, with all your devices.";
    "conversations", Local, NoParams, Vec<Conversation>, "Your conversations, most recent first.";
    "messages", Local, MessagesParams, Vec<Message>, "The newest messages of a conversation, oldest first.";
    "search", Local, SearchParams, SearchResults, "Messages matching the words and filters given, from this device's search index.";
    "send", Connection, SendParams, Message, "Send a text message.";
    "edit", Connection, EditParams, Message, "Edit one of your messages; earlier versions stay visible.";
    "delete", Connection, MessageParams, Done, "Delete one of your messages, with every version, for everyone.";
    "versions", Local, MessageParams, Vec<MessageVersion>, "Every version of a message, the original first.";
    "set_profile", Connection, ProfileParams, Done, "Change your display name, typing template, note, arrival style or banner colour.";
    "set_picture", Connection, SetPictureParams, Done, "Set your profile picture or banner from an image file, or remove it.";
    "save_picture", Local, SavePictureParams, Done, "Write a profile picture or banner this device has to a file.";
    "set_conversation_name", Connection, ConversationNameParams, Done, "Set your display name in one conversation.";
    "set_nickname", Local, NicknameParams, Done, "Set your private nickname for someone.";
    "person", Local, UserParams, Person, "What you know about a person.";
    "send_typing", Connection, ConversationParams, Done, "Tell a conversation you are typing (sent at most every 3 seconds).";
    "set_presence", Connection, SetPresenceParams, Done, "Choose the presence others see.";
    "presence", Local, UserParams, PresenceResult, "Someone's presence, as last heard by this device.";
    "safety_number", Local, UserParams, SafetyNumber, "The safety number between you and a user.";
    "set_verified", Connection, VerifiedParams, Done, "Record that you compared safety numbers with a user, or withdraw it.";
    "mark_read", Connection, ConversationParams, Done, "Mark a conversation read, on all your devices.";
    "read_receipts", Local, NoParams, ReadReceipts, "Whether you send read receipts.";
    "message_receipts", Local, MessageParams, MessageReceipts, "Who has one of your messages, and who has read it.";
    "set_read_receipts", Connection, ReadReceiptsParams, Done, "Send read receipts or stop, on all your devices.";
    // A connection: calls live in memory, so a one-shot CLI must fetch first.
    "calls", Connection, NoParams, Vec<CallInfo>, "The live calls, in every conversation.";
    "call_start", Connection, CallStartParams, CallJoined, "Start a call (signalling only: bring your own audio with the token).";
    "call_join", Connection, CallParams, CallJoined, "Join or answer a call (signalling only).";
    "call_leave", Connection, CallParams, Done, "Leave a call; in a DM this ends it.";
    "call_decline", Connection, CallParams, Done, "Decline a ringing call.";
    "call_ring", Connection, CallRingParams, Done, "Ring people into a live call.";
    // A connection, so a one-shot CLI shows the server's newest.
    "announcements", Connection, AnnouncementsParams, Vec<Announcement>, "The server's newest announcements, oldest first.";
    "announcement_summary", Connection, NoParams, AnnouncementSummary, "Unread announcements, whether you are a host, and planned maintenance.";
    "mark_announcements_read", Connection, NoParams, Done, "Mark the announcements read, on all your devices.";
    "announce", Connection, AnnounceParams, Done, "As a host: post an announcement to everyone on the server.";
    "plan_maintenance", Connection, PlanMaintenanceParams, Done, "As a host: plan maintenance; everyone is told and reminded.";
    "end_maintenance", Connection, NoParams, Done, "As a host: cancel the planned maintenance, or announce it is over.";
    "create_invite", Connection, CreateInviteParams, InviteCode, "As a host: make a single-use invite code for someone to sign up with.";
}

/// Find a method by name.
pub fn method(name: &str) -> Option<&'static MethodInfo> {
    METHODS.iter().find(|m| m.name == name)
}

/// An API error, with a JSON-RPC error code.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    /// JSON-RPC error code. -32601: unknown method; -32602: bad parameters;
    /// 1000 and up: Post errors (see [`code_of`]).
    pub code: i64,
    /// Human-readable message.
    pub message: String,
}

/// JSON-RPC code for each core error.
pub fn code_of(e: &core::Error) -> i64 {
    use core::Error as E;
    match e {
        E::NotRegistered => 1001,
        E::AlreadyRegistered => 1002,
        E::NotConnected | E::Network(_) => 1003,
        E::Server(_) => 1004,
        E::InvalidInput(_) => 1005,
        E::NotFound(_) => 1006,
        E::NotAllowed(_) => 1007,
        E::UnknownUser(_) => 1008,
        E::IdentityChanged(_) => 1009,
        E::PeerUnavailable(_) => 1010,
        E::NoTokens => 1011,
        E::WrongKey => 1012,
        _ => 1099,
    }
}

impl From<core::Error> for ApiError {
    fn from(e: core::Error) -> Self {
        Self { code: code_of(&e), message: e.to_string() }
    }
}

fn params<T: DeserializeOwned>(v: Value) -> Result<T, ApiError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| ApiError { code: -32602, message: format!("invalid parameters: {e}") })
}

fn id<T: std::str::FromStr<Err = core::Error>>(s: &str) -> Result<T, ApiError> {
    s.parse().map_err(ApiError::from)
}

/// `#rrggbb` (or `rrggbb`) as `0xRRGGBB`.
fn parse_color(s: &str) -> Result<u32, ApiError> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    match u32::from_str_radix(hex, 16) {
        Ok(c) if hex.len() == 6 => Ok(c),
        _ => Err(ApiError { code: -32602, message: format!("{s} is not a colour like #6e8bff") }),
    }
}

/// An image file to make a picture from, refusing one too large to be one.
fn read_image(path: &str) -> Result<Vec<u8>, ApiError> {
    let bad = |m: String| ApiError { code: -32602, message: m };
    let size = std::fs::metadata(path).map_err(|e| bad(format!("reading {path}: {e}")))?.len();
    if size > core::picture::MAX_SOURCE_BYTES as u64 {
        return Err(bad(format!("{path} is over {} MB", core::picture::MAX_SOURCE_BYTES / 1024 / 1024)));
    }
    std::fs::read(path).map_err(|e| bad(format!("reading {path}: {e}")))
}

fn ok<T: Serialize>(v: T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| ApiError { code: -32603, message: e.to_string() })
}

/// Run one method. The caller is responsible for connecting first when the
/// method's [`MethodInfo::needs`] says so.
pub async fn call(core: &Core, name: &str, p: Value) -> Result<Value, ApiError> {
    match name {
        "register" => {
            let p: RegisterParams = params(p)?;
            ok(identity(core.register(&p.server, &p.invite, &p.username, p.bot).await?))
        }
        "link_begin" => {
            let p: LinkBeginParams = params(p)?;
            ok(LinkCode { code: core.begin_link(&p.server, &p.name, p.bot).await? })
        }
        "link_finish" => {
            params::<NoParams>(p)?;
            ok(identity(core.finish_link().await?))
        }
        "link_inspect" | "link_approve" => {
            let p: CodeParams = params(p)?;
            let r = if name == "link_inspect" { core.link_inspect(&p.code).await? } else { core.link_approve(&p.code).await? };
            ok(LinkRequest { device_id: r.device_id, name: r.name })
        }
        "devices" => {
            params::<NoParams>(p)?;
            let ds = core.devices().await?;
            ok(ds.into_iter().map(|d| Device { id: d.id, name: d.name, this_device: d.this_device }).collect::<Vec<_>>())
        }
        "refresh_keys" => {
            params::<NoParams>(p)?;
            core.refresh_keys().await?;
            ok(Done {})
        }
        "rename_device" => {
            let p: RenameDeviceParams = params(p)?;
            core.rename_device(&p.device, &p.name).await?;
            ok(Done {})
        }
        "remove_device" => {
            let p: DeviceParams = params(p)?;
            core.remove_device(&p.device).await?;
            ok(Done {})
        }
        "whoami" => {
            params::<NoParams>(p)?;
            ok(core.identity().await.map(identity))
        }
        "sync" => {
            params::<NoParams>(p)?;
            core.sync().await?;
            ok(Done {})
        }
        "start_dm" => {
            let p: UserParams = params(p)?;
            ok(conversation(core.start_dm(&p.username).await?))
        }
        "create_group" => {
            let p: CreateGroupParams = params(p)?;
            ok(conversation(core.create_group(&p.name, &p.members).await?))
        }
        "members" => {
            let p: ConversationParams = params(p)?;
            let ms = core.members(id(&p.conversation)?).await?;
            ok(ms
                .into_iter()
                .map(|m| Member { username: m.username, shown_name: m.shown_name, admin: m.admin, devices: m.devices, you: m.you })
                .collect::<Vec<_>>())
        }
        "add_members" | "remove_members" => {
            let p: MembersParams = params(p)?;
            let conv = id(&p.conversation)?;
            if name == "add_members" { core.add_members(conv, &p.members).await? } else { core.remove_members(conv, &p.members).await? }
            ok(Done {})
        }
        "rename_group" => {
            let p: RenameGroupParams = params(p)?;
            core.rename_group(id(&p.conversation)?, &p.name).await?;
            ok(Done {})
        }
        "set_admin" => {
            let p: SetAdminParams = params(p)?;
            core.set_admin(id(&p.conversation)?, &p.username, p.admin).await?;
            ok(Done {})
        }
        "set_history" => {
            let p: SetHistoryParams = params(p)?;
            core.set_history(id(&p.conversation)?, p.shared, p.days).await?;
            ok(Done {})
        }
        "leave_group" => {
            let p: ConversationParams = params(p)?;
            core.leave_group(id(&p.conversation)?).await?;
            ok(Done {})
        }
        "conversations" => {
            params::<NoParams>(p)?;
            ok(core.conversations()?.into_iter().map(conversation).collect::<Vec<_>>())
        }
        "messages" => {
            let p: MessagesParams = params(p)?;
            ok(core.messages(id(&p.conversation)?, p.limit.unwrap_or(50))?.into_iter().map(message).collect::<Vec<_>>())
        }
        "search" => {
            let p: SearchParams = params(p)?;
            let query = core::SearchQuery {
                text: p.text,
                conversation: p.conversation.as_deref().map(id).transpose()?,
                from: p.from,
                mentions: p.mentions,
                has_link: p.has_link,
                after_ms: p.after_ms,
                before_ms: p.before_ms,
                oldest_first: p.oldest_first,
                limit: p.limit.unwrap_or(50),
                offset: p.offset.unwrap_or(0),
            };
            let found = core.search(&query)?;
            ok(SearchResults { total: found.total, messages: found.messages.into_iter().map(message).collect() })
        }
        "send" => {
            let p: SendParams = params(p)?;
            let conv: ConversationId = match (p.conversation, p.to) {
                (Some(c), None) => id(&c)?,
                (None, Some(user)) => core.start_dm(&user).await?.id,
                _ => return Err(ApiError { code: -32602, message: "give exactly one of conversation or to".into() }),
            };
            ok(message(core.send_text(conv, &p.text).await?))
        }
        "edit" => {
            let p: EditParams = params(p)?;
            ok(message(core.edit_message(id(&p.conversation)?, id::<MessageId>(&p.message)?, &p.text).await?))
        }
        "delete" => {
            let p: MessageParams = params(p)?;
            core.delete_message(id(&p.conversation)?, id::<MessageId>(&p.message)?).await?;
            ok(Done {})
        }
        "versions" => {
            let p: MessageParams = params(p)?;
            let vs = core.message_versions(id(&p.conversation)?, id::<MessageId>(&p.message)?)?;
            ok(vs
                .into_iter()
                .map(|v| MessageVersion { version: v.version, body: v.body, at_ms: v.at_ms, pending: v.pending })
                .collect::<Vec<_>>())
        }
        "set_profile" => {
            let p: ProfileParams = params(p)?;
            let banner_color = match p.banner_color.as_deref() {
                None => None,
                Some("") => Some(None),
                Some(c) => Some(Some(parse_color(c)?)),
            };
            core.set_profile(core::ProfileUpdate {
                display_name: p.display_name,
                typing_template: p.typing_template,
                note: p.note,
                note_keep: p.note_keep,
                arrival: p.arrival,
                banner_color,
            })
            .await?;
            ok(Done {})
        }
        "set_picture" => {
            let p: SetPictureParams = params(p)?;
            let kind = match p.kind.as_str() {
                "picture" => core::picture::PictureKind::Avatar,
                "banner" => core::picture::PictureKind::Banner,
                _ => return Err(ApiError { code: -32602, message: "kind is picture or banner".into() }),
            };
            let source = match p.file {
                Some(f) => Some((read_image(&f)?, None)),
                None => None,
            };
            core.set_picture(kind, source).await?;
            ok(Done {})
        }
        "save_picture" => {
            let p: SavePictureParams = params(p)?;
            let bytes = core.picture(&p.id)?.ok_or_else(|| ApiError { code: -32602, message: "this device has no picture with that ID".into() })?;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p.file)
                .and_then(|mut f| std::io::Write::write_all(&mut f, &bytes))
                .map_err(|e| ApiError { code: -32602, message: format!("writing {}: {e}", p.file) })?;
            ok(Done {})
        }
        "set_conversation_name" => {
            let p: ConversationNameParams = params(p)?;
            core.set_conversation_name(id(&p.conversation)?, &p.name).await?;
            ok(Done {})
        }
        "set_nickname" => {
            let p: NicknameParams = params(p)?;
            core.set_nickname(&p.username, &p.nickname).await?;
            ok(Done {})
        }
        "person" => {
            let p: UserParams = params(p)?;
            let x = core.person(&p.username).await?;
            ok(Person {
                username: x.username,
                shown_name: x.shown_name,
                display_name: x.display_name,
                nickname: x.nickname,
                typing_template: x.typing_template,
                note: x.note,
                verified: x.verified,
                arrival: x.arrival,
                picture: x.avatar,
                banner: x.banner,
                banner_color: x.banner_color.map(|c| format!("#{c:06x}")),
            })
        }
        "set_verified" => {
            let p: VerifiedParams = params(p)?;
            core.set_verified(&p.username, p.verified).await?;
            ok(Done {})
        }
        "mark_read" => {
            let p: ConversationParams = params(p)?;
            core.mark_read(id(&p.conversation)?).await?;
            ok(Done {})
        }
        "send_typing" => {
            let p: ConversationParams = params(p)?;
            core.send_typing(id(&p.conversation)?).await?;
            ok(Done {})
        }
        "calls" => ok(core.calls().await?.into_iter().map(call_info).collect::<Vec<_>>()),
        "call_start" => {
            let p: CallStartParams = params(p)?;
            ok(call_joined(core.start_call(id(&p.conversation)?, p.video, &p.ring).await?))
        }
        "call_ring" => {
            let p: CallRingParams = params(p)?;
            core.ring_call(id(&p.conversation)?, id(&p.call)?, &p.usernames).await?;
            ok(Done {})
        }
        "call_join" => {
            let p: CallParams = params(p)?;
            ok(call_joined(core.join_call(id(&p.conversation)?, id(&p.call)?).await?))
        }
        "call_leave" => {
            let p: CallParams = params(p)?;
            core.leave_call(id(&p.conversation)?, id(&p.call)?).await?;
            ok(Done {})
        }
        "call_decline" => {
            let p: CallParams = params(p)?;
            core.decline_call(id(&p.conversation)?, id(&p.call)?).await?;
            ok(Done {})
        }
        "set_presence" => {
            let p: SetPresenceParams = params(p)?;
            let setting = match p.presence {
                PresenceChoice::Online => core::PresenceSetting::Online,
                PresenceChoice::Away => core::PresenceSetting::Away,
                PresenceChoice::Dnd => core::PresenceSetting::DoNotDisturb,
                PresenceChoice::Invisible => core::PresenceSetting::Invisible,
            };
            core.set_presence(setting).await?;
            ok(Done {})
        }
        "presence" => {
            let p: UserParams = params(p)?;
            ok(PresenceResult { presence: presence_word(core.presence(&p.username).await?).into() })
        }
        "safety_number" => {
            let p: UserParams = params(p)?;
            ok(SafetyNumber { safety_number: core.safety_number(&p.username).await? })
        }
        "read_receipts" => ok(ReadReceipts { on: core.read_receipts()? }),
        "message_receipts" => {
            let p: MessageParams = params(p)?;
            let r = core.message_receipts(id(&p.conversation)?, id(&p.message)?).await?;
            ok(MessageReceipts {
                people: r
                    .people
                    .into_iter()
                    .map(|x| PersonReceipt {
                        username: x.username,
                        shown_name: x.shown_name,
                        state: match x.state {
                            core::ReceiptState::Waiting => "waiting",
                            core::ReceiptState::Delivered => "delivered",
                            core::ReceiptState::Read => "read",
                        }
                        .into(),
                    })
                    .collect(),
                too_large: r.too_large,
                read_receipts_off: r.read_receipts_off,
            })
        }
        "set_read_receipts" => {
            let p: ReadReceiptsParams = params(p)?;
            core.set_read_receipts(p.on).await?;
            ok(Done {})
        }
        "announcements" => {
            let p: AnnouncementsParams = params(p)?;
            ok(core.announcements(p.limit.unwrap_or(50))?.into_iter().map(announcement).collect::<Vec<_>>())
        }
        "announcement_summary" => ok(announcement_summary(core.announcement_summary()?)),
        "mark_announcements_read" => {
            core.mark_announcements_read().await?;
            ok(Done {})
        }
        "announce" => {
            let p: AnnounceParams = params(p)?;
            core.announce(&p.text).await?;
            ok(Done {})
        }
        "plan_maintenance" => {
            let p: PlanMaintenanceParams = params(p)?;
            core.plan_maintenance(p.starts_at_ms.unwrap_or(0), p.minutes, &p.reason, p.quiet).await?;
            ok(Done {})
        }
        "end_maintenance" => {
            core.end_maintenance().await?;
            ok(Done {})
        }
        "create_invite" => {
            let p: CreateInviteParams = params(p)?;
            let i = core.create_invite(p.bot, p.days.unwrap_or(7)).await?;
            ok(InviteCode { code: i.code, server: i.server, expires_at_ms: i.expires_at_ms, bot: i.bot })
        }
        _ => Err(ApiError { code: -32601, message: format!("unknown method {name}") }),
    }
}

/// The OpenRPC document describing every method and the `event` notification.
pub fn openrpc() -> Value {
    let methods: Vec<Value> = METHODS
        .iter()
        .map(|m| {
            let p = (m.params)();
            let required: Vec<String> = p["required"].as_array().map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let params: Vec<Value> = p["properties"]
                .as_object()
                .map(|props| {
                    props
                        .iter()
                        .map(|(name, schema)| json!({ "name": name, "required": required.contains(name), "schema": schema }))
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "name": m.name,
                "summary": m.summary,
                "paramStructure": "by-name",
                "params": params,
                "result": { "name": "result", "schema": (m.result)() },
            })
        })
        .collect();
    json!({
        "openrpc": "1.3.2",
        "info": {
            "title": "Post local API",
            "version": VERSION,
            "description": "Served by `post daemon`. After calling `subscribe`, the daemon sends `event` notifications.",
        },
        "methods": methods,
        "components": { "schemas": { "Event": schema::<Event>() } },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_method_dispatches_and_has_a_schema() {
        let doc = openrpc();
        assert_eq!(doc["methods"].as_array().map(Vec::len), Some(METHODS.len()));
        for m in METHODS {
            assert!(method(m.name).is_some());
            assert!((m.params)().is_object(), "{} params schema", m.name);
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(params::<UserParams>(json!({ "username": "a", "usernmae": "b" })).is_err());
        assert!(params::<UserParams>(json!({ "username": "a" })).is_ok());
        assert!(params::<NoParams>(Value::Null).is_ok());
    }
}
