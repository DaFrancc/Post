//! One Postern profile, opened, as the app sees it.
//!
//! Types here are plain data the generator mirrors into Dart classes. Events
//! are one struct with a `kind` rather than a Rust enum with fields, so the
//! Dart side needs no extra code generation (`freezed`). IDs cross as hex
//! strings, times as Unix milliseconds.

use std::path::PathBuf;

use flutter_rust_bridge::frb;
use postern_core::{self as core, ConversationId, Core, MessageId};

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
    /// Messages from others since the user last read it.
    pub unread: u32,
    /// The newest message, for a preview.
    pub last_message: Option<MessageInfo>,
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
    /// Edited at least once.
    pub edited: bool,
    /// Number of versions, the original included.
    pub versions: u32,
    /// Not yet accepted by the server.
    pub pending: bool,
    /// The server refused it.
    pub failed: bool,
}

/// One version of an edited message.
pub struct MessageVersionInfo {
    /// 0 is the original.
    pub version: u32,
    /// Its text.
    pub body: String,
    /// When it was written, by the sender's clock (Unix ms).
    pub at_ms: i64,
    /// Not yet accepted by the server (own edits only).
    pub pending: bool,
}

/// A member of a conversation.
pub struct MemberInfo {
    /// Username.
    pub username: String,
    /// The name the viewer sees for them here.
    pub shown_name: String,
    /// May add and remove people and change the group's details.
    pub admin: bool,
    /// How many of their devices are in the conversation.
    pub devices: u32,
    /// It is the viewer.
    pub you: bool,
}

/// What this device knows about a person.
pub struct PersonInfo {
    /// Username.
    pub username: String,
    /// The name shown for them outside any conversation.
    pub shown_name: String,
    /// The display name they chose; empty if none.
    pub display_name: String,
    /// The viewer's private nickname for them; empty if none.
    pub nickname: String,
    /// Their typing template; empty for the standard text.
    pub typing_template: String,
    /// Their note, if set and not expired.
    pub note: Option<String>,
    /// The viewer compared safety numbers with them.
    pub verified: bool,
}

/// One of this account's devices.
pub struct DeviceInfo {
    /// Device ID, hex.
    pub id: String,
    /// Its name; empty if none.
    pub name: String,
    /// The device this app runs on.
    pub this_device: bool,
}

/// A device asking to join this account.
pub struct LinkRequestInfo {
    /// Its device ID, hex.
    pub device_id: String,
    /// The name its user gave it; empty if none.
    pub name: String,
}

/// Something the user was told, usually a security warning.
pub struct NoticeInfo {
    /// When (Unix ms).
    pub at_ms: i64,
    /// What happened, as a sentence.
    pub text: String,
}

/// What others see of someone.
pub enum PresenceState {
    /// Away from the app for a while, signed off, or invisible.
    Offline,
    /// Using the app.
    Online,
    /// Set by hand, or idle for ten minutes.
    Away,
    /// Not to be disturbed.
    DoNotDisturb,
}

/// The presence the user chooses to show.
pub enum PresenceChoice {
    /// Shown online.
    Online,
    /// Shown away.
    Away,
    /// Shown as not to be disturbed; notifications are silenced.
    DoNotDisturb,
    /// Shown offline; everything still arrives.
    Invisible,
}

/// A change to the user's profile. `None` leaves a field as it is; an empty
/// string clears it.
pub struct ProfileChange {
    /// Display name.
    pub display_name: Option<String>,
    /// Typing template, containing `$$NAME$$`.
    pub typing_template: Option<String>,
    /// Note.
    pub note: Option<String>,
    /// With a new note: keep it until changed, rather than for 24 hours.
    pub note_keep: bool,
}

