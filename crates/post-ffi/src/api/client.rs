//! One Post profile, opened, as the app sees it.
//!
//! Types here are plain data the generator mirrors into Dart classes. Events
//! are one struct with a `kind` rather than a Rust enum with fields, so the
//! Dart side needs no extra code generation (`freezed`). IDs cross as hex
//! strings, times as Unix milliseconds.

use std::path::PathBuf;

use flutter_rust_bridge::frb;
use post_core::{self as core, ConversationId, Core, MessageId};

use super::error::{ErrorKind, PostError, from_anyhow};
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
    profile_dir: PathBuf,
    /// The database key is in the platform keystore (not a key file), so
    /// deleting the profile deletes it there too.
    keystore: bool,
    /// The update the last check found, for downloading and installing.
    update: std::sync::Mutex<Option<core::update::Available>>,
}

/// A newer release of the app, checked against the release key.
pub struct UpdateInfo {
    /// Its version.
    pub version: String,
    /// What changed, as plain text.
    pub notes: String,
    /// When it was released (Unix ms).
    pub released_at_ms: i64,
    /// How much there is to download, in bytes.
    pub size: u64,
    /// This app is too old to keep talking to the server once it updates:
    /// show it prominently. (Not `required`, a word Dart keeps for itself.)
    pub mandatory: bool,
    /// The app can install it itself: it runs from a release's install,
    /// which updates replace. A copy built from source never installs or
    /// registers itself; it can only say an update exists.
    pub can_install: bool,
}

/// A live call (design section 10).
pub struct CallInfo {
    /// Its ID.
    pub id: String,
    /// Its conversation's ID.
    pub conversation: String,
    /// Who started it.
    pub started_by: String,
    /// When (Unix ms).
    pub started_at_ms: i64,
    /// Started with video.
    pub video: bool,
    /// Who is in it now.
    pub participants: Vec<CallParticipantInfo>,
    /// It is calling the user.
    pub ringing: bool,
    /// This device is in it.
    pub joined: bool,
    /// It is over.
    pub ended: bool,
}

/// Someone in a call.
pub struct CallParticipantInfo {
    /// Their identity in LiveKit (hex).
    pub participant_id: String,
    /// Who they are.
    pub username: String,
    /// One of the user's own devices.
    pub you: bool,
    /// This device.
    pub this_device: bool,
}

/// What joining a call's room takes.
pub struct CallJoinInfo {
    /// The call, with this device in it.
    pub call: CallInfo,
    /// This device's identity in LiveKit (hex).
    pub participant_id: String,
    /// LiveKit's URL.
    pub url: String,
    /// A short-lived LiveKit token.
    pub token: String,
    /// The host's limit on what each participant sends, kbit/s.
    pub max_bitrate_kbps: u32,
    /// The host's limit on participants.
    pub max_participants: u32,
}

/// One participant's media key.
pub struct CallKey {
    /// Whose (their identity in LiveKit, hex).
    pub participant_id: String,
    /// 32 bytes.
    pub key: Vec<u8>,
}

/// A call's media keys now.
pub struct CallKeySet {
    /// The key-ring slot to install them in.
    pub key_index: u32,
    /// One per participant.
    pub keys: Vec<CallKey>,
}

/// How far a download has got.
pub struct UpdateProgress {
    /// Bytes downloaded.
    pub done: u64,
    /// Bytes in all.
    pub total: u64,
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
    /// How many of those mention the user.
    pub mentions: u32,
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
    /// Yours, and every other member has it: two ticks.
    pub delivered: bool,
    /// Yours, and every other member read it: two blue ticks.
    pub read: bool,
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
    /// How their messages arrive on screen: a preset's name, or empty for
    /// the default.
    pub arrival: String,
    /// Their profile picture's ID, for `picture`, once this device has it;
    /// until then, or with none, show the default.
    pub avatar: Option<String>,
    /// Their banner's ID, for `picture`, once this device has it.
    pub banner: Option<String>,
    /// The banner colour they chose, `0xRRGGBB`, if any.
    pub banner_color: Option<u32>,
    /// Their bio (about me), formatted like a message; empty if none.
    pub bio: String,
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
    /// What it is about, for its icon.
    pub kind: NoticeKind,
}

