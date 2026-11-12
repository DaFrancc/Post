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
    #[prost(oneof = "Body", tags = "10, 13, 14, 16, 17, 18, 30")]
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
    /// Design section 7 reserves 16 for `CallEvent`.
    #[prost(message, tag = "16")]
    Call(CallEvent),
    #[prost(message, tag = "17")]
    Profile(Profile),
    #[prost(message, tag = "18")]
    ConversationName(ConversationName),
    /// Self group only.
    #[prost(message, tag = "30")]
    SelfSync(SelfSync),
}

/// A call starting, someone joining or leaving it, declining it, or it
/// ending (design section 10, "Signalling"). Not shown as a message.
#[derive(Clone, PartialEq, Message)]
pub struct CallEvent {
    #[prost(enumeration = "CallKind", tag = "1")]
    pub kind: i32,
    /// The call's random 16-byte ID, chosen by whoever starts it.
    #[prost(bytes = "vec", tag = "2")]
    pub call_id: Vec<u8>,
    /// The sender's random 16-byte name in the call (start, join, leave):
    /// their identity in LiveKit, which never sees device IDs.
    #[prost(bytes = "vec", tag = "3")]
    pub participant_id: Vec<u8>,
    /// Starting with video.
    #[prost(bool, tag = "4")]
    pub video: bool,
    /// Whom to ring (32-byte account IDs): on a start, only these (none
    /// listed: everyone, unless `quiet`); on a ring, these, into a call
    /// already under way.
    #[prost(bytes = "vec", repeated, tag = "5")]
    pub ring: Vec<Vec<u8>>,
    /// A start that rings nobody: the call shows as one to join (large
    /// groups, design section 10).
    #[prost(bool, tag = "6")]
    pub quiet: bool,
}

/// Most people one call event can ring.
pub const MAX_RING: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum CallKind {
    Start = 0,
    Join = 1,
    Leave = 2,
    Decline = 3,
    End = 4,
    /// Ring people into a call already under way.
    Ring = 5,
}

impl CallEvent {
    /// Well formed: a 16-byte call ID, and a 16-byte participant ID where
    /// the kind needs one.
    pub fn check(&self) -> Result<CallKind, &'static str> {
        let kind = CallKind::try_from(self.kind).map_err(|_| "unknown call event")?;
        if self.call_id.len() != 16 {
            return Err("a call ID is 16 bytes");
        }
        let needs_participant = matches!(kind, CallKind::Start | CallKind::Join | CallKind::Leave);
        if needs_participant && self.participant_id.len() != 16 || !needs_participant && !self.participant_id.is_empty() {
            return Err("a participant ID is 16 bytes, for start, join and leave only");
        }
        let rings = matches!(kind, CallKind::Start | CallKind::Ring);
        if !rings && !self.ring.is_empty() || kind == CallKind::Ring && self.ring.is_empty() {
            return Err("only a start or a ring names whom to ring, and a ring names someone");
        }
        if self.ring.len() > MAX_RING || self.ring.iter().any(|a| a.len() != 32) {
            return Err("whom to ring: up to 256 account IDs of 32 bytes");
        }
        Ok(kind)
    }
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
    /// How far the user has read the server's announcements, as the latest
    /// read announcement's ID. Merges by taking the larger, like `read`.
    #[prost(uint64, tag = "6")]
    pub announcements_read: u64,
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
    /// How this person's messages arrive on screen: the name of an app
    /// animation preset, like `decode`. Empty means the app's default; a
    /// name an app does not know also falls back to its default, so new
    /// presets need no protocol change.
    #[prost(string, tag = "6")]
    pub arrival: String,
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
/// Longest arrival preset name.
pub const MAX_ARRIVAL: usize = 32;