/// What changed. The app re-reads what it shows for that kind.
pub enum EventKind {
    /// The conversation list (one was added, renamed, left, read).
    Conversations,
    /// Messages in `conversation`.
    Messages,
    /// Someone's profile or shown name (`username`).
    Profile,
    /// `username` is typing in `conversation`; `text` is what to show.
    Typing,
    /// `username`'s presence is now `text`: online, away, dnd or offline.
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
    /// key comes from the platform keystore, made on first open; or, with
    /// `key_file`, from that file (as the CLI does), which lets test and
    /// headless profiles stay out of the keystore.
    pub async fn open(profile_dir: String, key_file: Option<String>) -> Result<Client, PosternError> {
        let dir = PathBuf::from(profile_dir);
        let core = tokio::task::spawn_blocking(move || -> anyhow::Result<Core> {
            let key = match key_file {
                Some(f) => core::keyfile::read(std::path::Path::new(&f))?,
                None => crate::keystore::key_for(&dir)?,
            };
            Ok(Core::open(&dir, &key)?)
        })
        .await
        .map_err(|_| PosternError::new(ErrorKind::Other, "Opening the profile stopped unexpectedly."))?
        .map_err(from_anyhow)?;
        Ok(Client { core })
    }

    // ------------------------------------------------------------ account

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

    /// On this device: which device a link code would add.
    pub async fn link_inspect(&self, code: String) -> Result<LinkRequestInfo, PosternError> {
        let r = self.core.link_inspect(&code).await?;
        Ok(LinkRequestInfo { device_id: r.device_id, name: r.name })
    }

    /// On this device: add the device behind a link code to the account and
    /// every conversation.
    pub async fn link_approve(&self, code: String) -> Result<LinkRequestInfo, PosternError> {
        let r = self.core.link_approve(&code).await?;
        Ok(LinkRequestInfo { device_id: r.device_id, name: r.name })
    }

