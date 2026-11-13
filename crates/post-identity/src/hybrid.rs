//! Hybrid signatures: Ed25519 and ML-DSA-65 over the same message.
//!
//! A hybrid signature is valid only if **both** halves verify, so it is at
//! least as strong as the stronger of the two: a quantum computer that breaks
//! Ed25519, or a flaw in the younger ML-DSA code, is not enough on its own.
//!
//! Both algorithms sign the same byte string:
//! `len(label) as u8 ‖ label ‖ message`. The label separates uses (see
//! [`crate::labels`]), so a signature made for one purpose never verifies for
//! another.
//!
//! ML-DSA signing is hedged (FIPS 204's default): fresh randomness is mixed
//! in, which protects against fault and side-channel attacks. Signatures are
//! therefore not reproducible. Ed25519 signatures are deterministic.

use std::fmt;

use ed25519_dalek::Signer as _;
use ml_dsa::{EncodedVerifyingKey, MlDsa65};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{Error, labels};

/// Ed25519 public key length.
pub const ED25519_PUBLIC_LEN: usize = 32;
/// Ed25519 signature length.
pub const ED25519_SIG_LEN: usize = 64;
/// ML-DSA-65 public key length (FIPS 204).
pub const MLDSA65_PUBLIC_LEN: usize = 1952;
/// ML-DSA-65 signature length (FIPS 204).
pub const MLDSA65_SIG_LEN: usize = 3309;

/// The exact bytes both algorithms sign.
fn to_be_signed(label: &str, msg: &[u8]) -> Vec<u8> {
    let label = label.as_bytes();
    let len = u8::try_from(label.len()).expect("labels are short constants");
    let mut out = Vec::with_capacity(1 + label.len() + msg.len());
    out.push(len);
    out.extend_from_slice(label);
    out.extend_from_slice(msg);
    out
}

/// A device's private hybrid signing key. Wiped from memory on drop.
pub struct HybridSigningKey {
    ed: ed25519_dalek::SigningKey,
    ml: ml_dsa::SigningKey<MlDsa65>,
}

impl HybridSigningKey {
    /// Generate a new key pair from the operating system's randomness.
    pub fn generate() -> Result<Self, Error> {
        let mut seeds = Zeroizing::new([0u8; 64]);
        getrandom::fill(&mut seeds[..]).map_err(|_| Error::Rng)?;
        Ok(Self::from_seeds(&seeds))
    }

    /// Rebuild a key from the 64-byte seed produced by [`Self::to_seeds`]:
    /// the Ed25519 secret key followed by the ML-DSA-65 seed.
    pub fn from_seeds(seeds: &[u8; 64]) -> Self {
        let (ed, ml) = seeds.split_at(32);
        let ed: &[u8; 32] = ed.try_into().expect("split at 32");
        let ml = ml_dsa::Seed::try_from(ml).expect("split at 32");
        Self { ed: ed25519_dalek::SigningKey::from_bytes(ed), ml: ml_dsa::SigningKey::<MlDsa65>::from_seed(&ml) }
    }

    /// The 64 secret bytes from which this key can be rebuilt. For storage only.
    pub fn to_seeds(&self) -> Zeroizing<[u8; 64]> {
        let mut out = Zeroizing::new([0u8; 64]);
        out[..32].copy_from_slice(&self.ed.to_bytes());
        out[32..].copy_from_slice(self.ml.as_seed());
        out
    }

    /// The public half.
    pub fn verifying_key(&self) -> HybridVerifyingKey {
        let ml = self.ml.expanded_key().verifying_key();
        HybridVerifyingKey { ed: self.ed.verifying_key(), ml_bytes: ml.encode().to_vec().into_boxed_slice(), ml }
    }

    /// Sign `msg` for the purpose named by `label`.
    pub fn sign(&self, label: &str, msg: &[u8]) -> Result<HybridSignature, Error> {
        let tbs = to_be_signed(label, msg);
        let ed = self.ed.sign(&tbs).to_bytes();
        let ml = self
            .ml
            .expanded_key()
            .sign_randomized(&tbs, &[], &mut getrandom::SysRng)
            .map_err(|_| Error::Rng)?
            .encode()
            .to_vec()
            .into_boxed_slice();
        Ok(HybridSignature { ed, ml })
    }
}

impl fmt::Debug for HybridSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HybridSigningKey").finish_non_exhaustive()
    }
}

/// A hybrid public key.
#[derive(Clone)]
pub struct HybridVerifyingKey {
    ed: ed25519_dalek::VerifyingKey,
    ml: ml_dsa::VerifyingKey<MlDsa65>,
    ml_bytes: Box<[u8]>,
}

