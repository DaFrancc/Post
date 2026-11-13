//! Typing indicators and presence: signals that are relayed to whoever is
//! online and never stored (design section 6, "Ephemeral signals").
//!
//! They are not MLS application messages: an undelivered one would leave a
//! gap in the sender's ratchet that offline devices would have to skip, and
//! they would fill the group log. Instead each signal is
//!
//! 1. signed with the sender device's MLS signature key (Ed25519), over
//!    `label ‖ group_id ‖ epoch ‖ signal`, so a member cannot pass off a
//!    signal as another member's;
//! 2. sealed with XChaCha20-Poly1305 under the group's
//!    `MLS-Exporter("post/ephemeral/v1")` secret, with the group ID and
//!    epoch as associated data, so only the group's current members can read
//!    it and it cannot be replayed into another group or epoch.
//!
//! Receivers check the signature against the claimed device's leaf, then
//! discard it, like every signature (design section 14). Signals more than a
//! minute old, or from more than a minute in the future, are dropped.

#![allow(missing_docs)]

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use post_identity::{AccountId, DeviceId};
use prost::{Message, Oneof};
use zeroize::Zeroizing;

use crate::{Error, Result};

/// MLS exporter label for the sealing key.
pub const EXPORTER_LABEL: &str = "post/ephemeral/v1";
/// Signature label.
const SIGN_LABEL: &str = "post/ephemeral/v1";
/// Signals older or newer than this on arrival are dropped.
pub const MAX_AGE_MS: u64 = 60_000;

#[derive(Clone, PartialEq, Message)]
pub struct Signal {
    #[prost(bytes = "vec", tag = "1")]
    pub account: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub device: Vec<u8>,
    #[prost(uint64, tag = "3")]
    pub sent_at_ms: u64,
    #[prost(oneof = "Kind", tags = "10, 11")]
    pub kind: Option<Kind>,
}

#[derive(Clone, PartialEq, Oneof)]
pub enum Kind {
    /// The sender is typing in this conversation.
    #[prost(message, tag = "10")]
    Typing(Typing),
    /// The sender's presence.
    #[prost(message, tag = "11")]
    Presence(PresenceSignal),
}

#[derive(Clone, PartialEq, Message)]
pub struct Typing {}

#[derive(Clone, PartialEq, Message)]
pub struct PresenceSignal {
    /// [`State`].
    #[prost(enumeration = "State", tag = "1")]
    pub state: i32,
}

/// What others see. A person who chose to be invisible sends nothing and is
/// seen as offline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum State {
    Offline = 0,
    Online = 1,
    Away = 2,
    DoNotDisturb = 3,
}

#[derive(Clone, PartialEq, Message)]
struct Signed {
    #[prost(bytes = "vec", tag = "1")]
    signal: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    signature: Vec<u8>,
}

/// Where a signal belongs: the group and the epoch it is sealed for.
pub struct Context<'a> {
    pub group_id: &'a [u8],
    pub epoch: u64,
    /// `MLS-Exporter(EXPORTER_LABEL, "", 32)` for that epoch.
    pub key: Zeroizing<Vec<u8>>,
}

impl Context<'_> {
    fn bound(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.group_id.len() + 8);
        out.extend_from_slice(self.group_id);
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out
    }

    fn signed_bytes(&self, signal: &[u8]) -> Vec<u8> {
        let mut msg = vec![SIGN_LABEL.len() as u8];
        msg.extend_from_slice(SIGN_LABEL.as_bytes());
        msg.extend_from_slice(&self.bound());
        msg.extend_from_slice(signal);
        msg
    }

    fn cipher(&self) -> Result<XChaCha20Poly1305> {
        XChaCha20Poly1305::new_from_slice(&self.key).map_err(|_| Error::Protocol("ephemeral key has the wrong length".into()))
    }

    /// Sign and seal a signal.
    pub fn seal(&self, signal: &Signal, key: &SigningKey) -> Result<Vec<u8>> {
        let bytes = signal.encode_to_vec();
        let signature = key.sign(&self.signed_bytes(&bytes)).to_bytes().to_vec();
        let plain = Signed { signal: bytes, signature }.encode_to_vec();
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).map_err(|_| Error::Rng)?;
        let ct = self
            .cipher()?
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: &plain, aad: &self.bound() })
            .map_err(|_| Error::Protocol("sealing failed".into()))?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Open a sealed signal and check its signature with the key of the leaf
    /// it claims to come from. `leaf_key` looks up that key; `None` means the
    /// claimed device is not a member.
    pub fn open(
        &self,
        sealed: &[u8],
        leaf_key: impl Fn(&AccountId, &DeviceId) -> Option<Vec<u8>>,
        now_ms: u64,
    ) -> std::result::Result<Signal, Dropped> {
        let bad = |why: &str| Dropped { why: why.into(), clocks_apart: false };
        if sealed.len() < 24 {
            return Err(bad("too short"));
        }
        let (nonce, ct) = sealed.split_at(24);
        let plain = self
            .cipher()
            .map_err(|_| bad("bad key"))?
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: &self.bound() })
            .map_err(|_| bad("not sealed for this group and epoch"))?;
        let signed = Signed::decode(plain.as_slice()).map_err(|_| bad("malformed"))?;
        let signal = Signal::decode(signed.signal.as_slice()).map_err(|_| bad("malformed"))?;
        let account = crate::store::account_id(&signal.account).map_err(|_| bad("bad account"))?;
        let device = DeviceId::from_slice(&signal.device).map_err(|_| bad("bad device"))?;
        let key = leaf_key(&account, &device).ok_or_else(|| bad("sender is not a member"))?;
        let key: [u8; 32] = key.as_slice().try_into().map_err(|_| bad("bad leaf key"))?;
        let vk = VerifyingKey::from_bytes(&key).map_err(|_| bad("bad leaf key"))?;
        let sig: [u8; 64] = signed.signature.as_slice().try_into().map_err(|_| bad("bad signature"))?;
        vk.verify_strict(&self.signed_bytes(&signed.signal), &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|_| bad("bad signature"))?;
        if signal.sent_at_ms.abs_diff(now_ms) > MAX_AGE_MS {
            // Usually not old at all: the sender's clock and this one
            // disagree. Worth saying how far, as nothing else shows it.
            let (way, by) = if signal.sent_at_ms > now_ms { ("ahead of", signal.sent_at_ms - now_ms) } else { ("behind", now_ms - signal.sent_at_ms) };
            return Err(Dropped {
                why: format!("its sender's clock is {}s {way} this device's (more than {}s apart)", by / 1000, MAX_AGE_MS / 1000),
                clocks_apart: true,
            });
        }
        Ok(signal)
    }
}