    /// The account's devices, oldest first.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, PosternError> {
        Ok(self
            .core
            .devices()
            .await?
            .into_iter()
            .map(|d| DeviceInfo { id: d.id, name: d.name, this_device: d.this_device })
            .collect())
    }

    /// Name one of the account's devices.
    pub async fn rename_device(&self, device: String, name: String) -> Result<(), PosternError> {
        Ok(self.core.rename_device(&device, &name).await?)
    }

    /// Remove another of the account's devices from it and every conversation.
    pub async fn remove_device(&self, device: String) -> Result<(), PosternError> {
        Ok(self.core.remove_device(&device).await?)
    }

    /// Give this device fresh keys in every conversation now.
    pub async fn refresh_keys(&self) -> Result<(), PosternError> {
        Ok(self.core.refresh_keys().await?)
    }

    /// Change the user's profile and share it with every conversation.
    pub async fn set_profile(&self, change: ProfileChange) -> Result<(), PosternError> {
        let update = core::ProfileUpdate {
            display_name: change.display_name,
            typing_template: change.typing_template,
            note: change.note,
            note_keep: change.note_keep,
        };
        Ok(self.core.set_profile(update).await?)
    }

    /// An app setting on this device, kept in the encrypted profile.
    pub async fn setting(&self, key: String) -> Result<Option<String>, PosternError> {
        Ok(self.core.setting(&key)?)
    }

    /// Set (or with `None` clear) an app setting on this device.
    pub async fn set_setting(&self, key: String, value: Option<String>) -> Result<(), PosternError> {
        Ok(self.core.set_setting(&key, value.as_deref()).await?)
    }

    // ------------------------------------------------------------ connection

    /// Stay connected until the app closes: run this once, without awaiting
    /// its end. It reconnects by itself.
    pub async fn run(&self) {
        self.core.run().await;
    }

    /// Whether the app is connected to the server now.
    pub async fn is_connected(&self) -> bool {
        self.core.is_connected().await
    }

    /// Fetch everything new now.
    pub async fn sync(&self) -> Result<(), PosternError> {
        self.core.connect().await?;
        Ok(self.core.sync().await?)
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

    // ------------------------------------------------------------ conversations

    /// Conversations, most recent first.
    pub async fn conversations(&self) -> Result<Vec<ConversationInfo>, PosternError> {
        Ok(self.core.conversations()?.into_iter().map(conversation).collect())
    }

    /// One conversation.
    pub async fn conversation(&self, id: String) -> Result<ConversationInfo, PosternError> {
        Ok(conversation(self.core.conversation(id.parse()?)?))
    }

    /// Open a DM with `username`, creating it if needed.
    pub async fn start_dm(&self, username: String) -> Result<ConversationInfo, PosternError> {
        Ok(conversation(self.core.start_dm(username.trim()).await?))
    }

    /// Create a group named `name` with these people, the user as admin.
    pub async fn create_group(&self, name: String, usernames: Vec<String>) -> Result<ConversationInfo, PosternError> {
        Ok(conversation(self.core.create_group(name.trim(), &usernames).await?))
    }

    /// Mark a conversation read, on all the user's devices.
    pub async fn mark_read(&self, conversation: String) -> Result<(), PosternError> {
        Ok(self.core.mark_read(conversation.parse()?).await?)
    }

    /// The members, the user first.
    pub async fn members(&self, conversation: String) -> Result<Vec<MemberInfo>, PosternError> {
        Ok(self
            .core
            .members(conversation.parse()?)
            .await?
            .into_iter()
            .map(|m| MemberInfo { username: m.username, shown_name: m.shown_name, admin: m.admin, devices: m.devices, you: m.you })
            .collect())
    }

    /// As an admin: add people to a group.
    pub async fn add_members(&self, conversation: String, usernames: Vec<String>) -> Result<(), PosternError> {
        Ok(self.core.add_members(conversation.parse()?, &usernames).await?)
    }

    /// As an admin: remove people from a group.
    pub async fn remove_members(&self, conversation: String, usernames: Vec<String>) -> Result<(), PosternError> {
        Ok(self.core.remove_members(conversation.parse()?, &usernames).await?)
    }

    /// As an admin: rename a group.
    pub async fn rename_group(&self, conversation: String, name: String) -> Result<(), PosternError> {
        Ok(self.core.rename_group(conversation.parse()?, name.trim()).await?)
    }

    /// As an admin: make someone an admin, or stop them being one.
    pub async fn set_admin(&self, conversation: String, username: String, admin: bool) -> Result<(), PosternError> {
        Ok(self.core.set_admin(conversation.parse()?, &username, admin).await?)
    }

    /// Leave a group, with all the user's devices.
    pub async fn leave_group(&self, conversation: String) -> Result<(), PosternError> {
        Ok(self.core.leave_group(conversation.parse()?).await?)
    }

    /// Set (or clear, with "") the user's name in one conversation.
    pub async fn set_conversation_name(&self, conversation: String, name: String) -> Result<(), PosternError> {
        Ok(self.core.set_conversation_name(conversation.parse()?, name.trim()).await?)
    }

    /// Notices about a conversation, or with `None` about the account, oldest
    /// first.
    pub async fn notices(&self, conversation: Option<String>, limit: u32) -> Result<Vec<NoticeInfo>, PosternError> {
        let id: Option<ConversationId> = conversation.map(|c| c.parse()).transpose()?;
        Ok(self
            .core
            .notices(id, limit)?
            .into_iter()
            .map(|n| NoticeInfo { at_ms: ms(n.at_ms), text: sentence(&n.text) })
            .collect())
    }

    // ------------------------------------------------------------ messages

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

    /// Edit one of the user's messages. Every version stays visible.
    pub async fn edit_message(&self, conversation: String, message_id: String, text: String) -> Result<MessageInfo, PosternError> {
        let (c, m): (ConversationId, MessageId) = (conversation.parse()?, message_id.parse()?);
        Ok(message(self.core.edit_message(c, m, &text).await?))
    }

    /// Delete one of the user's messages, with every version, for everyone.
    pub async fn delete_message(&self, conversation: String, message_id: String) -> Result<(), PosternError> {
        Ok(self.core.delete_message(conversation.parse()?, message_id.parse()?).await?)
    }

    /// Every version of a message, the original first.
    pub async fn message_versions(&self, conversation: String, message_id: String) -> Result<Vec<MessageVersionInfo>, PosternError> {
        Ok(self
            .core
            .message_versions(conversation.parse()?, message_id.parse()?)?
            .into_iter()
            .map(|v| MessageVersionInfo { version: v.version, body: v.body, at_ms: ms(v.at_ms), pending: v.pending })
            .collect())
    }

    /// Tell a conversation the user is typing. Call it on every keystroke;
    /// it sends at most every three seconds.
    pub async fn send_typing(&self, conversation: String) -> Result<(), PosternError> {
        Ok(self.core.send_typing(conversation.parse()?).await?)
    }

    // ------------------------------------------------------------ people

    /// What this device knows about someone.
    pub async fn person(&self, username: String) -> Result<PersonInfo, PosternError> {
        let p = self.core.person(&username).await?;
        Ok(PersonInfo {
            username: p.username,
            shown_name: p.shown_name,
            display_name: p.display_name,
            nickname: p.nickname,
            typing_template: p.typing_template,
            note: p.note,
            verified: p.verified,
        })
    }

    /// Set (or clear, with "") a private nickname for someone.
    pub async fn set_nickname(&self, username: String, nickname: String) -> Result<(), PosternError> {
        Ok(self.core.set_nickname(&username, nickname.trim()).await?)
    }

    /// The safety number between the user and `username`: 12 groups of 5
    /// digits, separated by spaces.
    pub async fn safety_number(&self, username: String) -> Result<String, PosternError> {
        Ok(self.core.safety_number(&username).await?)
    }

    /// Record that the user compared safety numbers with someone, or (false)
    /// withdraw it.
    pub async fn set_verified(&self, username: String, verified: bool) -> Result<(), PosternError> {
        Ok(self.core.set_verified(&username, verified).await?)
    }

    /// Someone's presence, as this device last heard it.
    pub async fn presence(&self, username: String) -> Result<PresenceState, PosternError> {
        Ok(match self.core.presence(&username).await? {
            core::Presence::Online => PresenceState::Online,
            core::Presence::Away => PresenceState::Away,
            core::Presence::DoNotDisturb => PresenceState::DoNotDisturb,
            core::Presence::Offline => PresenceState::Offline,
        })
    }

    /// The presence the user chose.
    pub async fn own_presence(&self) -> Result<PresenceChoice, PosternError> {
        Ok(match self.core.own_presence().await? {
            core::PresenceSetting::Online => PresenceChoice::Online,
            core::PresenceSetting::Away => PresenceChoice::Away,
            core::PresenceSetting::DoNotDisturb => PresenceChoice::DoNotDisturb,
            core::PresenceSetting::Invisible => PresenceChoice::Invisible,
        })
    }

    /// Choose the presence others see.
    pub async fn set_presence(&self, choice: PresenceChoice) -> Result<(), PosternError> {
        let setting = match choice {
            PresenceChoice::Online => core::PresenceSetting::Online,
            PresenceChoice::Away => core::PresenceSetting::Away,
            PresenceChoice::DoNotDisturb => core::PresenceSetting::DoNotDisturb,
            PresenceChoice::Invisible => core::PresenceSetting::Invisible,
        };
        Ok(self.core.set_presence(setting).await?)
    }
}

