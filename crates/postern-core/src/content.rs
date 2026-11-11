//! What goes inside an MLS application message (design section 7).
//!
//! Decoding is deliberately lenient about unknown fields and body types, so a
//! newer client's message shows as "unsupported, update the app" instead of
//! failing. That is safe here because nothing about identity or authority is
//! taken from the content: the sender comes from the MLS leaf that sent it.

#![allow(missing_docs)]

use prost::{Message, Oneof};

#[derive(Clone, PartialEq, Message)]
pub struct Content {
    /// 16 random bytes. Must equal the log entry's `client_msg_id`.
    #[prost(bytes = "vec", tag = "1")]
    pub id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub sent_at_ms: u64,
    /// The sender's device chain head, for chain-head gossip (design section 5).
    #[prost(message, optional, tag = "3")]
    pub sender_chain: Option<ChainHead>,
    #[prost(oneof = "Body", tags = "10, 13, 14, 17, 18, 30")]
    pub body: Option<Body>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ChainHead {
    #[prost(uint64, tag = "1")]
    pub seq: u64,
    #[prost(bytes = "vec", tag = "2")]
    pub head: Vec<u8>,
}

#[derive(Clone, PartialEq, Oneof)]
pub enum Body {
    #[prost(message, tag = "10")]
    Text(Text),
    #[prost(message, tag = "13")]
    Edit(Edit),
    #[prost(message, tag = "14")]
    Delete(Delete),
    #[prost(message, tag = "17")]
    Profile(Profile),
    #[prost(message, tag = "18")]
    ConversationName(ConversationName),
    /// Self group only.
    #[prost(message, tag = "30")]
    SelfSync(SelfSync),
}

/// What an account's devices keep in step, sent only in its self group
/// (design section 6). Each entry sets one value; an empty name clears it.
#[derive(Clone, PartialEq, Message)]
pub struct SelfSync {
    #[prost(message, repeated, tag = "1")]
    pub nicknames: Vec<NicknameEntry>,
    #[prost(message, repeated, tag = "2")]
    pub device_names: Vec<DeviceNameEntry>,
    /// A full copy, sent when a device joins the self group. Receivers take
    /// from it only values they lack, so it never undoes a newer change.
    #[prost(bool, tag = "3")]
    pub snapshot: bool,
    /// People whose safety number the user compared, or stopped trusting.
    #[prost(message, repeated, tag = "4")]
    pub verified: Vec<VerifiedEntry>,
    /// How far the user has read conversations. Positions only move forward,
    /// so these merge by taking the larger, in any order.
    #[prost(message, repeated, tag = "5")]
    pub read: Vec<ReadEntry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct VerifiedEntry {
    /// 32-byte account ID.
    #[prost(bytes = "vec", tag = "1")]
    pub account: Vec<u8>,
    #[prost(bool, tag = "2")]
    pub verified: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct ReadEntry {
    /// 16-byte group ID.
    #[prost(bytes = "vec", tag = "1")]
    pub group: Vec<u8>,
    /// Read up to and including this position in the group's log.
    #[prost(uint64, tag = "2")]
    pub seq: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct NicknameEntry {
    /// 32-byte account ID.
    #[prost(bytes = "vec", tag = "1")]
    pub account: Vec<u8>,
    #[prost(string, tag = "2")]
    pub name: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct DeviceNameEntry {
    /// 16-byte device ID.
    #[prost(bytes = "vec", tag = "1")]
    pub device: Vec<u8>,
    #[prost(string, tag = "2")]
    pub name: String,
}

/// The sender's profile (design section 7). Sent into every conversation when
/// it changes; the highest version per account wins.
#[derive(Clone, PartialEq, Message)]
pub struct Profile {
    #[prost(uint64, tag = "1")]
    pub version: u64,
    /// Global display name; empty means show the username.
    #[prost(string, tag = "2")]
    pub display_name: String,
    // Tag 3 is the avatar, added with attachments in phase 6.
    /// Typing template containing `$$NAME$$`; empty means the standard text.
    #[prost(string, tag = "4")]
    pub typing_template: String,
    #[prost(message, optional, tag = "5")]
    pub note: Option<Note>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Note {
    #[prost(string, tag = "1")]
    pub text: String,
    #[prost(uint64, tag = "2")]
    pub set_at_ms: u64,
    /// 0 means no expiry.
    #[prost(uint64, tag = "3")]
    pub expires_at_ms: u64,
}

/// The sender's display name in this conversation only. Empty clears it.
#[derive(Clone, PartialEq, Message)]
pub struct ConversationName {
    #[prost(string, tag = "1")]
    pub name: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct Text {
    #[prost(string, tag = "1")]
    pub body: String,
}

/// Replace the text of `target`. Only its sender may; every version is kept.
#[derive(Clone, PartialEq, Message)]
pub struct Edit {
    #[prost(bytes = "vec", tag = "1")]
    pub target: Vec<u8>,
    #[prost(string, tag = "2")]
    pub body: String,
}

/// Remove `target` and all its versions. Only its sender may.
#[derive(Clone, PartialEq, Message)]
pub struct Delete {
    #[prost(bytes = "vec", tag = "1")]
    pub target: Vec<u8>,
}

/// Longest message text accepted, in bytes.
pub const MAX_TEXT: usize = 64 * 1024;
/// Longest display name, in characters.
pub const MAX_DISPLAY_NAME: usize = 64;
/// Longest typing template, in characters.
pub const MAX_TYPING_TEMPLATE: usize = 80;
/// The placeholder a typing template must contain.
pub const NAME_PLACEHOLDER: &str = "$$NAME$$";
/// Most placeholders a typing template may contain.
pub const MAX_PLACEHOLDERS: usize = 3;
/// Longest note, in characters.
pub const MAX_NOTE: usize = 60;
/// How long a note lasts unless its owner chooses no expiry.
pub const NOTE_DEFAULT_MS: u64 = 24 * 3600 * 1000;

/// Why a name, template or note is not allowed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("{what} is longer than {max} characters")]
    TooLong { what: &'static str, max: usize },
    #[error("{0} may not contain control characters or line breaks")]
    ControlCharacters(&'static str),
    #[error("a typing template must contain {NAME_PLACEHOLDER} between 1 and {MAX_PLACEHOLDERS} times")]
    Placeholders,
}

fn check_text(s: &str, what: &'static str, max: usize) -> Result<(), ProfileError> {
    if s.chars().count() > max {
        return Err(ProfileError::TooLong { what, max });
    }
    if s.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202e}' | '\u{2066}'..='\u{2069}')) {
        return Err(ProfileError::ControlCharacters(what));
    }
    Ok(())
}

/// Check a display name (global or per-conversation).
pub fn check_display_name(s: &str) -> Result<(), ProfileError> {
    check_text(s, "display name", MAX_DISPLAY_NAME)
}

/// Check a typing template. Empty is allowed and means the standard text.
pub fn check_typing_template(s: &str) -> Result<(), ProfileError> {
    check_text(s, "typing template", MAX_TYPING_TEMPLATE)?;
    let n = s.matches(NAME_PLACEHOLDER).count();
    if !s.is_empty() && !(1..=MAX_PLACEHOLDERS).contains(&n) {
        return Err(ProfileError::Placeholders);
    }
    Ok(())
}

/// Check a note's text.
pub fn check_note(s: &str) -> Result<(), ProfileError> {
    check_text(s, "note", MAX_NOTE)
}

/// Fill a typing template with the name the viewer sees. Empty template
/// means the standard text.
pub fn render_typing(template: &str, name: &str) -> String {
    if template.is_empty() { format!("{name} is typing") } else { template.replace(NAME_PLACEHOLDER, name) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_templates() {
        assert!(check_typing_template("").is_ok());
        assert!(check_typing_template("the one who types is $$NAME$$").is_ok());
        assert!(check_typing_template("$$NAME$$ $$NAME$$ $$NAME$$").is_ok());
        assert_eq!(check_typing_template("no placeholder"), Err(ProfileError::Placeholders));
        assert_eq!(check_typing_template("$$NAME$$$$NAME$$$$NAME$$$$NAME$$"), Err(ProfileError::Placeholders));
        assert!(check_typing_template("line\nbreak $$NAME$$").is_err());
        assert!(check_typing_template(&format!("{} $$NAME$$", "x".repeat(80))).is_err());
        assert_eq!(render_typing("the one who types is $$NAME$$", "Alice"), "the one who types is Alice");
        assert_eq!(render_typing("", "Alice"), "Alice is typing");
    }

    #[test]
    fn names_and_notes() {
        assert!(check_display_name("Alice ☺").is_ok());
        assert!(check_display_name(&"a".repeat(65)).is_err());
        // Right-to-left override could disguise a name.
        assert!(check_display_name("evil\u{202e}gnp.exe").is_err());
        assert!(check_note("at the beach").is_ok());
        assert!(check_note(&"n".repeat(61)).is_err());
    }
}