/// What a notice is about.
pub enum NoticeKind {
    /// A security warning, or something else about the conversation.
    General,
    /// A call started or ended.
    Call,
    /// A call that rang the user went unanswered.
    MissedCall,
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

/// A search: words (each the start of a word, any case or accents) and
/// filters, all of which must match.
pub struct SearchOptions {
    /// The words.
    pub text: String,
    /// Only in this conversation.
    pub conversation: Option<String>,
    /// Only sent by this username.
    pub from: Option<String>,
    /// Only messages that mention this username.
    pub mentions: Option<String>,
    /// Only messages with a link.
    pub has_link: bool,
    /// Only sent at or after this (Unix ms).
    pub after_ms: Option<u64>,
    /// Only sent before this (Unix ms).
    pub before_ms: Option<u64>,
    /// Oldest first, rather than newest.
    pub oldest_first: bool,
    /// At most this many: the page size.
    pub limit: u32,
    /// Skipping this many first: where the page starts.
    pub offset: u32,
}

/// What a search found.
pub struct SearchFound {
    /// How many match in all.
    pub total: u32,
    /// The page asked for.
    pub messages: Vec<MessageInfo>,
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
    /// How the user's messages arrive on screen: a preset's name.
    pub arrival: Option<String>,
    /// Bio (about me): up to `bio_limit()` characters, line breaks
    /// included, formatted like a message.
    pub bio: Option<String>,
}

/// Which profile picture.
#[derive(Clone, Copy)]
pub enum PictureKind {
    /// The round profile picture: square.
    Avatar,
    /// The banner across the top of a profile: 3 : 1.
    Banner,
}

impl From<PictureKind> for core::picture::PictureKind {
    fn from(k: PictureKind) -> Self {
        match k {
            PictureKind::Avatar => Self::Avatar,
            PictureKind::Banner => Self::Banner,
        }
    }
}

/// The part of an image to use, as fractions of its width and height (of
/// the image as shown, turned upright).
pub struct PictureCrop {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// How large a picture may be, for saying so in the app.
pub struct PictureLimits {
    /// Width in pixels.
    pub max_width: u32,
    /// Height in pixels.
    pub max_height: u32,
    /// Bytes.
    pub max_bytes: u32,
    /// Largest image file one may be made from, in bytes.
    pub max_source_bytes: u32,
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
    /// A call in `conversation` started, rang, changed or ended: read
    /// `calls()` for how things stand (an ended call is gone from it).
    Call,
    /// The server posted an announcement; `text` is a preview of it.
    Announcement,
    /// The announcements changed (new ones, read, the maintenance plan, or
    /// whether you are a host): read `announcement_summary()`.
    Announcements,
}

/// A single-use invite code a host made.
pub struct InviteInfo {
    /// The code. Shown once: the server keeps only its hash.
    pub code: String,
    /// The server address to sign up at with it.
    pub server: String,
    /// When it stops working (Unix ms).
    pub expires_at_ms: i64,
    /// It registers a bot account.
    pub bot: bool,
}

/// Who has one of the user's messages.
pub struct MessageReceiptsInfo {
    /// Every other member, by the name the user sees, sorted.
    pub people: Vec<PersonReceiptInfo>,
    /// Too large a conversation for receipts.
    pub too_large: bool,
    /// The user's own read receipts are off: deliveries only.
    pub read_receipts_off: bool,
}

/// How far one person has got with the user's message.
pub struct PersonReceiptInfo {
    /// Their username.
    pub username: String,
    /// The name the user sees for them in this conversation.
    pub shown_name: String,
    /// How far they have got.
    pub state: ReceiptState,
}

/// How far one person has got.
pub enum ReceiptState {
    /// Not on any of their devices yet.
    Waiting,
    /// On one of their devices.
    Delivered,
    /// Read.
    Read,
}

/// One of the server's announcements, shown as from the server.
pub struct AnnouncementInfo {
    /// Increasing ID.
    pub id: i64,
    /// When it was posted (Unix ms).
    pub at_ms: i64,
    /// What it is.
    pub kind: AnnouncementKind,
    /// A host's words, or for maintenance the reason given (may be empty).
    pub text: String,
    /// Maintenance: when it starts or started (Unix ms), to show in the
    /// reader's time zone.
    pub starts_at_ms: i64,
    /// Maintenance: how long it should take.
    pub minutes: u32,
}

/// What an announcement is. All but `Text` are posted by the server itself.
pub enum AnnouncementKind {
    /// Written by a host.
    Text,
    /// Maintenance was planned.
    MaintenancePlanned,
    /// The planned maintenance starts soon.
    MaintenanceReminder,
    /// Maintenance has started.
    MaintenanceStarted,
    /// Maintenance is over.
    MaintenanceOver,
    /// The planned maintenance was cancelled.
    MaintenanceCancelled,
    /// A kind from a newer server.
    Unsupported,
}

/// Maintenance planned or underway.
pub struct MaintenanceInfo {
    /// When it starts or started (Unix ms).
    pub starts_at_ms: i64,
    /// How long it should take.
    pub minutes: u32,
    /// Why, if given.
    pub reason: String,
    /// It has started.
    pub underway: bool,
}

/// The announcements channel at a glance.
pub struct AnnouncementSummary {
    /// The server's domain: what they are shown as from.
    pub server: String,
    /// Posted since the user last read them.
    pub unread: u32,
    /// The newest, for the channel's preview line.
    pub latest: Option<AnnouncementInfo>,
    /// The user is one of the server's hosts: show the composer.
    pub host: bool,
    /// Maintenance planned or underway.
    pub maintenance: Option<MaintenanceInfo>,
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
    pub async fn open(profile_dir: String, key_file: Option<String>) -> Result<Client, PostError> {
        let dir = PathBuf::from(profile_dir);
        let profile_dir = dir.clone();
        let keystore = key_file.is_none();
        let core = tokio::task::spawn_blocking(move || -> anyhow::Result<Core> {
            let key = match key_file {
                Some(f) => core::keyfile::read(std::path::Path::new(&f))?,
                None => crate::keystore::key_for(&dir)?,
            };
            Ok(Core::open(&dir, &key)?)
        })
        .await
        .map_err(|_| PostError::new(ErrorKind::Other, "Opening the profile stopped unexpectedly."))?
        .map_err(from_anyhow)?;
        Ok(Client { core, profile_dir, keystore, update: std::sync::Mutex::default() })
    }

