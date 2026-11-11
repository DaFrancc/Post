//! Postern's local API (design section 11, "Command-line client and local
//! API").
//!
//! One set of named methods with typed parameters and results. The CLI maps
//! each subcommand to one method; the daemon serves the same methods as
//! JSON-RPC 2.0. Both go through [`call`], so they cannot drift apart.
//!
//! Versioned as `postern.v1`: within a version, changes are additive only.
//! Parameters reject unknown fields, so a typo fails instead of being ignored.

use postern_core::{self as core, ConversationId, Core, MessageId};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The API version.
pub const VERSION: &str = "postern.v1";

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
    /// `pending`, `sent`, `received` or `failed`.
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
    /// Something you should know, usually a security warning.
    Notice {
        /// Conversation ID, if it concerns one.
        conversation: Option<String>,
        /// What happened.
        text: String,
    },
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
            core::MessageState::Received => "received",
            core::MessageState::Failed => "failed",
        }
        .into(),
    }
}

fn identity(i: core::Identity) -> Identity {
    Identity { username: i.username, account_id: i.account_id, device_id: i.device_id, server: i.server, bot: i.bot }
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
        core::Event::Notice { conversation, text } => Event::Notice { conversation: conversation.map(|c| c.to_string()), text },
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
    "conversations", Local, NoParams, Vec<Conversation>, "Your conversations, most recent first.";
    "messages", Local, MessagesParams, Vec<Message>, "The newest messages of a conversation, oldest first.";
    "send", Connection, SendParams, Message, "Send a text message.";
    "edit", Connection, EditParams, Message, "Edit one of your messages; earlier versions stay visible.";
    "delete", Connection, MessageParams, Done, "Delete one of your messages, with every version, for everyone.";
    "versions", Local, MessageParams, Vec<MessageVersion>, "Every version of a message, the original first.";
    "set_profile", Connection, ProfileParams, Done, "Change your display name, typing template or note.";
    "set_conversation_name", Connection, ConversationNameParams, Done, "Set your display name in one conversation.";
    "set_nickname", Local, NicknameParams, Done, "Set your private nickname for someone.";
    "person", Local, UserParams, Person, "What you know about a person.";
    "safety_number", Local, UserParams, SafetyNumber, "The safety number between you and a user.";
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
    /// 1000 and up: Postern errors (see [`code_of`]).
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
        "conversations" => {
            params::<NoParams>(p)?;
            ok(core.conversations()?.into_iter().map(conversation).collect::<Vec<_>>())
        }
        "messages" => {
            let p: MessagesParams = params(p)?;
            ok(core.messages(id(&p.conversation)?, p.limit.unwrap_or(50))?.into_iter().map(message).collect::<Vec<_>>())
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
            core.set_profile(core::ProfileUpdate {
                display_name: p.display_name,
                typing_template: p.typing_template,
                note: p.note,
                note_keep: p.note_keep,
            })
            .await?;
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
            })
        }
        "safety_number" => {
            let p: UserParams = params(p)?;
            ok(SafetyNumber { safety_number: core.safety_number(&p.username).await? })
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
            "title": "Postern local API",
            "version": VERSION,
            "description": "Served by `postern daemon`. After calling `subscribe`, the daemon sends `event` notifications.",
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
