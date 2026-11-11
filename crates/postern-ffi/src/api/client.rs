//! One Postern profile, opened, as the app sees it.
//!
//! Types here are plain data the generator mirrors into Dart classes. Events
//! are one struct with a `kind` rather than a Rust enum with fields, so the
//! Dart side needs no extra code generation (`freezed`).

use std::path::PathBuf;

use flutter_rust_bridge::frb;
use postern_core::{self as core, ConversationId, Core};

use super::error::{ErrorKind, PosternError, from_anyhow};
use crate::frb_generated::StreamSink;

/// Called once when the Dart side loads the library: logging and panic
/// backtraces.
#[frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

/// An open profile: one account on this device.
pub struct Client {
    core: Core,
}

/// The account on this device.
pub struct Identity {
    /// Username.
    pub username: String,
    /// Account ID, hex. The safety number is derived from it.
    pub account_id: String,
    /// This device's ID, hex.
    pub device_id: String,
    /// Server URL.
    pub server: String,
    /// Whether it is a bot account.
    pub bot: bool,
}

/// A conversation in the list.
pub struct ConversationInfo {
    /// Conversation ID, hex.
    pub id: String,
    /// A group chat, rather than a DM.
    pub is_group: bool,
    /// What to call it.
    pub title: String,
    /// The other person's username, for a DM.
    pub peer: Option<String>,
    /// Unix milliseconds of the latest activity.
    pub last_activity_ms: i64,
    /// False once this device left or was removed.
    pub following: bool,
}

/// A message.
pub struct MessageInfo {
    /// Message ID, hex.
    pub id: String,
    /// Conversation ID, hex.
    pub conversation: String,
    /// The name the viewer sees for the sender here.
    pub sender: String,
    /// The sender's username.
    pub sender_username: String,
    /// Sent by this account (from any of its devices).
    pub outgoing: bool,
    /// When the sender sent it, by their clock (Unix ms).
    pub sent_at_ms: i64,
    /// The latest text; `None` if deleted or of an unsupported kind.
    pub body: Option<String>,
    /// Deleted by its sender.
    pub deleted: bool,
    /// A kind of message this version does not understand.
    pub unsupported: bool,
    /// Number of versions, the original included.
    pub versions: u32,
    /// Not yet accepted by the server.
    pub pending: bool,
    /// The server refused it.
    pub failed: bool,
}

/// What changed. The app re-reads what it shows for that kind.
pub enum EventKind {
    /// The conversation list (one was added, renamed, left).
    Conversations,
    /// Messages in `conversation`.
    Messages,
    /// Someone's profile or shown name (`username`).
    Profile,
    /// `username` is typing in `conversation`; `text` is what to show.
    Typing,
    /// `username`'s presence is now `text`.
    Presence,
    /// `username` added or removed devices.
    Devices,
    /// Something to tell the user (`text`), usually a security warning.
    Notice,
}

/// A live event.
pub struct AppEvent {
    /// What kind of change.
    pub kind: EventKind,
    /// The conversation it concerns, if any.
    pub conversation: Option<String>,
    /// The person it concerns, if any.
    pub username: Option<String>,
    /// Text to show, if any.
    pub text: Option<String>,
}

impl Client {
    /// Open (creating if needed) the profile in `profile_dir`. Its database
    /// key comes from the platform keystore, made on first open.
    pub async fn open(profile_dir: String) -> Result<Client, PosternError> {
        let dir = PathBuf::from(profile_dir);
        let core = tokio::task::spawn_blocking(move || -> anyhow::Result<Core> {
            let key = crate::keystore::key_for(&dir)?;
            Ok(Core::open(&dir, &key)?)
        })
        .await
        .map_err(|_| PosternError::new(ErrorKind::Other, "Opening the profile stopped unexpectedly."))?
        .map_err(from_anyhow)?;
        Ok(Client { core })
    }

    /// The account on this device, if there is one yet.
    pub async fn identity(&self) -> Option<Identity> {
        self.core.identity().await.map(identity)
    }

    /// Create an account with an invite.
    pub async fn register(&self, server: String, invite: String, username: String) -> Result<Identity, PosternError> {
        Ok(identity(self.core.register(&server, &invite, &username, false).await?))
    }

    /// On a new device: offer it for linking to an existing account on
    /// `server`, as `device_name`. Returns the code to show; then call
    /// [`Client::finish_link`].
    pub async fn begin_link(&self, server: String, device_name: String) -> Result<String, PosternError> {
        Ok(self.core.begin_link(&server, &device_name, false).await?)
    }