    // ------------------------------------------------------------ logging out

    /// Log out: this device leaves the account, after sending what is
    /// queued, and the profile closes. Needs the server. Refused on the
    /// account's only device (check `devices()` first), which would lose
    /// the account. Then call `delete_profile`.
    pub async fn log_out(&self) -> Result<(), PostError> {
        Ok(self.core.log_out().await?)
    }

    /// Close the profile and delete it from this device: its database, its
    /// downloads, and its key in the keystore. After `log_out`, or alone to
    /// give up an account on its only device. The `Client` is finished
    /// afterwards; open a new one for the welcome screens.
    pub async fn delete_profile(&self) -> Result<(), PostError> {
        self.core.close().await;
        let dir = self.profile_dir.clone();
        let keystore = self.keystore;
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if keystore {
                crate::keystore::delete_profile(&dir)
            } else {
                Ok(core::delete_profile(&dir)?)
            }
        })
        .await
        .map_err(|_| PostError::new(ErrorKind::Other, "Deleting the profile stopped unexpectedly."))?
        .map_err(from_anyhow)
    }

    // ------------------------------------------------------------ updates

    async fn updater(&self, current_version: &str) -> Result<core::update::Updater, PostError> {
        let me = self.core.identity().await.ok_or(core::Error::NotRegistered)?;
        Ok(core::update::Updater::new(&me.server, current_version, self.profile_dir.join("updates"))?)
    }

    fn found_update(&self) -> Result<core::update::Available, PostError> {
        let found = self.update.lock().unwrap_or_else(|p| p.into_inner()).clone();
        found.ok_or_else(|| PostError::new(ErrorKind::Other, "Check for updates first."))
    }

    /// Ask this account's server for a release newer than `current_version`
    /// (this app's), checked against the release key built into the app.
    pub async fn check_update(&self, current_version: String) -> Result<Option<UpdateInfo>, PostError> {
        let found = self.updater(&current_version).await?.check().await?;
        *self.update.lock().unwrap_or_else(|p| p.into_inner()) = found.clone();
        Ok(found.map(|a| UpdateInfo {
            version: a.manifest.version.clone(),
            notes: a.manifest.notes.clone(),
            released_at_ms: ms(a.manifest.released_at_ms),
            size: a.size,
            mandatory: a.required,
            can_install: core::update::install_root().is_some(),
        }))
    }

    /// Download the update the last check found, resuming a download left
    /// part way; `sink` hears how far it has got.
    pub async fn download_update(&self, current_version: String, sink: StreamSink<UpdateProgress>) -> Result<(), PostError> {
        let found = self.found_update()?;
        let updater = self.updater(&current_version).await?;
        updater
            .download(&found, |done, total| {
                let _ = sink.add(UpdateProgress { done, total });
            })
            .await?;
        Ok(())
    }

    /// Install the downloaded update in place of this app's version. Gives
    /// the program to start instead of this one; the running app keeps
    /// running until it is restarted. Only a release's install can: a copy
    /// built from source never installs itself.
    pub async fn install_update(&self, current_version: String) -> Result<String, PostError> {
        let found = self.found_update()?;
        let root = core::update::install_root()
            .ok_or_else(|| PostError::new(ErrorKind::Other, "This copy of Post was built from source, so it does not install updates."))?;
        let updater = self.updater(&current_version).await?;
        let program = tokio::task::spawn_blocking(move || updater.install(&found, &root))
            .await
            .map_err(|_| PostError::new(ErrorKind::Other, "Installing stopped unexpectedly."))??;
        Ok(program.to_string_lossy().into_owned())
    }

