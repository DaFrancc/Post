//! Errors as the app shows them: a kind to act on and one plain sentence,
//! never a backtrace or a protocol code.

use post_core as core;
use post_proto::ErrorCode;

/// What went wrong, for the app to react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Something the user typed is not acceptable; `message` says why.
    InvalidInput,
    /// The invite code is unknown, used or expired.
    InviteInvalid,
    /// The username belongs to someone else.
    UsernameTaken,
    /// The server could not be reached, or stopped answering.
    Network,
    /// The server is not a Post server this app can talk to.
    Server,
    /// No account on this device yet.
    NotRegistered,
    /// The platform keyring is missing or refused.
    Keyring,
    /// The profile on this device could not be read.
    Storage,
    /// Anything else.
    Other,
}

/// An error the app can show as it is.
#[derive(Debug, Clone)]
pub struct PostError {
    /// What went wrong.
    pub kind: ErrorKind,
    /// One sentence for the user.
    pub message: String,
}

impl PostError {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

impl std::fmt::Display for PostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PostError {}

impl From<core::Error> for PostError {
    fn from(e: core::Error) -> Self {
        use core::Error as E;
        match e {
            E::InvalidInput(m) => Self::new(ErrorKind::InvalidInput, capitalize(&m)),
            E::UnknownUser(u) => Self::new(ErrorKind::InvalidInput, format!("There is no one called {u} on this server.")),
            E::NotRegistered => Self::new(ErrorKind::NotRegistered, "There is no account on this device yet."),
            E::AlreadyRegistered => Self::new(ErrorKind::InvalidInput, "This device already has an account."),
            E::Network(_) | E::NotConnected => Self::new(
                ErrorKind::Network,
                "Could not reach the server. Check the address, and that the server is running.",
            ),
            E::Server(s) => from_server(s.code(), &s.message),
            E::Protocol(_) => Self::new(
                ErrorKind::Server,
                "The server did not answer like a Post server. Check the address, or the server may need updating.",
            ),
            E::WrongKey | E::Corrupt(_) | E::Db(_) | E::Storage(_) => {
                Self::new(ErrorKind::Storage, format!("This device's Post data could not be read ({e})."))
            }
            other => Self::new(ErrorKind::Other, capitalize(&other.to_string())),
        }
    }
}

fn from_server(code: ErrorCode, detail: &str) -> PostError {
    let (kind, text) = match code {
        ErrorCode::InviteInvalid => (ErrorKind::InviteInvalid, "That invite code is unknown, already used, or expired. Ask your server's admin for a new one."),
        ErrorCode::UsernameTaken => (ErrorKind::UsernameTaken, "That username is taken. Try another."),
        ErrorCode::RateLimited => (ErrorKind::Server, "The server is busy. Try again in a moment."),
        ErrorCode::ChainRejected => (ErrorKind::Server, "The server refused this device's keys. The server may be for a different address."),
        ErrorCode::Disabled => (ErrorKind::Server, "This device or account has been disabled on the server."),
        ErrorCode::Internal => (ErrorKind::Server, "The server had a problem. Try again in a moment."),
        _ => return PostError::new(ErrorKind::Server, format!("The server refused: {}.", detail.trim_end_matches('.'))),
    };
    PostError::new(kind, text)
}

/// Errors from the keyring and the file system arrive as `anyhow` chains;
/// show the outermost message only.
pub(crate) fn from_anyhow(e: anyhow::Error) -> PostError {
    match e.downcast::<core::Error>() {
        Ok(core) => core.into(),
        Err(e) => {
            let text = capitalize(&e.to_string());
            let kind = if text.to_lowercase().contains("keyring") { ErrorKind::Keyring } else { ErrorKind::Other };
            PostError::new(kind, text)
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    let mut out: String = match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '?', '!']) {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_sentences() {
        let e: PostError = core::Error::InvalidInput("usernames are 3 to 32 of a-z".into()).into();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::InvalidInput, "Usernames are 3 to 32 of a-z."));
        let e: PostError = core::Error::Network("connecting to ws://x: IO error: Connection refused (os error 111)".into()).into();
        assert_eq!(e.kind, ErrorKind::Network);
        assert!(!e.message.contains("os error"), "no raw system errors: {}", e.message);
        let e = from_server(ErrorCode::UsernameTaken, "username is taken");
        assert_eq!(e.kind, ErrorKind::UsernameTaken);
        let e = from_anyhow(anyhow::anyhow!("no keyring is available (dbus)"));
        assert_eq!(e.kind, ErrorKind::Keyring);
    }
}