    /// On the new device: wait (up to ten minutes) for the code to be
    /// approved on another device, then join the account.
    pub async fn finish_link(&self) -> Result<Identity, PosternError> {
        Ok(identity(self.core.finish_link().await?))
    }

    /// Stay connected until the app closes: run this once, without awaiting
    /// its end. It reconnects by itself.
    pub async fn run(&self) {
        self.core.run().await;
    }

    /// Fetch everything new now.
    pub async fn sync(&self) -> Result<(), PosternError> {
        self.core.connect().await?;
        Ok(self.core.sync().await?)
    }

    /// Conversations, most recent first.
    pub async fn conversations(&self) -> Result<Vec<ConversationInfo>, PosternError> {
        Ok(self.core.conversations()?.into_iter().map(conversation).collect())
    }

    /// The newest `limit` messages of a conversation, oldest first.
    pub async fn messages(&self, conversation: String, limit: u32) -> Result<Vec<MessageInfo>, PosternError> {
        let id: ConversationId = conversation.parse()?;
        Ok(self.core.messages(id, limit)?.into_iter().map(message).collect())
    }

    /// Send a text message.
    pub async fn send_text(&self, conversation: String, text: String) -> Result<MessageInfo, PosternError> {
        let id: ConversationId = conversation.parse()?;
        Ok(message(self.core.send_text(id, &text).await?))
    }

    /// Live events, until the Dart stream is closed.
    pub async fn events(&self, sink: StreamSink<AppEvent>) {
        let mut rx = self.core.subscribe();
        loop {
            match rx.recv().await {
                Ok(e) => {
                    if sink.add(app_event(e)).is_err() {
                        return;
                    }
                }
                // Missed some: the app re-reads everything it shows.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let all = AppEvent { kind: EventKind::Conversations, conversation: None, username: None, text: None };
                    if sink.add(all).is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

fn identity(i: core::Identity) -> Identity {
    Identity { username: i.username, account_id: i.account_id, device_id: i.device_id, server: i.server, bot: i.bot }
}

fn conversation(c: core::Conversation) -> ConversationInfo {
    ConversationInfo {
        id: c.id.to_string(),
        is_group: c.kind == core::ConversationKind::Group,
        title: c.title,
        peer: c.peer,
        last_activity_ms: i64::try_from(c.last_activity_ms).unwrap_or(i64::MAX),
        following: c.following,
    }
}

fn message(m: core::Message) -> MessageInfo {
    MessageInfo {
        id: m.id.to_string(),
        conversation: m.conversation.to_string(),
        sender: m.sender,
        sender_username: m.sender_username,
        outgoing: m.outgoing,
        sent_at_ms: i64::try_from(m.sent_at_ms).unwrap_or(i64::MAX),
        body: m.body,
        deleted: m.deleted,
        unsupported: m.unsupported,
        versions: m.versions,
        pending: m.state == core::MessageState::Pending,
        failed: m.state == core::MessageState::Failed,
    }
}

fn app_event(e: core::Event) -> AppEvent {
    let ev = |kind, conversation: Option<ConversationId>, username: Option<String>, text: Option<String>| AppEvent {
        kind,
        conversation: conversation.map(|c| c.to_string()),
        username,
        text,
    };
    match e {
        core::Event::ConversationAdded(c) | core::Event::ConversationChanged(c) => ev(EventKind::Conversations, Some(c.id), None, None),
        core::Event::MessageAdded(m) | core::Event::MessageChanged(m) => ev(EventKind::Messages, Some(m.conversation), None, None),
        core::Event::MessageDeleted { conversation, .. } => ev(EventKind::Messages, Some(conversation), None, None),
        core::Event::ProfileChanged { username } => ev(EventKind::Profile, None, Some(username), None),
        core::Event::DevicesChanged { username, .. } => ev(EventKind::Devices, None, Some(username), None),
        core::Event::Typing { conversation, username, text } => ev(EventKind::Typing, Some(conversation), Some(username), Some(text)),
        core::Event::Presence { username, presence } => {
            let word = match presence {
                core::Presence::Online => "online",
                core::Presence::Away => "away",
                core::Presence::DoNotDisturb => "dnd",
                core::Presence::Offline => "offline",
            };
            ev(EventKind::Presence, None, Some(username), Some(word.into()))
        }
        core::Event::Notice { conversation, text } => ev(EventKind::Notice, conversation, None, Some(text)),
    }
}