    // ------------------------------------------------------------ account

    /// The account on this device, if there is one yet.
    pub async fn identity(&self) -> Option<Identity> {
        self.core.identity().await.map(identity)
    }

    /// Create an account with an invite.
    pub async fn register(&self, server: String, invite: String, username: String) -> Result<Identity, PostError> {
        Ok(identity(self.core.register(&server, &invite, &username, false).await?))
    }

    /// On a new device: offer it for linking to an existing account on
    /// `server`, as `device_name`. Returns the code to show; then call
    /// [`Client::finish_link`].
    pub async fn begin_link(&self, server: String, device_name: String) -> Result<String, PostError> {
        Ok(self.core.begin_link(&server, &device_name, false).await?)
    }

    /// On the new device: wait (up to ten minutes) for the code to be
    /// approved on another device, then join the account.
    pub async fn finish_link(&self) -> Result<Identity, PostError> {
        Ok(identity(self.core.finish_link().await?))
    }

    /// On this device: which device a link code would add.
    pub async fn link_inspect(&self, code: String) -> Result<LinkRequestInfo, PostError> {
        let r = self.core.link_inspect(&code).await?;
        Ok(LinkRequestInfo { device_id: r.device_id, name: r.name })
    }

    /// On this device: add the device behind a link code to the account and
    /// every conversation.
    pub async fn link_approve(&self, code: String) -> Result<LinkRequestInfo, PostError> {
        let r = self.core.link_approve(&code).await?;
        Ok(LinkRequestInfo { device_id: r.device_id, name: r.name })
    }

