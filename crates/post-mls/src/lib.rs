//! The MLS configuration every Post client and the server must agree on
//! (design section 6): the ciphersuite, how a leaf names its device, the group
//! metadata extension, and the checks the server can make on ciphertext.
//!
//! Keeping this in one crate means the server and clients cannot drift apart.

pub use openmls::prelude::ContentType;
use openmls::prelude::{tls_codec::Deserialize as _, *};
use openmls_traits::crypto::OpenMlsCrypto;
use post_identity::{AccountId, DeviceId, LinkHash};
use prost::Message as _;

/// `MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`. OpenMLS uses the
/// provisional codepoint 0x004E; the IETF draft assigns 0x0011.
pub const SUITE: Ciphersuite = Ciphersuite::MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519;

/// Group context extension carrying [`GroupMeta`]. In the private-use range.
pub const META_EXTENSION: u16 = 0xff10;

/// Application messages are padded to a multiple of this many bytes, to blur
/// message length (design section 7).
pub const PADDING: usize = 256;

/// Why MLS input was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Bytes did not decode as the expected MLS structure.
    #[error("malformed MLS {0}")]
    Malformed(&'static str),
    /// Not a PrivateMessage. Post groups use ciphertext-only framing, so
    /// commits and proposals are encrypted too.
    #[error("message is not a PrivateMessage")]
    NotPrivate,
    /// The KeyPackage failed OpenMLS validation (signature, lifetime, ...).
    #[error("invalid KeyPackage: {0}")]
    InvalidKeyPackage(String),
    /// The KeyPackage uses a different ciphersuite.
    #[error("wrong ciphersuite")]
    WrongSuite,
    /// The leaf's credential is not a Post device credential.
    #[error("bad credential")]
    BadCredential,
    /// The leaf does not support what every Post group requires.
    #[error("missing required capability")]
    MissingCapability,
}

// ---------------------------------------------------------------- credentials

/// Length of a Post credential identity: account ID ‖ device ID.
pub const CREDENTIAL_LEN: usize = 32 + 16;

/// The basic credential naming a device: `account_id ‖ device_id`.
pub fn credential(account: &AccountId, device: &DeviceId) -> Credential {
    let mut id = Vec::with_capacity(CREDENTIAL_LEN);
    id.extend_from_slice(&account.0);
    id.extend_from_slice(&device.0);
    BasicCredential::new(id).into()
}

/// The device a credential names. Says nothing about whether that device is
/// authorized: check its account's chain with `Chain::authorizes_leaf`.
pub fn parse_credential(c: &Credential) -> Result<(AccountId, DeviceId), Error> {
    let basic = BasicCredential::try_from(c.clone()).map_err(|_| Error::BadCredential)?;
    let id = basic.identity();
    if id.len() != CREDENTIAL_LEN {
        return Err(Error::BadCredential);
    }
    let mut account = [0u8; 32];
    account.copy_from_slice(&id[..32]);
    let device = DeviceId::from_slice(&id[32..]).map_err(|_| Error::BadCredential)?;
    Ok((LinkHash(account), device))
}

/// The device behind a leaf, with the signature key it uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafIdentity {
    /// The account the leaf claims.
    pub account: AccountId,
    /// The device the leaf claims.
    pub device: DeviceId,
    /// The leaf's MLS signature key.
    pub signature_key: Vec<u8>,
}

/// Read a leaf node's claimed identity.
pub fn leaf_identity(leaf: &LeafNode) -> Result<LeafIdentity, Error> {
    let (account, device) = parse_credential(leaf.credential())?;
    Ok(LeafIdentity { account, device, signature_key: leaf.signature_key().as_slice().to_vec() })
}

// ---------------------------------------------------------------- capabilities and config

/// The capabilities every Post leaf advertises. `LastResort` must be
/// listed for a device's last-resort KeyPackage to be valid.
pub fn capabilities() -> Capabilities {
    Capabilities::new(
        None,
        Some(&[SUITE]),
        Some(&[ExtensionType::Unknown(META_EXTENSION), ExtensionType::LastResort]),
        None,
        Some(&[CredentialType::Basic]),
    )
}

