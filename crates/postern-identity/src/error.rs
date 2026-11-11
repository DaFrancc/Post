/// Why an identity operation was rejected.
///
/// Every variant is a hard failure: callers must not act on the input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Bytes did not decode, or a field had the wrong length or form.
    #[error("malformed {0}")]
    Malformed(&'static str),
    /// Bytes decoded but are not the canonical encoding (for example, they
    /// carry unknown fields). Rejected so a signed object has one encoding.
    #[error("non-canonical encoding")]
    NonCanonical,
    /// The link uses a format version this code does not understand.
    #[error("unsupported link version {0}")]
    UnsupportedVersion(u32),
    /// A signature did not verify. Both halves of a hybrid signature must.
    #[error("bad signature")]
    BadSignature,
    /// An added device's countersignature did not verify.
    #[error("bad countersignature")]
    BadCosignature,
    /// An `AddDevice` link had no countersignature from the new device.
    #[error("missing countersignature")]
    MissingCosignature,
    /// A countersignature was attached to a link that must not have one.
    #[error("unexpected countersignature")]
    UnexpectedCosignature,
    /// The link's sequence number does not extend the chain.
    #[error("wrong sequence number: expected {expected}, got {got}")]
    WrongSequence {
        /// The next sequence number the chain expects.
        expected: u64,
        /// The sequence number in the link.
        got: u64,
    },
    /// The link does not point at the current chain head.
    #[error("link does not extend the current head")]
    WrongPrevHash,
    /// The link names a different account.
    #[error("link belongs to a different account")]
    WrongAccount,
    /// The first link of a chain must be a genesis link.
    #[error("chain does not start with a genesis link")]
    NotGenesis,
    /// A genesis link appeared after the start of a chain.
    #[error("genesis link after the start of the chain")]
    GenesisNotFirst,
    /// The signer is not a device of this account.
    #[error("signer is not a device of this account")]
    UnknownSigner,
    /// The signer was a device of this account but has been removed.
    #[error("signer has been removed")]
    SignerRemoved,
    /// The link's signer is not allowed to perform this operation.
    #[error("signer may not perform this operation")]
    SignerNotAllowed,
    /// The target device is not a device of this account.
    #[error("unknown device")]
    UnknownDevice,
    /// The target device has already been removed.
    #[error("device already removed")]
    DeviceRemoved,
    /// The device, or its MLS key, is already in the chain. Removed devices
    /// cannot return; a returning device generates new keys.
    #[error("device or key already present in the chain")]
    AlreadyPresent,
    /// Removing this device would leave the account with none.
    #[error("cannot remove the last device")]
    LastDevice,
    /// The account already has the maximum number of active devices.
    #[error("too many devices")]
    TooManyDevices,
    /// A bot device added to a non-bot account, or the reverse. Whether an
    /// account is a bot is fixed by its genesis device.
    #[error("device's bot status does not match the account")]
    BotMismatch,
    /// Username does not meet the rules in [`crate::chain::validate_username`].
    #[error("invalid username")]
    InvalidUsername,
    /// Server domain does not meet the rules in [`crate::chain::validate_domain`].
    #[error("invalid server domain")]
    InvalidDomain,
    /// A link at a sequence number we already hold differs from ours.
    /// This means the server, or a device, forked the chain.
    #[error("chain fork at sequence {seq}")]
    Fork {
        /// Where the two chains disagree.
        seq: u64,
    },
    /// Links were offered starting beyond the end of our chain.
    #[error("gap in chain: have {have} links, offered links from {from}")]
    Gap {
        /// Links already held.
        have: u64,
        /// First sequence number offered.
        from: u64,
    },
    /// The operating system's random number generator failed.
    #[error("randomness unavailable")]
    Rng,
}