    /// The account's devices, oldest first.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, PostError> {
        Ok(self
            .core
            .devices()
            .await?
            .into_iter()
            .map(|d| DeviceInfo { id: d.id, name: d.name, this_device: d.this_device })
            .collect())
    }

    /// Name one of the account's devices.
    pub async fn rename_device(&self, device: String, name: String) -> Result<(), PostError> {
        Ok(self.core.rename_device(&device, &name).await?)
    }

    /// Remove another of the account's devices from it and every conversation.
    pub async fn remove_device(&self, device: String) -> Result<(), PostError> {
        Ok(self.core.remove_device(&device).await?)
    }

    /// Give this device fresh keys in every conversation now.
    pub async fn refresh_keys(&self) -> Result<(), PostError> {
        Ok(self.core.refresh_keys().await?)
    }

    /// Change the user's profile and share it with every conversation.
    pub async fn set_profile(&self, change: ProfileChange) -> Result<(), PostError> {
        let update = core::ProfileUpdate {
            display_name: change.display_name,
            typing_template: change.typing_template,
            note: change.note,
            note_keep: change.note_keep,
            arrival: change.arrival,
            banner_color: None,
            bio: change.bio,
        };
        Ok(self.core.set_profile(update).await?)
    }

    /// Set the banner colour shown when there is no banner image
    /// (`0xRRGGBB`), or with `None` leave it to the app.
    pub async fn set_banner_color(&self, color: Option<u32>) -> Result<(), PostError> {
        let update = core::ProfileUpdate { banner_color: Some(color), ..Default::default() };
        Ok(self.core.set_profile(update).await?)
    }

    /// Set the user's profile picture or banner from an image file: the
    /// `crop` part of it, else its middle. It is shrunk to fit, encrypted,
    /// uploaded and shared. Needs a connection.
    pub async fn set_picture(&self, kind: PictureKind, path: String, crop: Option<PictureCrop>) -> Result<(), PostError> {
        let unreadable = |e: std::io::Error| PostError::new(ErrorKind::InvalidInput, format!("Could not read that file ({e})."));
        let size = tokio::fs::metadata(&path).await.map_err(unreadable)?.len();
        if size > core::picture::MAX_SOURCE_BYTES as u64 {
            let mb = core::picture::MAX_SOURCE_BYTES / 1024 / 1024;
            return Err(PostError::new(ErrorKind::InvalidInput, format!("That image is over {mb} MB. Pick a smaller one.")));
        }
        let bytes = tokio::fs::read(&path).await.map_err(unreadable)?;
        let crop = crop.map(|c| core::picture::Crop { x: c.x, y: c.y, width: c.width, height: c.height });
        Ok(self.core.set_picture(kind.into(), Some((bytes, crop))).await?)
    }

    /// Remove the user's profile picture or banner.
    pub async fn remove_picture(&self, kind: PictureKind) -> Result<(), PostError> {
        Ok(self.core.set_picture(kind.into(), None).await?)
    }

    /// How large a picture of `kind` may be.
    #[frb(sync)]
    pub fn picture_limits(kind: PictureKind) -> PictureLimits {
        let k: core::picture::PictureKind = kind.into();
        let (max_width, max_height) = k.max_size();
        PictureLimits {
            max_width,
            max_height,
            max_bytes: k.max_bytes() as u32,
            max_source_bytes: core::picture::MAX_SOURCE_BYTES as u32,
        }
    }

    /// Longest bio, in characters (Unicode scalar values: Dart's runes),
    /// line breaks included.
    #[frb(sync)]
    pub fn bio_limit() -> u32 {
        core::MAX_BIO as u32
    }

    /// An app setting on this device, kept in the encrypted profile.
    pub async fn setting(&self, key: String) -> Result<Option<String>, PostError> {
        Ok(self.core.setting(&key)?)
    }

    /// Set (or with `None` clear) an app setting on this device.
    pub async fn set_setting(&self, key: String, value: Option<String>) -> Result<(), PostError> {
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
    pub async fn sync(&self) -> Result<(), PostError> {
        self.core.connect().await?;
        Ok(self.core.sync().await?)
    }

    /// Live events, until the Dart stream is closed or the profile is.
    pub async fn events(&self, sink: StreamSink<AppEvent>) {
        let mut rx = self.core.subscribe();
        loop {
            let next = tokio::select! {
                () = self.core.closed() => return,
                next = rx.recv() => next,
            };
            match next {
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

    // ------------------------------------------------------------ announcements

    /// The announcements channel at a glance.
    pub async fn announcement_summary(&self) -> Result<AnnouncementSummary, PostError> {
        let s = self.core.announcement_summary()?;
        Ok(AnnouncementSummary {
            server: s.server,
            unread: s.unread,
            latest: s.latest.map(announcement),
            host: s.host,
            maintenance: s.maintenance.map(|m| MaintenanceInfo {
                starts_at_ms: ms(m.starts_at_ms),
                minutes: m.minutes,
                reason: m.reason,
                underway: m.underway,
            }),
        })
    }

    /// The newest `limit` announcements, oldest first.
    pub async fn announcements(&self, limit: u32) -> Result<Vec<AnnouncementInfo>, PostError> {
        Ok(self.core.announcements(limit)?.into_iter().map(announcement).collect())
    }

    /// Mark the announcements read, on all the user's devices.
    pub async fn mark_announcements_read(&self) -> Result<(), PostError> {
        Ok(self.core.mark_announcements_read().await?)
    }

    /// As a host: post an announcement to everyone on the server.
    pub async fn announce(&self, text: String) -> Result<(), PostError> {
        Ok(self.core.announce(&text).await?)
    }

    /// As a host: plan maintenance. `starts_at_ms` 0 starts it now.
    pub async fn plan_maintenance(&self, starts_at_ms: i64, minutes: u32, reason: String, quiet: bool) -> Result<(), PostError> {
        let starts = u64::try_from(starts_at_ms).map_err(|_| PostError::new(ErrorKind::InvalidInput, "That time is not valid."))?;
        Ok(self.core.plan_maintenance(starts, minutes, &reason, quiet).await?)
    }

    /// As a host: make a single-use invite code, valid for `days`.
    pub async fn create_invite(&self, bot: bool, days: u32) -> Result<InviteInfo, PostError> {
        let i = self.core.create_invite(bot, days).await?;
        Ok(InviteInfo { code: i.code, server: i.server, expires_at_ms: ms(i.expires_at_ms), bot: i.bot })
    }

    /// As a host: cancel the planned maintenance, or announce it is over.
    pub async fn end_maintenance(&self) -> Result<(), PostError> {
        Ok(self.core.end_maintenance().await?)
    }

    // ------------------------------------------------------------ conversations

    /// Conversations, most recent first.
    pub async fn conversations(&self) -> Result<Vec<ConversationInfo>, PostError> {
        Ok(self.core.conversations()?.into_iter().map(conversation).collect())
    }

    /// One conversation.
    pub async fn conversation(&self, id: String) -> Result<ConversationInfo, PostError> {
        Ok(conversation(self.core.conversation(id.parse()?)?))
    }

    /// Open a DM with `username`, creating it if needed.
    pub async fn start_dm(&self, username: String) -> Result<ConversationInfo, PostError> {
        Ok(conversation(self.core.start_dm(username.trim()).await?))
    }

    /// Create a group named `name` with these people, the user as admin.
    pub async fn create_group(&self, name: String, usernames: Vec<String>) -> Result<ConversationInfo, PostError> {
        Ok(conversation(self.core.create_group(name.trim(), &usernames).await?))
    }

    /// Mark a conversation read, on all the user's devices.
    pub async fn mark_read(&self, conversation: String) -> Result<(), PostError> {
        Ok(self.core.mark_read(conversation.parse()?).await?)
    }

    /// The members, the user first.
    pub async fn members(&self, conversation: String) -> Result<Vec<MemberInfo>, PostError> {
        Ok(self
            .core
            .members(conversation.parse()?)
            .await?
            .into_iter()
            .map(|m| MemberInfo { username: m.username, shown_name: m.shown_name, admin: m.admin, devices: m.devices, you: m.you })
            .collect())
    }

    /// As an admin: add people to a group.
    pub async fn add_members(&self, conversation: String, usernames: Vec<String>) -> Result<(), PostError> {
        Ok(self.core.add_members(conversation.parse()?, &usernames).await?)
    }

    /// As an admin: remove people from a group.
    pub async fn remove_members(&self, conversation: String, usernames: Vec<String>) -> Result<(), PostError> {
        Ok(self.core.remove_members(conversation.parse()?, &usernames).await?)
    }

    /// As an admin: rename a group.
    pub async fn rename_group(&self, conversation: String, name: String) -> Result<(), PostError> {
        Ok(self.core.rename_group(conversation.parse()?, name.trim()).await?)
    }

    /// As an admin: make someone an admin, or stop them being one.
    pub async fn set_admin(&self, conversation: String, username: String, admin: bool) -> Result<(), PostError> {
        Ok(self.core.set_admin(conversation.parse()?, &username, admin).await?)
    }

    /// Leave a group, with all the user's devices.
    pub async fn leave_group(&self, conversation: String) -> Result<(), PostError> {
        Ok(self.core.leave_group(conversation.parse()?).await?)
    }

    /// Set (or clear, with "") the user's name in one conversation.
    pub async fn set_conversation_name(&self, conversation: String, name: String) -> Result<(), PostError> {
        Ok(self.core.set_conversation_name(conversation.parse()?, name.trim()).await?)
    }

    /// Notices about a conversation, or with `None` about the account, oldest
    /// first.
    pub async fn notices(&self, conversation: Option<String>, limit: u32) -> Result<Vec<NoticeInfo>, PostError> {
        let id: Option<ConversationId> = conversation.map(|c| c.parse()).transpose()?;
        Ok(self
            .core
            .notices(id, limit)?
            .into_iter()
            .map(|n| NoticeInfo {
                at_ms: ms(n.at_ms),
                text: sentence(&n.text),
                kind: match n.kind {
                    core::NoticeKind::General => NoticeKind::General,
                    core::NoticeKind::Call => NoticeKind::Call,
                    core::NoticeKind::MissedCall => NoticeKind::MissedCall,
                },
            })
            .collect())
    }

    // ------------------------------------------------------------ messages

    /// The newest `limit` messages of a conversation, oldest first.
    pub async fn messages(&self, conversation: String, limit: u32) -> Result<Vec<MessageInfo>, PostError> {
        let id: ConversationId = conversation.parse()?;
        Ok(self.core.messages(id, limit)?.into_iter().map(message).collect())
    }

    /// Messages matching a search, from this device's index: how many in
    /// all, and the page asked for.
    pub async fn search(&self, query: SearchOptions) -> Result<SearchFound, PostError> {
        let conversation = query.conversation.map(|c| c.parse::<ConversationId>()).transpose()?;
        let q = core::SearchQuery {
            text: query.text,
            conversation,
            from: query.from,
            mentions: query.mentions,
            has_link: query.has_link,
            after_ms: query.after_ms,
            before_ms: query.before_ms,
            oldest_first: query.oldest_first,
            limit: query.limit,
            offset: query.offset,
        };
        let found = self.core.search(&q)?;
        Ok(SearchFound { total: found.total, messages: found.messages.into_iter().map(message).collect() })
    }

    /// Send a text message.
    pub async fn send_text(&self, conversation: String, text: String) -> Result<MessageInfo, PostError> {
        let id: ConversationId = conversation.parse()?;
        Ok(message(self.core.send_text(id, &text).await?))
    }

    /// Edit one of the user's messages. Every version stays visible.
    pub async fn edit_message(&self, conversation: String, message_id: String, text: String) -> Result<MessageInfo, PostError> {
        let (c, m): (ConversationId, MessageId) = (conversation.parse()?, message_id.parse()?);
        Ok(message(self.core.edit_message(c, m, &text).await?))
    }

    /// Delete one of the user's messages, with every version, for everyone.
    pub async fn delete_message(&self, conversation: String, message_id: String) -> Result<(), PostError> {
        Ok(self.core.delete_message(conversation.parse()?, message_id.parse()?).await?)
    }

    /// Every version of a message, the original first.
    pub async fn message_versions(&self, conversation: String, message_id: String) -> Result<Vec<MessageVersionInfo>, PostError> {
        Ok(self
            .core
            .message_versions(conversation.parse()?, message_id.parse()?)?
            .into_iter()
            .map(|v| MessageVersionInfo { version: v.version, body: v.body, at_ms: ms(v.at_ms), pending: v.pending })
            .collect())
    }

    /// Tell a conversation the user is typing. Call it on every keystroke;
    /// it sends at most every three seconds.
    pub async fn send_typing(&self, conversation: String) -> Result<(), PostError> {
        Ok(self.core.send_typing(conversation.parse()?).await?)
    }

    // ------------------------------------------------------------ calls

    /// Start a call in a conversation, with this device in it, ringing
    /// `ring` (usernames), or with none named everyone in a DM or small group.
    pub async fn start_call(&self, conversation: String, video: bool, ring: Vec<String>) -> Result<CallJoinInfo, PostError> {
        Ok(call_join(self.core.start_call(conversation.parse()?, video, &ring).await?))
    }

    /// Ring people (usernames) into a live call.
    pub async fn ring_call(&self, conversation: String, call: String, usernames: Vec<String>) -> Result<(), PostError> {
        Ok(self.core.ring_call(conversation.parse()?, call.parse()?, &usernames).await?)
    }

    /// Join (or answer) a live call from this device.
    pub async fn join_call(&self, conversation: String, call: String) -> Result<CallJoinInfo, PostError> {
        Ok(call_join(self.core.join_call(conversation.parse()?, call.parse()?).await?))
    }

    /// Leave a call this device is in; in a DM this ends it.
    pub async fn leave_call(&self, conversation: String, call: String) -> Result<(), PostError> {
        Ok(self.core.leave_call(conversation.parse()?, call.parse()?).await?)
    }

    /// Decline a ringing call.
    pub async fn decline_call(&self, conversation: String, call: String) -> Result<(), PostError> {
        Ok(self.core.decline_call(conversation.parse()?, call.parse()?).await?)
    }

    /// The keys to install for a call's media now: fetch again on call and
    /// conversation events, and install them at `key_index`.
    pub async fn call_keys(&self, conversation: String, call: String) -> Result<CallKeySet, PostError> {
        let k = self.core.call_keys(conversation.parse()?, call.parse()?).await?;
        Ok(CallKeySet {
            key_index: k.key_index,
            keys: k.keys.into_iter().map(|(participant_id, key)| CallKey { participant_id, key: key.to_vec() }).collect(),
        })
    }

    /// The live calls, in every conversation.
    pub async fn calls(&self) -> Result<Vec<CallInfo>, PostError> {
        Ok(self.core.calls().await?.into_iter().map(call_info).collect())
    }

    // ------------------------------------------------------------ people

    /// What this device knows about someone.
    pub async fn person(&self, username: String) -> Result<PersonInfo, PostError> {
        let p = self.core.person(&username).await?;
        Ok(PersonInfo {
            username: p.username,
            shown_name: p.shown_name,
            display_name: p.display_name,
            nickname: p.nickname,
            typing_template: p.typing_template,
            note: p.note,
            verified: p.verified,
            arrival: p.arrival,
            avatar: p.avatar,
            banner: p.banner,
            banner_color: p.banner_color,
            bio: p.bio,
        })
    }

    /// A profile picture or banner this device has, by the ID a person
    /// gives for it: the image file's bytes (PNG or JPEG), already checked.
    pub fn picture(&self, id: String) -> Result<Option<Vec<u8>>, PostError> {
        Ok(self.core.picture(&id)?)
    }

    /// A picture this device has, scaled to exactly `width`×`height` pixels
    /// (the device pixels it will fill), cropped from its middle to that
    /// shape: RGBA bytes, row by row. Shrunk well, once, here, so the app
    /// draws it pixel for pixel.
    pub async fn picture_scaled(&self, id: String, width: u32, height: u32) -> Result<Option<Vec<u8>>, PostError> {
        Ok(self.core.picture_scaled(&id, width, height).await?)
    }

    /// Set (or clear, with "") a private nickname for someone.
    pub async fn set_nickname(&self, username: String, nickname: String) -> Result<(), PostError> {
        Ok(self.core.set_nickname(&username, nickname.trim()).await?)
    }

    /// The safety number between the user and `username`: 12 groups of 5
    /// digits, separated by spaces.
    pub async fn safety_number(&self, username: String) -> Result<String, PostError> {
        Ok(self.core.safety_number(&username).await?)
    }

    /// Record that the user compared safety numbers with someone, or (false)
    /// withdraw it.
    pub async fn set_verified(&self, username: String, verified: bool) -> Result<(), PostError> {
        Ok(self.core.set_verified(&username, verified).await?)
    }

    /// Who has one of the user's messages: each other member, and how far.
    pub async fn message_receipts(&self, conversation: String, message: String) -> Result<MessageReceiptsInfo, PostError> {
        let r = self.core.message_receipts(conversation.parse()?, message.parse()?).await?;
        Ok(MessageReceiptsInfo {
            people: r
                .people
                .into_iter()
                .map(|p| PersonReceiptInfo {
                    username: p.username,
                    shown_name: p.shown_name,
                    state: match p.state {
                        core::ReceiptState::Waiting => ReceiptState::Waiting,
                        core::ReceiptState::Delivered => ReceiptState::Delivered,
                        core::ReceiptState::Read => ReceiptState::Read,
                    },
                })
                .collect(),
            too_large: r.too_large,
            read_receipts_off: r.read_receipts_off,
        })
    }

    /// Whether the user sends read receipts (on unless turned off).
    pub async fn read_receipts(&self) -> Result<bool, PostError> {
        Ok(self.core.read_receipts()?)
    }

    /// Send read receipts or stop, on all the user's devices. Off, the user
    /// also stops seeing when others read theirs.
    pub async fn set_read_receipts(&self, enabled: bool) -> Result<(), PostError> {
        Ok(self.core.set_read_receipts(enabled).await?)
    }

    /// Someone's presence, as this device last heard it.
    pub async fn presence(&self, username: String) -> Result<PresenceState, PostError> {
        Ok(match self.core.presence(&username).await? {
            core::Presence::Online => PresenceState::Online,
            core::Presence::Away => PresenceState::Away,
            core::Presence::DoNotDisturb => PresenceState::DoNotDisturb,
            core::Presence::Offline => PresenceState::Offline,
        })
    }

    /// The presence the user chose.
    pub async fn own_presence(&self) -> Result<PresenceChoice, PostError> {
        Ok(match self.core.own_presence().await? {
            core::PresenceSetting::Online => PresenceChoice::Online,
            core::PresenceSetting::Away => PresenceChoice::Away,
            core::PresenceSetting::DoNotDisturb => PresenceChoice::DoNotDisturb,
            core::PresenceSetting::Invisible => PresenceChoice::Invisible,
        })
    }

    /// Choose the presence others see.
    pub async fn set_presence(&self, choice: PresenceChoice) -> Result<(), PostError> {
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

fn call_info(c: core::Call) -> CallInfo {
    CallInfo {
        id: c.id.to_string(),
        conversation: c.conversation.to_string(),
        started_by: c.started_by,
        started_at_ms: ms(c.started_at_ms),
        video: c.video,
        participants: c
            .participants
            .into_iter()
            .map(|p| CallParticipantInfo { participant_id: p.participant_id, username: p.username, you: p.you, this_device: p.this_device })
            .collect(),
        ringing: c.ringing,
        joined: c.joined,
        ended: c.ended,
    }
}

fn call_join(j: core::CallJoin) -> CallJoinInfo {
    CallJoinInfo {
        call: call_info(j.call),
        participant_id: j.participant_id,
        url: j.url,
        token: j.token,
        max_bitrate_kbps: j.max_bitrate_kbps,
        max_participants: j.max_participants,
    }
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
        mentions: c.mentions,
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
        delivered: matches!(m.state, core::MessageState::Delivered | core::MessageState::Read),
        read: m.state == core::MessageState::Read,
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
        core::Event::CallChanged(c) => ev(EventKind::Call, Some(c.conversation), None, None),
        core::Event::AnnouncementAdded(a) => ev(EventKind::Announcement, None, None, Some(a.text)),
        core::Event::AnnouncementsChanged(_) => ev(EventKind::Announcements, None, None, None),
    }
}

fn announcement(a: core::Announcement) -> AnnouncementInfo {
    use core::AnnouncementKind as K;
    AnnouncementInfo {
        id: ms(a.id),
        at_ms: ms(a.at_ms),
        kind: match a.kind {
            K::Text => AnnouncementKind::Text,
            K::MaintenancePlanned => AnnouncementKind::MaintenancePlanned,
            K::MaintenanceReminder => AnnouncementKind::MaintenanceReminder,
            K::MaintenanceStarted => AnnouncementKind::MaintenanceStarted,
            K::MaintenanceOver => AnnouncementKind::MaintenanceOver,
            K::MaintenanceCancelled => AnnouncementKind::MaintenanceCancelled,
            K::Unsupported => AnnouncementKind::Unsupported,
        },
        text: a.text,
        starts_at_ms: ms(a.starts_at_ms),
        minutes: a.minutes,
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