fn sender_ratchet() -> SenderRatchetConfiguration {
    // Devices process each group's log strictly in order (design section 6),
    // so messages never arrive out of order; a small window suffices.
    SenderRatchetConfiguration::new(0, 1000)
}

/// Configuration for joining a group from a Welcome.
pub fn join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(true)
        .padding_size(PADDING)
        .max_past_epochs(0)
        .sender_ratchet_configuration(sender_ratchet())
        .build()
}

/// The group context extensions of a Post group with this metadata, and
/// nothing else. A commit that changes the metadata must set exactly these.
pub fn group_context_extensions(meta: &GroupMeta) -> Result<Extensions<GroupContext>, Error> {
    Extensions::from_vec(vec![
        Extension::Unknown(META_EXTENSION, UnknownExtension(meta.encode())),
        Extension::RequiredCapabilities(RequiredCapabilitiesExtension::new(
            &[ExtensionType::Unknown(META_EXTENSION)],
            &[],
            &[CredentialType::Basic],
        )),
    ])
    .map_err(|_| Error::Malformed("group extensions"))
}

/// A builder for a new group with Post's settings and the given metadata.
pub fn group_builder(meta: &GroupMeta) -> Result<MlsGroupBuilder, Error> {
    let extensions = group_context_extensions(meta)?;
    Ok(MlsGroup::builder()
        .ciphersuite(SUITE)
        .with_wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(true)
        .padding_size(PADDING)
        .max_past_epochs(0)
        .sender_ratchet_configuration(sender_ratchet())
        .with_capabilities(capabilities())
        .with_group_context_extensions(extensions))
}

// ---------------------------------------------------------------- group metadata

/// What kind of conversation a group is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum ConversationKind {
    /// Not set; rejected.
    Unspecified = 0,
    /// A direct message between exactly two accounts.
    Dm = 1,
    /// A named group.
    Group = 2,
    /// An account's own devices and nobody else (design section 6). Carries
    /// what those devices keep in step: nicknames, device names, settings.
    SelfGroup = 3,
}

/// Group metadata, agreed by all members as part of the MLS group context
/// (design section 6). The server never sees it.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GroupMeta {
    /// [`ConversationKind`].
    #[prost(enumeration = "ConversationKind", tag = "1")]
    pub kind: i32,
    /// Group name; empty for DMs.
    #[prost(string, tag = "2")]
    pub name: String,
    /// Accounts allowed to add or remove accounts and change this metadata.
    /// In a DM, both accounts.
    #[prost(bytes = "vec", repeated, tag = "3")]
    pub admins: Vec<Vec<u8>>,
    /// [`HistoryVisibility`]: whether a member who adds someone shares past
    /// messages with them. Unspecified in DMs, which never gain members.
    #[prost(enumeration = "HistoryVisibility", tag = "4")]
    pub history: i32,
    /// With `Shared`, how many days back to share; 0 means everything.
    #[prost(uint32, tag = "5")]
    pub history_days: u32,
}

/// Whether new members of a group receive its past messages (design section 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum HistoryVisibility {
    /// Not set. Required for DMs; rejected for groups.
    Unspecified = 0,
    /// New members see only what is sent after they join.
    Hidden = 1,
    /// The adding member shares past messages with the newcomer.
    Shared = 2,
}

impl GroupMeta {
    /// Metadata for a DM between two accounts.
    pub fn dm(a: &AccountId, b: &AccountId) -> Self {
        let mut admins = vec![a.0.to_vec(), b.0.to_vec()];
        admins.sort();
        Self { kind: ConversationKind::Dm as i32, name: String::new(), admins, history: 0, history_days: 0 }
    }

    /// Metadata for a named group, created by `creator` as its first admin.
    pub fn group(name: &str, creator: &AccountId, history: HistoryVisibility, history_days: u32) -> Self {
        Self {
            kind: ConversationKind::Group as i32,
            name: name.into(),
            admins: vec![creator.0.to_vec()],
            history: history as i32,
            history_days,
        }
    }

    /// Metadata for an account's self group. The account is its only admin.
    pub fn self_group(account: &AccountId) -> Self {
        Self {
            kind: ConversationKind::SelfGroup as i32,
            name: String::new(),
            admins: vec![account.0.to_vec()],
            history: 0,
            history_days: 0,
        }
    }

