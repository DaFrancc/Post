//! The client-server wire protocol (design section 8).
//!
//! Binary WebSocket frames, one protobuf message each: [`ClientFrame`] up,
//! [`ServerFrame`] down. Types are hand-written prost structs, so building
//! needs no `protoc`.
//!
//! Every connection starts with the server's [`Hello`]. Then it becomes one
//! of two kinds, and stays that kind:
//!
//! - A **session**: the first request is [`Register`] or [`Auth`]. Only
//!   device-scoped requests are allowed: directory, own chain, KeyPackage
//!   uploads, mailbox, tokens.
//! - An **anonymous connection**: the first request is anything else. Only
//!   conversation requests are allowed: groups, sends, commits, fetches,
//!   subscriptions, KeyPackage claims. They are authorized by group access
//!   keys or anonymous tokens, never by identity.
//!
//! The server refuses a request on the wrong kind of connection, so a client
//! cannot accidentally link its identity to its conversations.
//!
//! Device linking ([`LinkPut`], [`LinkGet`]) also runs on anonymous
//! connections: the new device has no account yet, and the server only
//! relays sealed slots it cannot read.

#![allow(missing_docs)]

use prost::{Message, Oneof};

/// Bumped on any incompatible change to this protocol.
pub const PROTOCOL_VERSION: u32 = 4;

/// Largest frame either side accepts. A Welcome for a 250-device group is
/// about 700 KB (spike 1); this leaves headroom.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Version of the anonymous token scheme in [`Token::version`]:
/// RFC 9474 RSABSSA-SHA384-PSS-Randomized.
pub const TOKEN_VERSION_RSABSSA_SHA384: u32 = 1;