/// Why a relayed signal was not used.
#[derive(Debug)]
pub struct Dropped {
    pub why: String,
    /// It checked out but for its time: the sender's clock and this
    /// device's are too far apart, which loses every signal between them.
    pub clocks_apart: bool,
}

impl std::fmt::Display for Dropped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a typing or presence signal was dropped: {}", self.why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(group: &[u8], epoch: u64, key: u8) -> Context<'_> {
        Context { group_id: group, epoch, key: Zeroizing::new(vec![key; 32]) }
    }

    fn typing(account: [u8; 32], device: [u8; 16], at: u64) -> Signal {
        Signal { account: account.to_vec(), device: device.to_vec(), sent_at_ms: at, kind: Some(Kind::Typing(Typing {})) }
    }

    #[test]
    fn signals_are_bound_to_their_sender_group_and_time() {
        let alice = SigningKey::from_bytes(&[1; 32]);
        let mallory = SigningKey::from_bytes(&[2; 32]);
        let alice_leaf = alice.verifying_key().to_bytes().to_vec();
        let keys = |a: &AccountId, d: &DeviceId| (a.0 == [1; 32] && d.0 == [1; 16]).then(|| alice_leaf.clone());
        let now = 1_000_000;
        let group = [7u8; 16];
        let c = ctx(&group, 5, 9);

        let sealed = c.seal(&typing([1; 32], [1; 16], now), &alice).expect("seal");
        assert_eq!(c.open(&sealed, keys, now + 500).expect("open").kind, Some(Kind::Typing(Typing {})));

        // Not readable in another epoch, another group, or under another key.
        let other_group = [8u8; 16];
        for wrong in [ctx(&group, 6, 9), ctx(&other_group, 5, 9), ctx(&group, 5, 3)] {
            assert!(wrong.open(&sealed, keys, now).is_err());
        }
        // A member cannot pass off a signal as Alice's.
        let forged = c.seal(&typing([1; 32], [1; 16], now), &mallory).expect("seal");
        assert!(c.open(&forged, keys, now).is_err());
        // Nor claim a device that is not in the group.
        let stranger = c.seal(&typing([3; 32], [3; 16], now), &alice).expect("seal");
        assert!(c.open(&stranger, keys, now).is_err());
        // Stale and future signals are dropped, as clocks apart (which is
        // what it usually is), saying how far.
        let stale = c.open(&sealed, keys, now + MAX_AGE_MS + 1_000).expect_err("stale");
        assert!(stale.clocks_apart && stale.why.contains("61s behind"), "{stale}");
        let future = c.seal(&typing([1; 32], [1; 16], now + 2 * MAX_AGE_MS), &alice).expect("seal");
        let ahead = c.open(&future, keys, now).expect_err("from the future");
        assert!(ahead.clocks_apart && ahead.why.contains("120s ahead of"), "{ahead}");
        // Nothing else is put down to clocks.
        assert!(!c.open(&forged, keys, now).expect_err("forged").clocks_apart);
        // Tampering breaks it.
        let mut bent = sealed.clone();
        bent[30] ^= 1;
        assert!(c.open(&bent, keys, now).is_err());
    }
}
