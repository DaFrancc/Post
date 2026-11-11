//! Account identity for Postern (design section 5).
//!
//! - [`hybrid`]: Ed25519 + ML-DSA-65 signatures. Both must verify.
//! - [`chain`]: the signed, hash-chained log of an account's devices.
//! - [`safety`]: safety numbers derived from two account IDs.
//!
//! Every signature and hash uses a unique, versioned label (see [`labels`]).
//! Everything here treats its input as hostile: the server supplies chains,
//! and the server is untrusted.

pub mod chain;
pub mod hybrid;
pub mod safety;
mod error;
mod wire;

pub use chain::{AccountId, Chain, DeviceEntry, DeviceKeys, DeviceRecord, Link, LinkHash, Op};
pub use error::Error;
pub use hybrid::{DeviceId, HybridSignature, HybridSigningKey, HybridVerifyingKey};
pub use safety::{SafetyNumber, safety_number};

/// Domain-separation labels. Never reuse one for a different purpose.
pub mod labels {
    /// A device signing a chain link.
    pub const CHAIN_LINK: &str = "postern/chain/v1";
    /// A newly added device countersigning the link that adds it.
    pub const CHAIN_COSIGN: &str = "postern/chain-cosign/v1";
    /// Hash of a link's bytes: chain pointers and account IDs.
    pub const LINK_HASH: &str = "postern/link-hash/v1";
    /// Device ID derived from a device's chain keys.
    pub const DEVICE_ID: &str = "postern/device-id/v1";
    /// Safety number for a pair of accounts.
    pub const SAFETY: &str = "postern/safety/v1";
    /// A device answering the server's login challenge.
    pub const AUTH: &str = "postern/auth/v1";
}