    /// The history setting, if valid for this kind of conversation.
    pub fn history_visibility(&self) -> Option<HistoryVisibility> {
        HistoryVisibility::try_from(self.history).ok()
    }

    /// The conversation kind, if valid.
    pub fn conversation_kind(&self) -> Option<ConversationKind> {
        match ConversationKind::try_from(self.kind) {
            Ok(ConversationKind::Unspecified) | Err(_) => None,
            Ok(k) => Some(k),
        }
    }

    /// Read and validate the metadata from a group context's extensions
    /// (`MlsGroup::extensions()`, or `group_context().extensions()` on a
    /// staged Welcome or commit).
    pub fn from_extensions(extensions: &Extensions<GroupContext>) -> Result<Self, Error> {
        let ext = extensions.unknown(META_EXTENSION).ok_or(Error::Malformed("group metadata"))?;
        let meta = GroupMeta::decode(ext.0.as_slice()).map_err(|_| Error::Malformed("group metadata"))?;
        if meta.encode_to_vec() != ext.0 {
            return Err(Error::Malformed("group metadata"));
        }
        let kind = meta.conversation_kind().ok_or(Error::Malformed("group kind"))?;
        if meta.admins.iter().any(|a| a.len() != 32) {
            return Err(Error::Malformed("group admins"));
        }
        let history = meta.history_visibility().ok_or(Error::Malformed("history visibility"))?;
        match kind {
            ConversationKind::Dm => {
                if meta.admins.len() != 2 || !meta.name.is_empty() || history != HistoryVisibility::Unspecified || meta.history_days != 0 {
                    return Err(Error::Malformed("dm metadata"));
                }
            }
            ConversationKind::Group => {
                if meta.admins.is_empty() || history == HistoryVisibility::Unspecified {
                    return Err(Error::Malformed("group metadata"));
                }
                if history == HistoryVisibility::Hidden && meta.history_days != 0 {
                    return Err(Error::Malformed("group metadata"));
                }
            }
            ConversationKind::SelfGroup => {
                if meta.admins.len() != 1 || !meta.name.is_empty() || history != HistoryVisibility::Unspecified || meta.history_days != 0 {
                    return Err(Error::Malformed("self group metadata"));
                }
            }
            ConversationKind::Unspecified => unreachable!("rejected above"),
        }
        Ok(meta)
    }

    fn encode(&self) -> Vec<u8> {
        self.encode_to_vec()
    }
}

// ---------------------------------------------------------------- server-side checks

/// What the server can learn from an MLS message without any keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The group the message is for.
    pub group_id: Vec<u8>,
    /// The epoch it was encrypted in.
    pub epoch: u64,
    /// Application message, commit or proposal.
    pub content_type: ContentType,
}

/// Parse an MLS message's public header. Rejects anything that is not a
/// PrivateMessage.
pub fn parse_header(bytes: &[u8]) -> Result<Header, Error> {
    let msg = MlsMessageIn::tls_deserialize_exact(bytes).map_err(|_| Error::Malformed("message"))?;
    let proto = msg.try_into_protocol_message().map_err(|_| Error::Malformed("protocol message"))?;
    if proto.wire_format() != WireFormat::PrivateMessage {
        return Err(Error::NotPrivate);
    }
    Ok(Header { group_id: proto.group_id().as_slice().to_vec(), epoch: proto.epoch().as_u64(), content_type: proto.content_type() })
}

/// Check that bytes are an MLS Welcome, without decrypting it.
pub fn check_welcome(bytes: &[u8]) -> Result<(), Error> {
    match MlsMessageIn::tls_deserialize_exact(bytes).map_err(|_| Error::Malformed("welcome"))?.extract() {
        MlsMessageBodyIn::Welcome(_) => Ok(()),
        _ => Err(Error::Malformed("welcome")),
    }
}

/// A KeyPackage that passed [`validate_key_package`].
#[derive(Debug, Clone)]
pub struct ValidKeyPackage {
    /// The validated KeyPackage.
    pub key_package: KeyPackage,
    /// The device its credential names, and the signature key it uses.
    pub leaf: LeafIdentity,
    /// Whether it is a last-resort KeyPackage.
    pub last_resort: bool,
}