impl HybridVerifyingKey {
    /// Parse a public key from its two encoded halves.
    ///
    /// Rejects Ed25519 keys that are not valid points or are of small order.
    pub fn from_parts(ed: &[u8], ml: &[u8]) -> Result<Self, Error> {
        let ed: &[u8; ED25519_PUBLIC_LEN] = ed.try_into().map_err(|_| Error::Malformed("ed25519 public key"))?;
        let ed = ed25519_dalek::VerifyingKey::from_bytes(ed).map_err(|_| Error::Malformed("ed25519 public key"))?;
        if ed.is_weak() {
            return Err(Error::Malformed("ed25519 public key"));
        }
        let enc = EncodedVerifyingKey::<MlDsa65>::try_from(ml).map_err(|_| Error::Malformed("ml-dsa-65 public key"))?;
        Ok(Self { ed, ml: ml_dsa::VerifyingKey::<MlDsa65>::decode(&enc), ml_bytes: ml.into() })
    }

    /// The Ed25519 half, encoded.
    pub fn ed25519_bytes(&self) -> [u8; ED25519_PUBLIC_LEN] {
        self.ed.to_bytes()
    }

    /// The ML-DSA-65 half, encoded.
    pub fn mldsa65_bytes(&self) -> &[u8] {
        &self.ml_bytes
    }

    /// Verify a signature made with [`HybridSigningKey::sign`] and the same label.
    /// Both halves must verify.
    pub fn verify(&self, label: &str, msg: &[u8], sig: &HybridSignature) -> Result<(), Error> {
        let tbs = to_be_signed(label, msg);
        let ed_ok = self.ed.verify_strict(&tbs, &ed25519_dalek::Signature::from_bytes(&sig.ed)).is_ok();
        let ml_ok =
            ml_dsa::Signature::<MlDsa65>::try_from(&sig.ml[..]).map(|s| self.ml.verify_with_context(&tbs, &[], &s)).unwrap_or(false);
        if ed_ok && ml_ok { Ok(()) } else { Err(Error::BadSignature) }
    }

    /// The ID of the device that owns this key: the first 16 bytes of
    /// `SHA-256(label ‖ ed25519 public ‖ ml-dsa-65 public)`.
    pub fn device_id(&self) -> DeviceId {
        let mut h = Sha256::new();
        h.update(to_be_signed(labels::DEVICE_ID, &[]));
        h.update(self.ed.as_bytes());
        h.update(&self.ml_bytes);
        let digest = h.finalize();
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest[..16]);
        DeviceId(id)
    }
}

impl PartialEq for HybridVerifyingKey {
    fn eq(&self, other: &Self) -> bool {
        self.ed == other.ed && self.ml_bytes == other.ml_bytes
    }
}
impl Eq for HybridVerifyingKey {}

impl fmt::Debug for HybridVerifyingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HybridVerifyingKey({})", self.device_id())
    }
}

/// A hybrid signature: both halves, encoded.
#[derive(Clone, PartialEq, Eq)]
pub struct HybridSignature {
    ed: [u8; ED25519_SIG_LEN],
    ml: Box<[u8]>,
}

impl HybridSignature {
    /// Parse a signature from its two encoded halves. Checks lengths only;
    /// validity is checked by [`HybridVerifyingKey::verify`].
    pub fn from_parts(ed: &[u8], ml: &[u8]) -> Result<Self, Error> {
        let ed = ed.try_into().map_err(|_| Error::Malformed("ed25519 signature"))?;
        if ml.len() != MLDSA65_SIG_LEN {
            return Err(Error::Malformed("ml-dsa-65 signature"));
        }
        Ok(Self { ed, ml: ml.into() })
    }

    /// The Ed25519 half.
    pub fn ed25519_bytes(&self) -> &[u8; ED25519_SIG_LEN] {
        &self.ed
    }

    /// The ML-DSA-65 half.
    pub fn mldsa65_bytes(&self) -> &[u8] {
        &self.ml
    }
}

impl fmt::Debug for HybridSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HybridSignature({}…)", hex::encode(&self.ed[..8]))
    }
}

/// A device's ID, derived from its chain keys. See [`HybridVerifyingKey::device_id`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(pub [u8; 16]);