fn ms(t: u64) -> i64 {
    i64::try_from(t).unwrap_or(i64::MAX)
}

/// Core notices start in lower case, to read inside a longer line; the app
/// shows them on their own.
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => {
            let mut out: String = first.to_uppercase().chain(c).collect();
            if !out.ends_with(['.', '?', '!']) {
                out.push('.');
            }
            out
        }
        None => String::new(),
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
        last_activity_ms: ms(c.last_activity_ms),
        following: c.following,
        unread: c.unread,
        last_message: c.last_message.map(message),
    }
}

fn message(m: core::Message) -> MessageInfo {
    MessageInfo {
        id: m.id.to_string(),
        conversation: m.conversation.to_string(),
        sender: m.sender,
        sender_username: m.sender_username,
        outgoing: m.outgoing,
        sent_at_ms: ms(m.sent_at_ms),
        body: m.body,
        deleted: m.deleted,
        unsupported: m.unsupported,
        edited: m.edited,
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
        core::Event::Notice { conversation, text } => ev(EventKind::Notice, conversation, None, Some(sentence(&text))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_read_as_sentences() {
        assert_eq!(sentence("a message could not be sent: too big"), "A message could not be sent: too big.");
        assert_eq!(sentence("done!"), "Done!");
        assert_eq!(sentence(""), "");
    }
}