/// Validate a serialized KeyPackage: OpenMLS checks (signatures, lifetime,
/// init key) plus Post's suite, credential format and capabilities.
/// The caller must still check `leaf` against the account's chain.
pub fn validate_key_package(bytes: &[u8], crypto: &impl OpenMlsCrypto) -> Result<ValidKeyPackage, Error> {
    let kp_in = KeyPackageIn::tls_deserialize_exact(bytes).map_err(|_| Error::Malformed("key package"))?;
    let kp = kp_in.validate(crypto, ProtocolVersion::Mls10).map_err(|e| Error::InvalidKeyPackage(e.to_string()))?;
    if kp.ciphersuite() != SUITE {
        return Err(Error::WrongSuite);
    }
    let caps = kp.leaf_node().capabilities();
    if !caps.extensions().contains(&ExtensionType::Unknown(META_EXTENSION)) || !caps.credentials().contains(&CredentialType::Basic) {
        return Err(Error::MissingCapability);
    }
    let leaf = leaf_identity(kp.leaf_node())?;
    Ok(ValidKeyPackage { last_resort: kp.last_resort(), key_package: kp, leaf })
}

/// The crypto provider Post uses everywhere (libcrux; see spike 1).
pub fn crypto() -> openmls_libcrux_crypto::CryptoProvider {
    openmls_libcrux_crypto::CryptoProvider::new().expect("libcrux provider initializes on supported platforms")
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmls::prelude::tls_codec::Serialize as _;
    use openmls_basic_credential::SignatureKeyPair;

    fn device() -> (openmls_libcrux_crypto::Provider, SignatureKeyPair, CredentialWithKey, AccountId, DeviceId) {
        let provider = openmls_libcrux_crypto::Provider::default();
        let signer = SignatureKeyPair::new(SUITE.signature_algorithm()).expect("keys");
        let account = LinkHash([7; 32]);
        let device = DeviceId([9; 16]);
        let cred = CredentialWithKey { credential: credential(&account, &device), signature_key: signer.to_public_vec().into() };
        (provider, signer, cred, account, device)
    }

    #[test]
    fn credential_roundtrip() {
        let (_, _, cred, account, device) = device();
        assert_eq!(parse_credential(&cred.credential).expect("parse"), (account, device));
        let short: Credential = BasicCredential::new(vec![1; 10]).into();
        assert_eq!(parse_credential(&short), Err(Error::BadCredential));
    }

    #[test]
    fn key_package_validation() {
        let (provider, signer, cred, account, device) = device();
        let kp = KeyPackage::builder().leaf_node_capabilities(capabilities()).build(SUITE, &provider, &signer, cred.clone()).expect("kp");
        let bytes = kp.key_package().tls_serialize_detached().expect("ser");
        let v = validate_key_package(&bytes, &crypto()).expect("valid");
        assert_eq!((v.leaf.account, v.leaf.device), (account, device));
        assert_eq!(v.leaf.signature_key, signer.to_public_vec());
        assert!(!v.last_resort);

        // A last-resort KeyPackage validates and says so.
        let last = KeyPackage::builder()
            .leaf_node_capabilities(capabilities())
            .mark_as_last_resort()
            .build(SUITE, &provider, &signer, cred.clone())
            .expect("last-resort kp");
        let bytes = last.key_package().tls_serialize_detached().expect("ser");
        assert!(validate_key_package(&bytes, &crypto()).expect("valid").last_resort);

        // Missing the metadata capability.
        let plain = KeyPackage::builder().build(SUITE, &provider, &signer, cred.clone()).expect("kp");
        let bytes = plain.key_package().tls_serialize_detached().expect("ser");
        assert_eq!(validate_key_package(&bytes, &crypto()).err(), Some(Error::MissingCapability));

        // Tampered.
        let mut bad = kp.key_package().tls_serialize_detached().expect("ser");
        let n = bad.len();
        bad[n - 10] ^= 1;
        assert!(validate_key_package(&bad, &crypto()).is_err());
    }

    #[test]
    fn group_metadata_and_headers() {
        let (a_provider, a_signer, a_cred, a_account, _) = device();
        let (b_provider, b_signer, b_cred, _, _) = device();
        let b_account = LinkHash([8; 32]);
        let meta = GroupMeta::dm(&a_account, &b_account);
        let mut group = group_builder(&meta).expect("builder").build(&a_provider, &a_signer, a_cred).expect("group");
        assert_eq!(GroupMeta::from_extensions(group.extensions()).expect("meta"), meta);

        let b_kp = KeyPackage::builder().leaf_node_capabilities(capabilities()).build(SUITE, &b_provider, &b_signer, b_cred).expect("kp");
        let (commit, welcome, _) = group.add_members(&a_provider, &a_signer, &[b_kp.key_package().clone()]).expect("add");
        group.merge_pending_commit(&a_provider).expect("merge");

        let h = parse_header(&commit.tls_serialize_detached().expect("ser")).expect("header");
        assert_eq!((h.epoch, h.content_type), (0, ContentType::Commit));
        assert_eq!(h.group_id, group.group_id().as_slice());
        check_welcome(&welcome.tls_serialize_detached().expect("ser")).expect("welcome");

        let msg = group.create_message(&a_provider, &a_signer, b"hi").expect("msg");
        let h = parse_header(&msg.tls_serialize_detached().expect("ser")).expect("header");
        assert_eq!((h.epoch, h.content_type), (1, ContentType::Application));
        assert_eq!(parse_header(b"garbage").err(), Some(Error::Malformed("message")));
    }

    #[test]
    fn metadata_validation() {
        let ext = |m: &GroupMeta| Extensions::single(Extension::Unknown(META_EXTENSION, UnknownExtension(m.encode_to_vec()))).expect("ext");
        let good = GroupMeta::dm(&LinkHash([1; 32]), &LinkHash([2; 32]));
        assert_eq!(GroupMeta::from_extensions(&ext(&good)).expect("valid"), good);

        let group = GroupMeta::group("friends", &LinkHash([1; 32]), HistoryVisibility::Shared, 30);
        assert_eq!(GroupMeta::from_extensions(&ext(&group)).expect("valid"), group);

        let own = GroupMeta::self_group(&LinkHash([1; 32]));
        assert_eq!(GroupMeta::from_extensions(&ext(&own)).expect("valid"), own);
        let two_owners = GroupMeta { admins: vec![vec![1; 32], vec![2; 32]], ..own.clone() };
        let named_self = GroupMeta { name: "mine".into(), ..own.clone() };
        for bad in [two_owners, named_self] {
            assert!(GroupMeta::from_extensions(&ext(&bad)).is_err(), "{bad:?}");
        }

        let named_dm = GroupMeta { name: "x".into(), ..good.clone() };
        let three_admin_dm = GroupMeta { admins: vec![vec![1; 32], vec![2; 32], vec![3; 32]], ..good.clone() };
        let dm_with_history = GroupMeta { history: HistoryVisibility::Shared as i32, ..good.clone() };
        let short_admin = GroupMeta { admins: vec![vec![1; 31]], ..group.clone() };
        let no_admins = GroupMeta { admins: vec![], ..group.clone() };
        let group_without_policy = GroupMeta { history: 0, history_days: 0, ..group.clone() };
        let hidden_with_days = GroupMeta { history: HistoryVisibility::Hidden as i32, ..group.clone() };
        let unknown_policy = GroupMeta { history: 9, ..group.clone() };
        let unspecified = GroupMeta { kind: 0, ..good.clone() };
        for bad in [
            named_dm,
            three_admin_dm,
            dm_with_history,
            short_admin,
            no_admins,
            group_without_policy,
            hidden_with_days,
            unknown_policy,
            unspecified,
        ] {
            assert!(GroupMeta::from_extensions(&ext(&bad)).is_err(), "{bad:?}");
        }
        // Missing entirely.
        assert!(GroupMeta::from_extensions(&Extensions::empty()).is_err());
        // Trailing bytes after valid metadata.
        let mut padded = good.encode_to_vec();
        padded.extend_from_slice(&[0x98, 0x06, 0x01]);
        let e = Extensions::single(Extension::Unknown(META_EXTENSION, UnknownExtension(padded))).expect("ext");
        assert!(GroupMeta::from_extensions(&e).is_err());
    }
}