/// The bytes a device signs (under `post_identity::labels::AUTH`) to log in
/// on this connection: `len(domain) as u8 ‖ domain ‖ nonce`. The domain stops
/// a signature for one server being replayed to another.
pub fn auth_message(server_domain: &str, nonce: &[u8]) -> Vec<u8> {
    let d = server_domain.as_bytes();
    let mut out = Vec::with_capacity(1 + d.len() + nonce.len());
    out.push(u8::try_from(d.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(d);
    out.extend_from_slice(nonce);
    out
}

// ---------------------------------------------------------------- frames

#[derive(Clone, PartialEq, Message)]
pub struct ClientFrame {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(oneof = "Request", tags = "10, 11, 20, 21, 22, 23, 24, 25, 26, 40, 41, 42, 43, 44, 45, 46, 47, 48")]
    pub request: Option<Request>,
}

#[derive(Clone, PartialEq, Oneof)]
pub enum Request {
    // Login: the first request of a session.
    #[prost(message, tag = "10")]
    Register(Register),
    #[prost(message, tag = "11")]
    Auth(Auth),

    // Session only.
    #[prost(message, tag = "20")]
    Directory(Directory),
    #[prost(message, tag = "21")]
    AppendChain(AppendChain),
    #[prost(message, tag = "22")]
    UploadKeyPackages(UploadKeyPackages),
    #[prost(message, tag = "23")]
    FetchMailbox(FetchMailbox),
    #[prost(message, tag = "24")]
    AckMailbox(AckMailbox),
    #[prost(message, tag = "25")]
    GetTokenKey(GetTokenKey),
    #[prost(message, tag = "26")]
    IssueTokens(IssueTokens),

    // Anonymous only.
    #[prost(message, tag = "40")]
    CreateGroup(CreateGroup),
    #[prost(message, tag = "41")]
    Send(Send),
    #[prost(message, tag = "42")]
    Commit(Commit),
    #[prost(message, tag = "43")]
    FetchGroups(FetchGroups),
    #[prost(message, tag = "44")]
    Subscribe(Subscribe),
    #[prost(message, tag = "45")]
    ClaimKeyPackages(ClaimKeyPackages),
    #[prost(message, tag = "46")]
    SendEphemeral(SendEphemeral),
    #[prost(message, tag = "47")]
    LinkPut(LinkPut),
    #[prost(message, tag = "48")]
    LinkGet(LinkGet),
}

impl Request {
    /// Whether this request belongs on a session (true) or an anonymous
    /// connection (false). Login requests count as session requests.
    pub fn is_session(&self) -> bool {
        matches!(
            self,
            Request::Register(_)
                | Request::Auth(_)
                | Request::Directory(_)
                | Request::AppendChain(_)
                | Request::UploadKeyPackages(_)
                | Request::FetchMailbox(_)
                | Request::AckMailbox(_)
                | Request::GetTokenKey(_)
                | Request::IssueTokens(_)
        )
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct ServerFrame {
    #[prost(oneof = "ServerKind", tags = "1, 2, 3, 4")]
    pub kind: Option<ServerKind>,
}

#[derive(Clone, PartialEq, Oneof)]
pub enum ServerKind {
    #[prost(message, tag = "1")]
    Hello(Hello),
    #[prost(message, tag = "2")]
    Response(Response),
    #[prost(message, tag = "3")]
    Wake(Wake),
    #[prost(message, tag = "4")]
    Ephemeral(Ephemeral),
}

/// A relayed ephemeral signal (typing, presence), pushed to anonymous
/// connections subscribed to the group. Never stored.
#[derive(Clone, PartialEq, Message)]
pub struct Ephemeral {
    #[prost(bytes = "vec", tag = "1")]
    pub group_id: Vec<u8>,
    /// Sealed and signed by the sender; opaque to the server.
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Hello {
    #[prost(uint32, tag = "1")]
    pub protocol_version: u32,
    #[prost(string, tag = "2")]
    pub server_domain: String,
    /// 32 random bytes, fresh per connection.
    #[prost(bytes = "vec", tag = "3")]
    pub nonce: Vec<u8>,
}

/// Something new is waiting. On a session: the mailbox. On an anonymous
/// connection: the listed subscribed groups. Carries no content.
#[derive(Clone, PartialEq, Message)]
pub struct Wake {
    #[prost(bool, tag = "1")]
    pub mailbox: bool,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub groups: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Response {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(oneof = "Reply", tags = "10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22")]
    pub reply: Option<Reply>,
}

#[derive(Clone, PartialEq, Oneof)]
pub enum Reply {
    #[prost(message, tag = "10")]
    Error(Error),
    #[prost(message, tag = "11")]
    Empty(Empty),
    #[prost(message, tag = "12")]
    Authenticated(Authenticated),
    #[prost(message, tag = "13")]
    DirectoryUpdate(DirectoryUpdate),
    #[prost(message, tag = "14")]
    KeyPackageCount(KeyPackageCount),
    #[prost(message, tag = "15")]
    Mailbox(Mailbox),
    #[prost(message, tag = "16")]
    TokenKey(TokenKey),
    #[prost(message, tag = "17")]
    IssuedTokens(IssuedTokens),
    #[prost(message, tag = "18")]
    ClaimedKeyPackages(ClaimedKeyPackages),
    #[prost(message, tag = "19")]
    Accepted(Accepted),
    #[prost(message, tag = "20")]
    GroupBatch(GroupBatch),
    #[prost(message, tag = "21")]
    GroupCreated(GroupCreated),
    #[prost(message, tag = "22")]
    LinkSlot(LinkSlot),
}

// ---------------------------------------------------------------- errors

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum ErrorCode {
    Unspecified = 0,
    /// The request was malformed or failed validation.
    BadRequest = 1,
    /// Login is required first, or failed.
    Unauthenticated = 2,
    /// The invite code is unknown, used or expired.
    InviteInvalid = 3,
    /// The username is already registered.
    UsernameTaken = 4,
    /// The named account, device or group does not exist.
    NotFound = 5,
    /// The message's epoch is not the group's current epoch. See
    /// `Error::current_epoch`.
    WrongEpoch = 6,
    /// The group access key does not match.
    BadAccessKey = 7,
    /// The anonymous token is invalid, expired or already spent.
    BadToken = 8,
    /// A chain link failed verification.
    ChainRejected = 9,
    /// A KeyPackage failed validation.
    KeyPackageInvalid = 10,
    /// The account or device is disabled.
    Disabled = 11,
    /// The server failed; the request may be retried.
    Internal = 12,
    /// A group with this ID already exists.
    GroupExists = 13,
    /// The request is not allowed on this kind of connection.
    WrongConnection = 14,
    /// A rate limit was reached.
    RateLimited = 15,
    /// The thing being created or written already exists.
    Conflict = 16,
}

#[derive(Clone, PartialEq, Message)]
pub struct Error {
    #[prost(enumeration = "ErrorCode", tag = "1")]
    pub code: i32,
    #[prost(string, tag = "2")]
    pub message: String,
    /// Set with `WrongEpoch`.
    #[prost(uint64, tag = "3")]
    pub current_epoch: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct Empty {}

// ---------------------------------------------------------------- login

/// Create an account. `auth_sig_*` sign [`auth_message`] with the genesis
/// device's chain key, binding the registration to this connection.
#[derive(Clone, PartialEq, Message)]
pub struct Register {
    #[prost(string, tag = "1")]
    pub invite_code: String,
    #[prost(bytes = "vec", tag = "2")]
    pub genesis: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub auth_sig_ed25519: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    pub auth_sig_mldsa65: Vec<u8>,
}

/// Log in as an existing device by signing [`auth_message`].
#[derive(Clone, PartialEq, Message)]
pub struct Auth {
    #[prost(bytes = "vec", tag = "1")]
    pub device_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub sig_ed25519: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub sig_mldsa65: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Authenticated {
    #[prost(bytes = "vec", tag = "1")]
    pub account_id: Vec<u8>,
    /// Unclaimed one-time KeyPackages this device has on the server.
    #[prost(uint32, tag = "2")]
    pub key_packages_remaining: u32,
    #[prost(bool, tag = "3")]
    pub has_last_resort: bool,
}

// ---------------------------------------------------------------- session requests

/// Every account changed since `since_version`. Every device downloads the
/// whole directory, so the server never learns whose details anyone wants.
#[derive(Clone, PartialEq, Message)]
pub struct Directory {
    #[prost(uint64, tag = "1")]
    pub since_version: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct DirectoryAccount {
    #[prost(bytes = "vec", tag = "1")]
    pub account_id: Vec<u8>,
    #[prost(string, tag = "2")]
    pub username: String,
    /// The full chain, genesis first. Clients verify it themselves.
    #[prost(bytes = "vec", repeated, tag = "3")]
    pub chain: Vec<Vec<u8>>,
    #[prost(bool, tag = "4")]
    pub disabled: bool,
    /// Active devices not seen for 30 days. Their accounts' other devices,
    /// or any member, remove their leaves from groups the account keeps
    /// another device in; they are added back when they return.
    #[prost(bytes = "vec", repeated, tag = "5")]
    pub stale_devices: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DirectoryUpdate {
    /// Pass this as `since_version` next time.
    #[prost(uint64, tag = "1")]
    pub version: u64,
    #[prost(message, repeated, tag = "2")]
    pub accounts: Vec<DirectoryAccount>,
}

/// Append a link to the caller's own chain.
#[derive(Clone, PartialEq, Message)]
pub struct AppendChain {
    #[prost(bytes = "vec", tag = "1")]
    pub link: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct UploadKeyPackages {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub key_packages: Vec<Vec<u8>>,
    /// Optional replacement last-resort KeyPackage.
    #[prost(bytes = "vec", tag = "2")]
    pub last_resort: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct KeyPackageCount {
    #[prost(uint32, tag = "1")]
    pub remaining: u32,
}

/// Welcomes addressed to this device.
#[derive(Clone, PartialEq, Message)]
pub struct FetchMailbox {
    /// 0 means the server default.
    #[prost(uint32, tag = "1")]
    pub limit: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct MailboxEntry {
    #[prost(uint64, tag = "1")]
    pub seq: u64,
    /// An MLS Welcome. Opaque to the server: the group ID is inside.
    #[prost(bytes = "vec", tag = "2")]
    pub welcome: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Mailbox {
    #[prost(message, repeated, tag = "1")]
    pub entries: Vec<MailboxEntry>,
    #[prost(bool, tag = "2")]
    pub more: bool,
}

/// Delete mailbox entries up to and including `seq`. Sent only after the
/// resulting state is committed locally.
#[derive(Clone, PartialEq, Message)]
pub struct AckMailbox {
    #[prost(uint64, tag = "1")]
    pub seq: u64,
}

/// The current public key for anonymous tokens.
#[derive(Clone, PartialEq, Message)]
pub struct GetTokenKey {}

#[derive(Clone, PartialEq, Message)]
pub struct TokenKey {
    #[prost(uint32, tag = "1")]
    pub version: u32,
    #[prost(bytes = "vec", tag = "2")]
    pub key_id: Vec<u8>,
    /// SubjectPublicKeyInfo DER.
    #[prost(bytes = "vec", tag = "3")]
    pub public_key: Vec<u8>,
    /// Tokens under this key are accepted until this time (Unix ms).
    #[prost(uint64, tag = "4")]
    pub valid_until_ms: u64,
}

/// Blind-sign these blinded messages under the key `key_id`.
#[derive(Clone, PartialEq, Message)]
pub struct IssueTokens {
    #[prost(bytes = "vec", tag = "1")]
    pub key_id: Vec<u8>,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub blinded: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct IssuedTokens {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub blind_signatures: Vec<Vec<u8>>,
}

// ---------------------------------------------------------------- anonymous requests

/// Proof of being a registered user, without saying which (design section 8).
#[derive(Clone, PartialEq, Message)]
pub struct Token {
    #[prost(uint32, tag = "1")]
    pub version: u32,
    #[prost(bytes = "vec", tag = "2")]
    pub key_id: Vec<u8>,
    /// The random message that was blind-signed.
    #[prost(bytes = "vec", tag = "3")]
    pub message: Vec<u8>,
    /// RFC 9474 message randomizer (32 bytes).
    #[prost(bytes = "vec", tag = "4")]
    pub randomizer: Vec<u8>,
    #[prost(bytes = "vec", tag = "5")]
    pub signature: Vec<u8>,
}

/// Register a group its creator has built at epoch 1. Costs one token.
#[derive(Clone, PartialEq, Message)]
pub struct CreateGroup {
    #[prost(message, optional, tag = "1")]
    pub token: Option<Token>,
    #[prost(bytes = "vec", tag = "2")]
    pub group_id: Vec<u8>,
    /// SHA-256 of epoch 1's access key.
    #[prost(bytes = "vec", tag = "3")]
    pub access_hash: Vec<u8>,
    /// The MLS Welcome, put in the mailbox of each device in `welcome_to`.
    #[prost(bytes = "vec", tag = "4")]
    pub welcome: Vec<u8>,
    #[prost(bytes = "vec", repeated, tag = "5")]
    pub welcome_to: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GroupCreated {
    #[prost(uint64, tag = "1")]
    pub epoch: u64,
}

/// An MLS application message or proposal, in the group's current epoch.
#[derive(Clone, PartialEq, Message)]
pub struct Send {
    #[prost(bytes = "vec", tag = "1")]
    pub group_id: Vec<u8>,
    /// The current epoch's access key.
    #[prost(bytes = "vec", tag = "2")]
    pub access_key: Vec<u8>,
    /// 16 random bytes. Retries with the same id are idempotent.
    #[prost(bytes = "vec", tag = "3")]
    pub client_msg_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    pub message: Vec<u8>,
}

/// An MLS commit, in the group's current epoch. Advances it.
#[derive(Clone, PartialEq, Message)]
pub struct Commit {
    #[prost(bytes = "vec", tag = "1")]
    pub group_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub access_key: Vec<u8>,
    /// SHA-256 of the next epoch's access key.
    #[prost(bytes = "vec", tag = "3")]
    pub next_access_hash: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    pub client_msg_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "5")]
    pub commit: Vec<u8>,
    #[prost(bytes = "vec", tag = "6")]
    pub welcome: Vec<u8>,
    #[prost(bytes = "vec", repeated, tag = "7")]
    pub welcome_to: Vec<Vec<u8>>,
}

/// Where an accepted Send or Commit landed in the group log.
#[derive(Clone, PartialEq, Message)]
pub struct Accepted {
    #[prost(uint64, tag = "1")]
    pub seq: u64,
    /// The group's epoch after this entry.
    #[prost(uint64, tag = "2")]
    pub epoch: u64,
}

/// A position in one group: entries of `epoch` after `after_seq`, authorized
/// by that epoch's access key.
#[derive(Clone, PartialEq, Message)]
pub struct GroupCursor {
    #[prost(bytes = "vec", tag = "1")]
    pub group_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub epoch: u64,
    #[prost(bytes = "vec", tag = "3")]
    pub access_key: Vec<u8>,
    #[prost(uint64, tag = "4")]
    pub after_seq: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct FetchGroups {
    #[prost(message, repeated, tag = "1")]
    pub groups: Vec<GroupCursor>,
    /// Most entries to return in total; 0 means the server default.
    #[prost(uint32, tag = "2")]
    pub limit: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum EntryKind {
    Unspecified = 0,
    Application = 1,
    Commit = 2,
    /// An MLS proposal, such as a member asking to be removed when leaving.
    /// Sent with [`Send`]; a later commit by another member applies it.
    Proposal = 3,
}

/// A group log entry. Carries no sender: MLS encrypts that inside.
#[derive(Clone, PartialEq, Message)]
pub struct LogEntry {
    #[prost(uint64, tag = "1")]
    pub seq: u64,
    #[prost(uint64, tag = "2")]
    pub epoch: u64,
    #[prost(enumeration = "EntryKind", tag = "3")]
    pub kind: i32,
    #[prost(bytes = "vec", tag = "4")]
    pub client_msg_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "5")]
    pub payload: Vec<u8>,
    #[prost(uint64, tag = "6")]
    pub created_at_ms: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct GroupEntries {
    #[prost(bytes = "vec", tag = "1")]
    pub group_id: Vec<u8>,
    #[prost(message, repeated, tag = "2")]
    pub entries: Vec<LogEntry>,
    /// Set when this group's result is cut short by an error, such as a
    /// wrong access key. The other groups are unaffected.
    #[prost(message, optional, tag = "3")]
    pub error: Option<Error>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GroupBatch {
    #[prost(message, repeated, tag = "1")]
    pub groups: Vec<GroupEntries>,
    /// More entries are waiting in at least one group.
    #[prost(bool, tag = "2")]
    pub more: bool,
}

/// Wake this connection when these groups get new entries. Held in memory
/// only. Replaces any earlier subscription on this connection.
#[derive(Clone, PartialEq, Message)]
pub struct Subscribe {
    #[prost(message, repeated, tag = "1")]
    pub groups: Vec<GroupCursor>,
}

/// Claim KeyPackages of an account. Costs one token.
///
/// Without `device_id`: one for each active device of the account, the way a
/// conversation is started. With `device_id`: up to `count` for that one
/// device, the way its own account's devices add it to many conversations at
/// once after it is linked.
#[derive(Clone, PartialEq, Message)]
pub struct ClaimKeyPackages {
    #[prost(message, optional, tag = "1")]
    pub token: Option<Token>,
    #[prost(bytes = "vec", tag = "2")]
    pub account_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub device_id: Vec<u8>,
    /// With `device_id`: how many, at most [`MAX_CLAIM`]. 0 means 1.
    #[prost(uint32, tag = "4")]
    pub count: u32,
}

/// Most KeyPackages one claim for a single device returns.
pub const MAX_CLAIM: u32 = 100;

#[derive(Clone, PartialEq, Message)]
pub struct ClaimedKeyPackage {
    #[prost(bytes = "vec", tag = "1")]
    pub device_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub key_package: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ClaimedKeyPackages {
    #[prost(message, repeated, tag = "1")]
    pub key_packages: Vec<ClaimedKeyPackage>,
}

/// Largest ephemeral payload accepted.
pub const MAX_EPHEMERAL_BYTES: usize = 4096;

/// Relay a signal to members connected right now. Authorized by the current
/// epoch's access key. Not stored, not ordered, not guaranteed.
#[derive(Clone, PartialEq, Message)]
pub struct SendEphemeral {
    #[prost(bytes = "vec", tag = "1")]
    pub group_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub access_key: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub payload: Vec<u8>,
}

// ---------------------------------------------------------------- device linking

/// Largest payload one link slot holds.
pub const MAX_LINK_SLOT_BYTES: usize = 64 * 1024;
/// Slots per rendezvous.
pub const LINK_SLOTS: u32 = 8;
/// Longest a [`LinkGet`] waits for its slot to be filled.
pub const MAX_LINK_WAIT_MS: u32 = 25_000;

/// Write one slot of a link rendezvous (design section 5, "Linking a device").
///
/// A rendezvous is a short-lived set of write-once slots, held in the
/// server's memory only and gone after ten minutes. Writing slot 0 creates
/// it; the other slots can be written once it exists. Payloads are sealed by
/// the two devices under a key the server never sees.
#[derive(Clone, PartialEq, Message)]
pub struct LinkPut {
    /// 16 bytes, derived from the link code.
    #[prost(bytes = "vec", tag = "1")]
    pub rendezvous: Vec<u8>,
    #[prost(uint32, tag = "2")]
    pub slot: u32,
    #[prost(bytes = "vec", tag = "3")]
    pub payload: Vec<u8>,
}

/// Read one slot, waiting up to `wait_ms` for it to be written.
#[derive(Clone, PartialEq, Message)]
pub struct LinkGet {
    #[prost(bytes = "vec", tag = "1")]
    pub rendezvous: Vec<u8>,
    #[prost(uint32, tag = "2")]
    pub slot: u32,
    #[prost(uint32, tag = "3")]
    pub wait_ms: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct LinkSlot {
    /// False if the wait ended before the slot was written.
    #[prost(bool, tag = "1")]
    pub present: bool,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code: code as i32, message: message.into(), current_epoch: 0 }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code(), self.message)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip() {
        let f = ClientFrame {
            id: 7,
            request: Some(Request::Send(Send {
                group_id: vec![1; 16],
                access_key: vec![4; 32],
                client_msg_id: vec![2; 16],
                message: vec![3; 40],
            })),
        };
        assert_eq!(ClientFrame::decode(f.encode_to_vec().as_slice()).expect("decode"), f);
    }

    /// The oneof `tags` lists repeat each variant's field number by hand. A
    /// number missing from a list would make that variant vanish on decode,
    /// so every variant must survive a round trip.
    #[test]
    fn every_variant_roundtrips() {
        let requests = vec![
            Request::Register(Register::default()),
            Request::Auth(Auth::default()),
            Request::Directory(Directory::default()),
            Request::AppendChain(AppendChain::default()),
            Request::UploadKeyPackages(UploadKeyPackages::default()),
            Request::FetchMailbox(FetchMailbox::default()),
            Request::AckMailbox(AckMailbox::default()),
            Request::GetTokenKey(GetTokenKey::default()),
            Request::IssueTokens(IssueTokens::default()),
            Request::CreateGroup(CreateGroup::default()),
            Request::Send(Send::default()),
            Request::Commit(Commit::default()),
            Request::FetchGroups(FetchGroups::default()),
            Request::Subscribe(Subscribe::default()),
            Request::ClaimKeyPackages(ClaimKeyPackages::default()),
            Request::SendEphemeral(SendEphemeral::default()),
            Request::LinkPut(LinkPut::default()),
            Request::LinkGet(LinkGet::default()),
        ];
        for r in requests {
            let f = ClientFrame { id: 1, request: Some(r) };
            assert_eq!(ClientFrame::decode(f.encode_to_vec().as_slice()).expect("decode"), f);
        }
        let replies = vec![
            Reply::Error(Error::default()),
            Reply::Empty(Empty::default()),
            Reply::Authenticated(Authenticated::default()),
            Reply::DirectoryUpdate(DirectoryUpdate::default()),
            Reply::KeyPackageCount(KeyPackageCount::default()),
            Reply::Mailbox(Mailbox::default()),
            Reply::TokenKey(TokenKey::default()),
            Reply::IssuedTokens(IssuedTokens::default()),
            Reply::ClaimedKeyPackages(ClaimedKeyPackages::default()),
            Reply::Accepted(Accepted::default()),
            Reply::GroupBatch(GroupBatch::default()),
            Reply::GroupCreated(GroupCreated::default()),
            Reply::LinkSlot(LinkSlot::default()),
        ];
        for r in replies {
            let f = ServerFrame { kind: Some(ServerKind::Response(Response { id: 1, reply: Some(r) })) };
            assert_eq!(ServerFrame::decode(f.encode_to_vec().as_slice()).expect("decode"), f);
        }
        for k in [
            ServerKind::Hello(Hello::default()),
            ServerKind::Wake(Wake::default()),
            ServerKind::Ephemeral(Ephemeral::default()),
        ] {
            let f = ServerFrame { kind: Some(k) };
            assert_eq!(ServerFrame::decode(f.encode_to_vec().as_slice()).expect("decode"), f);
        }
    }

    #[test]
    fn auth_message_binds_domain() {
        assert_ne!(auth_message("a.example", &[0; 32]), auth_message("b.example", &[0; 32]));
        assert_ne!(auth_message("ab", b"c"), auth_message("a", b"bc"));
    }

    #[test]
    fn connection_kinds() {
        assert!(Request::Directory(Directory::default()).is_session());
        assert!(Request::IssueTokens(IssueTokens::default()).is_session());
        assert!(!Request::Send(Send::default()).is_session());
        assert!(!Request::ClaimKeyPackages(ClaimKeyPackages::default()).is_session());
        assert!(!Request::FetchGroups(FetchGroups::default()).is_session());
        // A device being linked has no account to log in with.
        assert!(!Request::LinkPut(LinkPut::default()).is_session());
        assert!(!Request::LinkGet(LinkGet::default()).is_session());
    }
}