impl DeviceId {
    /// Parse from 16 bytes.
    pub fn from_slice(b: &[u8]) -> Result<Self, Error> {
        Ok(Self(b.try_into().map_err(|_| Error::Malformed("device id"))?))
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> HybridSigningKey {
        HybridSigningKey::generate().expect("rng")
    }

    #[test]
    fn sizes_match_fips_204() {
        let k = key();
        let vk = k.verifying_key();
        let sig = k.sign("test", b"m").expect("sign");
        assert_eq!(vk.mldsa65_bytes().len(), MLDSA65_PUBLIC_LEN);
        assert_eq!(sig.mldsa65_bytes().len(), MLDSA65_SIG_LEN);
    }

    #[test]
    fn sign_verify_roundtrip() {
        let k = key();
        let sig = k.sign("test", b"hello").expect("sign");
        k.verifying_key().verify("test", b"hello", &sig).expect("verify");
    }

    #[test]
    fn wrong_message_label_or_key_fails() {
        let k = key();
        let sig = k.sign("test", b"hello").expect("sign");
        let vk = k.verifying_key();
        assert_eq!(vk.verify("test", b"hellO", &sig), Err(Error::BadSignature));
        assert_eq!(vk.verify("other", b"hello", &sig), Err(Error::BadSignature));
        assert_eq!(key().verifying_key().verify("test", b"hello", &sig), Err(Error::BadSignature));
    }

    #[test]
    fn label_boundary_is_unambiguous() {
        // ("ab", "c") and ("a", "bc") must not produce the same signed bytes.
        assert_ne!(to_be_signed("ab", b"c"), to_be_signed("a", b"bc"));
    }

    #[test]
    fn either_half_alone_is_not_enough() {
        let k = key();
        let vk = k.verifying_key();
        let good = k.sign("test", b"m").expect("sign");
        let other = key().sign("test", b"m").expect("sign");

        // Good Ed25519, foreign ML-DSA.
        let mixed = HybridSignature::from_parts(&good.ed, &other.ml).expect("parts");
        assert_eq!(vk.verify("test", b"m", &mixed), Err(Error::BadSignature));
        // Foreign Ed25519, good ML-DSA.
        let mixed = HybridSignature::from_parts(&other.ed, &good.ml).expect("parts");
        assert_eq!(vk.verify("test", b"m", &mixed), Err(Error::BadSignature));
        // Zeroed halves.
        let zero_ml = HybridSignature::from_parts(&good.ed, &[0u8; MLDSA65_SIG_LEN]).expect("parts");
        assert_eq!(vk.verify("test", b"m", &zero_ml), Err(Error::BadSignature));
        let zero_ed = HybridSignature::from_parts(&[0u8; 64], &good.ml).expect("parts");
        assert_eq!(vk.verify("test", b"m", &zero_ed), Err(Error::BadSignature));
    }

    #[test]
    fn seeds_roundtrip() {
        let k = key();
        let k2 = HybridSigningKey::from_seeds(&k.to_seeds());
        assert_eq!(k.verifying_key(), k2.verifying_key());
        let sig = k2.sign("test", b"m").expect("sign");
        k.verifying_key().verify("test", b"m", &sig).expect("verify");
    }

    #[test]
    fn public_key_parsing() {
        let vk = key().verifying_key();
        let parsed = HybridVerifyingKey::from_parts(&vk.ed25519_bytes(), vk.mldsa65_bytes()).expect("parse");
        assert_eq!(parsed, vk);
        assert_eq!(parsed.device_id(), vk.device_id());

        assert!(HybridVerifyingKey::from_parts(&[1u8; 31], vk.mldsa65_bytes()).is_err());
        assert!(HybridVerifyingKey::from_parts(&vk.ed25519_bytes(), &vk.mldsa65_bytes()[1..]).is_err());
        // The identity point is a small-order (weak) key.
        let mut identity = [0u8; 32];
        identity[0] = 1;
        assert!(HybridVerifyingKey::from_parts(&identity, vk.mldsa65_bytes()).is_err());
    }

    #[test]
    fn signature_parsing_checks_lengths() {
        assert!(HybridSignature::from_parts(&[0u8; 63], &[0u8; MLDSA65_SIG_LEN]).is_err());
        assert!(HybridSignature::from_parts(&[0u8; 64], &[0u8; MLDSA65_SIG_LEN - 1]).is_err());
    }

    #[test]
    fn device_ids_differ() {
        assert_ne!(key().verifying_key().device_id(), key().verifying_key().device_id());
    }

    #[test]
    fn debug_does_not_print_secrets() {
        let k = key();
        let seeds = k.to_seeds();
        let dbg = format!("{k:?}");
        assert!(!dbg.contains(&hex::encode(&seeds[..8])));
    }
}
