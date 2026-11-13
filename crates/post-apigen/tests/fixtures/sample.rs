//! A stand-in for post-core, covering every row of the type table and every
//! annotation (docs/phase-1-api.md, section 5). The generator parses it; it
//! is never compiled.

use post_api_macros::api;

/// Not part of the API: no #[api].
pub struct Internal {
    pub secret: Vec<u8>,
}

/// One of this account's devices.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Its ID, hex.
    pub id: String,
    /// What the user called it.
    pub name: String,
    /// This is the device in use.
    pub this_device: bool,
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
    /// When it was used, if it has been.
    #[api(unix_ms)]
    pub used_at_ms: Option<u64>,
    /// How many days it was made for.
    pub days: u32,
    /// Made for a bot account.
    ///
    /// Bots cannot be admins.
    #[api(default)]
    pub bot: bool,
}

/// A message in a conversation.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// Its ID.
    pub id: MessageId,
    /// Its conversation.
    pub conversation: ConversationId,
    /// The call it announces, if any.
    pub call: Option<CallId>,
    /// Who sent it.
    pub sender: String,
    /// Its text, if it has not been deleted.
    pub text: Option<String>,
    /// Who it mentions.
    pub mentions: Vec<String>,
    /// Earlier versions of it, newest first.
    #[api(default)]
    pub versions: Vec<Message>,
    /// Its sender's presence when it arrived.
    pub presence: Presence,
    /// How its sender fills their banner, if they do.
    pub banner: Option<Fill>,
    /// Who has read it, grouped.
    pub receipts: Vec<Vec<String>>,
    /// The kinds of picture it carries.
    pub pictures: Vec<kinds::PictureKind>,
}

/// What others see of someone.
#[api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Not connected.
    Offline,
    /// Connected and active.
    Online,
    /// Connected but idle.
    Away,
    /// Connected; do not notify.
    DoNotDisturb,
}

/// A colour, or a gradient between two at an angle.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fill {
    /// One colour.
    Solid {
        /// 0xRRGGBB.
        colour: u32,
    },
    /// Two colours.
    Gradient {
        /// The first, 0xRRGGBB.
        from: u32,
        /// The second, 0xRRGGBB.
        to: u32,
        /// Degrees clockwise from up.
        angle: u32,
    },
}

/// Something that happened, for live views.
#[api]
#[derive(Clone, Debug)]
pub enum Event {
    /// A message arrived or changed.
    MessageAdded {
        /// The message.
        message: Message,
    },
    /// A message was deleted.
    MessageDeleted {
        /// Its conversation.
        conversation: ConversationId,
        /// The message.
        message_id: MessageId,
    },
    /// Someone's presence changed.
    Presence {
        /// Whose.
        username: String,
        /// What it is now.
        presence: Presence,
        /// Until when, if they set an end (Unix ms).
        #[api(unix_ms)]
        until_ms: Option<u64>,
    },
    /// This account's devices changed.
    DevicesChanged,
    /// The subscriber fell behind and missed some events.
    Lagged {
        /// How many.
        missed: u64,
    },
}

/// The client.
pub struct Core {
    db: Db,
}

impl Core {
    /// Not part of the API: no #[api].
    pub fn open() -> Self {
        todo!()
    }

    /// This account's devices, oldest first.
    #[api(needs = local)]
    pub async fn devices(&self) -> Result<Vec<Device>> {
        todo!()
    }

    /// Name one of this account's devices.
    #[api(needs = connection)]
    pub async fn rename_device(
        &self,
        /// The device's ID.
        device: &str,
        /// Its new name.
        name: &str,
    ) -> Result<()> {
        todo!()
    }

    /// Make a single-use invite code for someone to create an account with
    /// (hosts only), valid for `days`.
    ///
    /// The code is shown once.
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
        todo!()
    }

    /// Make someone an admin of a group, or stop them being one.
    #[api(needs = connection)]
    pub async fn set_admin(
        &self,
        /// The group.
        conversation: ConversationId,
        /// The member's username.
        username: &str,
        /// True to make them an admin, false to remove the role.
        admin: bool,
    ) -> Result<()> {
        todo!()
    }

    /// A conversation's messages, newest last.
    #[api(needs = local)]
    pub fn messages(
        &self,
        /// The conversation.
        conversation: ConversationId,
        /// Only messages before this one.
        before: Option<MessageId>,
        /// At most this many.
        #[api(default = 50)]
        limit: u32,
        /// Only messages sent after this (Unix ms).
        #[api(unix_ms)]
        since_ms: Option<u64>,
    ) -> Result<Vec<Message>> {
        todo!()
    }

    /// Messages matching a search.
    #[api(needs = local)]
    pub fn search(
        &self,
        /// Words to find, if any.
        query: Option<&str>,
        /// Only from these people.
        senders: &[String],
        /// Only in these conversations, if given.
        conversations: Option<&[ConversationId]>,
        /// Also with these tags.
        #[api(default)]
        tags: Vec<String>,
    ) -> Result<Vec<Message>> {
        todo!()
    }

    /// Set or clear the banner's fill.
    #[api(needs = connection)]
    pub async fn set_banner_fill(
        &self,
        /// The fill, or none to clear it.
        fill: Option<&Fill>,
    ) -> Result<()> {
        todo!()
    }

    /// Set the profile's colours.
    #[api(needs = connection)]
    pub async fn set_theme(
        &self,
        /// The fill.
        theme: &Fill,
        /// What kind of picture it frames.
        kind: kinds::PictureKind,
        /// Its presence for now.
        presence: Presence,
    ) -> Result<()> {
        todo!()
    }

    /// Whether the client is connected.
    #[api(needs = local)]
    pub fn is_connected(&self) -> bool {
        todo!()
    }

    /// Forget what is cached.
    #[api(needs = local)]
    pub fn clear_cache(&self) {}

    /// This account's username, if it has one yet.
    #[api(needs = local)]
    pub fn identity(&self) -> Result<Option<String>> {
        todo!()
    }

    /// How many messages are stored.
    #[api(needs = local)]
    pub fn message_count(&self) -> u64 {
        todo!()
    }

    /// A greeting.
    #[api(needs = local)]
    pub fn greet(
        &self,
        /// Who to greet.
        #[api(default = "you")]
        name: String,
        /// How often.
        #[api(default = 1)]
        times: u64,
        /// Shout it.
        #[api(default = false)]
        loud: bool,
    ) -> String {
        todo!()
    }

    /// The time on the server's clock for one on this device's (Unix ms).
    #[api(needs = connection)]
    pub async fn server_time(
        &self,
        /// This device's time (Unix ms).
        #[api(unix_ms)]
        at_ms: u64,
    ) -> Result<u64> {
        todo!()
    }

    /// Start a direct conversation.
    #[api(needs = connection)]
    pub async fn start_dm(
        &self,
        /// Who with.
        username: String,
    ) -> Result<ConversationId> {
        todo!()
    }

    /// Create the account.
    #[api(needs = registration)]
    pub async fn register(
        &self,
        /// The server's address.
        server: &str,
        /// The invite code.
        invite: &str,
    ) -> Result<()> {
        todo!()
    }
}

/// A trait impl: never part of the API.
impl Default for Core {
    fn default() -> Self {
        todo!()
    }
}