/// Why a name, template or note is not allowed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("{what} is longer than {max} characters")]
    TooLong { what: &'static str, max: usize },
    #[error("{0} may not contain control characters or line breaks")]
    ControlCharacters(&'static str),
    #[error("a typing template must contain {NAME_PLACEHOLDER} between 1 and {MAX_PLACEHOLDERS} times")]
    Placeholders,
    #[error("an arrival style is a short name of lowercase letters, digits and dashes")]
    Arrival,
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

/// Check an arrival preset's name: empty, or up to 32 lowercase ASCII
/// letters, digits and dashes.
pub fn check_arrival(s: &str) -> Result<(), ProfileError> {
    if s.len() <= MAX_ARRIVAL && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        Ok(())
    } else {
        Err(ProfileError::Arrival)
    }
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

/// The usernames a message's text mentions (design section 7, "Mentions"),
/// each once, in the order they first appear.
///
/// A mention is `@` and a username, not touching a letter, digit or `_` on
/// either side, outside code and not after a backslash. The text is read in
/// the same blocks as the app's formatting reads it: fenced code blocks,
/// quote lines one at a time, and paragraphs in which inline code may run
/// across lines.
pub fn mentions(body: &str) -> Vec<String> {
    let lines: Vec<(usize, usize)> = {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, b) in body.bytes().enumerate() {
            if b == b'\n' {
                out.push((start, i));
                start = i + 1;
            }
        }
        out.push((start, body.len()));
        out
    };
    let line = |i: usize| &body[lines[i].0..lines[i].1];
    let is_fence = |i: usize| line(i).starts_with("```");
    let is_quote = |i: usize| line(i).starts_with("> ") || line(i) == ">";
    // A fence opens a block when its own line holds no closing fence and a
    // later line closes it.
    let closes_at = |i: usize| (!line(i)[3..].contains("```")).then(|| (i + 1..lines.len()).find(|&c| is_fence(c))).flatten();

    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_fence(i)
            && let Some(close) = closes_at(i)
        {
            i = close + 1;
            continue;
        }
        if is_quote(i) {
            while i < lines.len() && is_quote(i) {
                let (start, end) = lines[i];
                scan_mentions(body, (start + 2).min(end), end, &mut found);
                i += 1;
            }
            continue;
        }
        let first = i;
        while i < lines.len() && !is_quote(i) && !(is_fence(i) && i != first && closes_at(i).is_some()) {
            i += 1;
        }
        scan_mentions(body, lines[first].0, lines[i - 1].1, &mut found);
    }
    found
}

/// Mentions in one block of text, `body[from..to]`, skipping escapes and
/// inline code as the app's formatting does.
fn scan_mentions(body: &str, from: usize, to: usize, found: &mut Vec<String>) {
    let s = &body.as_bytes()[..to];
    let run = |i: usize, c: u8| s[i..].iter().take_while(|&&b| b == c).count();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = from;
    while i < to {
        match s[i] {
            b'\\' if i + 1 < to && s[i + 1].is_ascii_punctuation() => i += 2,
            b'`' => {
                // Code runs to the next run of exactly as many backticks.
                let n = run(i, b'`');
                let mut k = i + n;
                let mut end = None;
                while k < to {
                    if s[k] == b'`' {
                        let m = run(k, b'`');
                        if m == n {
                            end = Some(k + m);
                            break;
                        }
                        k += m;
                    } else {
                        k += 1;
                    }
                }
                i = match end {
                    Some(e) if e - i > 2 * n => e,
                    _ => i + n,
                };
            }
            b'@' => {
                let name_len = s[i + 1..].iter().take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || **b == b'_').count();
                let name = &body[i + 1..i + 1 + name_len];
                let before_ok = !body[from..i].chars().next_back().is_some_and(word);
                let after_ok = !body[i + 1 + name_len..to].chars().next().is_some_and(word);
                if before_ok && after_ok && post_identity::chain::validate_username(name).is_ok() {
                    if !found.iter().any(|f| f == name) {
                        found.push(name.to_owned());
                    }
                    i += 1 + name_len;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_in_text() {
        let m = |s: &str| mentions(s);
        assert_eq!(m("hi @alice and @bob_2, @alice again"), ["alice", "bob_2"]);
        assert_eq!(m("@carol: start of text"), ["carol"]);
        assert_eq!(m("**@dave** *@erin* (@frank) ||@gina||"), ["dave", "erin", "frank", "gina"]);
        // `_` is part of names, so `_` italics cannot wrap one (`*` can).
        assert!(m("_@erin_").is_empty());
        assert_eq!(m("> quoted @hank\nand @ivan"), ["hank", "ivan"]);
        // Not mentions: an email, too short or long, capitals, touching a
        // word, escaped, in code.
        assert!(m("mail sam@example.com").is_empty());
        assert!(m("@ab @Alice @alice2X é@alice @alice_é").is_empty());
        assert!(m(&format!("@{}", "a".repeat(33))).is_empty());
        assert!(m(r"\@alice").is_empty());
        assert!(m("`@alice` and ``x @bob ` y``").is_empty());
        assert!(m("```\n@alice\n```\n").is_empty());
        // An unclosed fence or tick is ordinary text.
        assert_eq!(m("```\n@alice"), ["alice"]);
        assert_eq!(m("a ` b @alice"), ["alice"]);
        // Inline code runs across lines in a paragraph, not out of a quote.
        assert!(m("a `x\n@alice` b").is_empty());
        assert_eq!(m("> a `x\n@alice` b"), ["alice"]);
        // Punctuation right after ends the name.
        assert_eq!(m("thanks @alice! and @bob."), ["alice", "bob"]);
    }

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
    fn arrival_names() {
        for ok in ["", "bubble", "decode", "type-writer", "wave2"] {
            assert!(check_arrival(ok).is_ok(), "{ok}");
        }
        for bad in ["Bubble", "two words", "décode", "a_b", &"x".repeat(33)] {
            assert_eq!(check_arrival(bad), Err(ProfileError::Arrival), "{bad}");
        }
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
