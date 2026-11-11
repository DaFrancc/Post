//! Post's signed releases (design section 12, "App updates").
//!
//! A release is a set of files and, for each platform, a manifest naming
//! them with their sizes and SHA-256 hashes. The release key, held offline,
//! signs each manifest's exact bytes with a hybrid (Ed25519 + ML-DSA-65)
//! signature; its public half is built into every app. Wherever the files
//! come from (GitHub, a Post server, a mirror), a device installs them only
//! if the manifest verifies under the built-in key, the version is newer than
//! its own, and every file matches.
//!
//! Also here: the passphrase-sealed file that keeps the release key's secret
//! half, and (in `main.rs`) the `post-release` tool that makes and signs
//! manifests.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use post_identity::hybrid::{ED25519_PUBLIC_LEN, ED25519_SIG_LEN, MLDSA65_PUBLIC_LEN, MLDSA65_SIG_LEN};
use post_identity::{HybridSignature, HybridSigningKey, HybridVerifyingKey, labels};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Why a release, a key or a key file was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// This build has no release key, so it accepts no release.
    #[error("this build has no release key")]
    NoReleaseKey,
    /// The signature is not the release key's over these exact bytes.
    #[error("the release signature does not verify")]
    BadSignature,
    /// Bytes that do not decode as what they should be.
    #[error("malformed {0}")]
    Malformed(&'static str),
    /// The manifest decodes but breaks a rule (named).
    #[error("invalid manifest: {0}")]
    Invalid(&'static str),
    /// A downloaded file is not the one the manifest names.
    #[error("{0} does not match the release")]
    FileMismatch(String),
    /// The key file's passphrase is wrong, or the file was changed.
    #[error("wrong passphrase, or the key file is damaged")]
    WrongPassphrase,
    /// The system's random number generator failed.
    #[error("no randomness available")]
    Rng,
}

/// Result with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// The platform names releases are built for.
pub mod platform {
    /// Linux on x86-64: a tarball installed per user (design section 12).
    pub const LINUX_X86_64: &str = "linux-x86_64";

    /// The platform this program was built for, if releases are made for it.
    pub fn current() -> Option<&'static str> {
        (cfg!(target_os = "linux") && cfg!(target_arch = "x86_64")).then_some(LINUX_X86_64)
    }
}

/// The manifest's file name for a platform, as published.
pub fn manifest_name(platform: &str) -> String {
    format!("manifest-{platform}.json")
}

/// The signature's file name for a platform's manifest.
pub fn signature_name(platform: &str) -> String {
    format!("{}.sig", manifest_name(platform))
}

// ---------------------------------------------------------------- versions

/// A release version: `major.minor.patch`, compared numerically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl FromStr for Version {
    type Err = Error;

    /// Exactly three numbers, without leading zeros or a `v`.
    fn from_str(s: &str) -> Result<Self> {
        let part = |p: Option<&str>| -> Result<u32> {
            let p = p.ok_or(Error::Malformed("version"))?;
            let ok = !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && (p == "0" || !p.starts_with('0'));
            if !ok {
                return Err(Error::Malformed("version"));
            }
            p.parse().map_err(|_| Error::Malformed("version"))
        };
        let mut it = s.split('.');
        let v = Version(part(it.next())?, part(it.next())?, part(it.next())?);
        if it.next().is_some() {
            return Err(Error::Malformed("version"));
        }
        Ok(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.0, self.1, self.2).cmp(&(other.0, other.1, other.2))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------- manifests

/// What a release holds for one platform. Signed as the exact bytes it was
/// published as, so it needs no canonical form; fields a newer manifest
/// adds are ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// This release's version.
    pub version: String,
    /// The platform these files are for (see [`platform`]).
    pub platform: String,
    /// The files to download, all of them needed.
    pub files: Vec<FileEntry>,
    /// Older versions can no longer talk to servers running this release's
    /// protocol: apps below it say so prominently.
    pub min_version: String,
    /// What changed, as plain text.
    pub notes: String,
    /// When it was released (Unix ms).
    pub released_at_ms: u64,
}

/// One file of a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Its file name: no directories.
    pub name: String,
    /// Bytes.
    pub size: u64,
    /// SHA-256, lowercase hex.
    pub sha256: String,
}

/// Largest manifest accepted, against a hostile source handing out a huge one.
pub const MAX_MANIFEST: usize = 64 * 1024;

impl Manifest {
    /// The version, parsed.
    pub fn version(&self) -> Result<Version> {
        self.version.parse()
    }

    /// The oldest version still able to talk to servers, parsed.
    pub fn min_version(&self) -> Result<Version> {
        self.min_version.parse()
    }

    /// Check what can be checked without the files.
    pub fn validate(&self) -> Result<()> {
        let version = self.version()?;
        if self.min_version()? > version {
            return Err(Error::Invalid("min_version is newer than the release"));
        }
        if self.platform.is_empty() || !self.platform.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_') {
            return Err(Error::Invalid("platform"));
        }
        if self.files.is_empty() {
            return Err(Error::Invalid("no files"));
        }
        for (i, f) in self.files.iter().enumerate() {
            if !safe_file_name(&f.name) {
                return Err(Error::Invalid("file name"));
            }
            if self.files[..i].iter().any(|g| g.name == f.name) {
                return Err(Error::Invalid("file named twice"));
            }
            if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                return Err(Error::Invalid("sha256"));
            }
        }
        Ok(())
    }

    /// The file named `name`, if the release has one.
    pub fn file(&self, name: &str) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.name == name)
    }
}

/// A plain file name: something a downloader can write into its own
/// directory and nowhere else.
fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

impl FileEntry {
    /// Describe `bytes` as the file `name`.
    pub fn describe(name: &str, bytes: &[u8]) -> Self {
        Self { name: name.into(), size: bytes.len() as u64, sha256: hex::encode(Sha256::digest(bytes)) }
    }

    /// Check a downloaded file's size and hash, given as computed.
    pub fn check(&self, size: u64, sha256: &[u8; 32]) -> Result<()> {
        if size == self.size && hex::encode(sha256) == self.sha256 { Ok(()) } else { Err(Error::FileMismatch(self.name.clone())) }
    }
}

// ---------------------------------------------------------------- the release key

/// The release key's public half, hex (the Ed25519 half, then ML-DSA-65's),
/// built into every app. Empty in a build made before the key existed: such
/// a build accepts no release.
const RELEASE_KEY_HEX: &str = include_str!("../release-key.pub");

/// The release key this build trusts.
pub fn release_key() -> Result<HybridVerifyingKey> {
    let hex = RELEASE_KEY_HEX.trim();
    if hex.is_empty() {
        return Err(Error::NoReleaseKey);
    }
    let bytes = hex::decode(hex).map_err(|_| Error::Malformed("built-in release key"))?;
    public_key_from_bytes(&bytes)
}

/// A public key as stored: Ed25519's 32 bytes, then ML-DSA-65's.
pub fn public_key_from_bytes(bytes: &[u8]) -> Result<HybridVerifyingKey> {
    if bytes.len() != ED25519_PUBLIC_LEN + MLDSA65_PUBLIC_LEN {
        return Err(Error::Malformed("release key"));
    }
    let (ed, ml) = bytes.split_at(ED25519_PUBLIC_LEN);
    HybridVerifyingKey::from_parts(ed, ml).map_err(|_| Error::Malformed("release key"))
}

/// A public key's stored form (see [`public_key_from_bytes`]).
pub fn public_key_to_bytes(key: &HybridVerifyingKey) -> Vec<u8> {
    [&key.ed25519_bytes()[..], key.mldsa65_bytes()].concat()
}

/// Sign a manifest's exact bytes.
pub fn sign(key: &HybridSigningKey, manifest: &[u8]) -> Result<Vec<u8>> {
    let sig = key.sign(labels::RELEASE, manifest).map_err(|_| Error::Rng)?;
    Ok([&sig.ed25519_bytes()[..], sig.mldsa65_bytes()].concat())
}

/// Verify a manifest's signature under `key`, then read and check it.
pub fn verify_with(key: &HybridVerifyingKey, manifest: &[u8], signature: &[u8]) -> Result<Manifest> {
    if manifest.len() > MAX_MANIFEST {
        return Err(Error::Malformed("manifest"));
    }
    if signature.len() != ED25519_SIG_LEN + MLDSA65_SIG_LEN {
        return Err(Error::Malformed("signature"));
    }
    let (ed, ml) = signature.split_at(ED25519_SIG_LEN);
    let sig = HybridSignature::from_parts(ed, ml).map_err(|_| Error::Malformed("signature"))?;
    key.verify(labels::RELEASE, manifest, &sig).map_err(|_| Error::BadSignature)?;
    let m: Manifest = serde_json::from_slice(manifest).map_err(|_| Error::Malformed("manifest"))?;
    m.validate()?;
    Ok(m)
}

/// Verify a manifest under the built-in release key.
pub fn verify(manifest: &[u8], signature: &[u8]) -> Result<Manifest> {
    verify_with(&release_key()?, manifest, signature)
}

// ---------------------------------------------------------------- the key file

/// How hard the passphrase is stretched (Argon2id). Stored in the file, so
/// it can be raised later without breaking old files.
#[derive(Clone, Copy, Debug)]
pub struct Stretch {
    /// Memory, KiB.
    pub memory_kib: u32,
    /// Passes.
    pub passes: u32,
}

impl Stretch {
    /// For a key that signs a release now and then: a second or so, 256 MiB.
    pub const DEFAULT: Stretch = Stretch { memory_kib: 256 * 1024, passes: 3 };
}

const KEY_FILE_MAGIC: &[u8] = b"post-release-key/v1\n";

/// Seal the release key's secret half under a passphrase:
/// `magic ‖ memory_kib ‖ passes ‖ salt(16) ‖ nonce(24) ‖ sealed seeds`.
pub fn seal_key(key: &HybridSigningKey, passphrase: &str, stretch: Stretch) -> Result<Vec<u8>> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut salt).map_err(|_| Error::Rng)?;
    getrandom::fill(&mut nonce).map_err(|_| Error::Rng)?;
    let k = stretch_passphrase(passphrase, &salt, stretch)?;
    let sealed = XChaCha20Poly1305::new((&*k).into())
        .encrypt(XNonce::from_slice(&nonce), &key.to_seeds()[..])
        .map_err(|_| Error::Rng)?;
    let mut out = KEY_FILE_MAGIC.to_vec();
    out.extend_from_slice(&stretch.memory_kib.to_le_bytes());
    out.extend_from_slice(&stretch.passes.to_le_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Open a key file sealed by [`seal_key`].
pub fn open_key(file: &[u8], passphrase: &str) -> Result<HybridSigningKey> {
    let rest = file.strip_prefix(KEY_FILE_MAGIC).ok_or(Error::Malformed("key file"))?;
    if rest.len() != 4 + 4 + 16 + 24 + 64 + 16 {
        return Err(Error::Malformed("key file"));
    }
    let word = |b: &[u8]| u32::from_le_bytes(b.try_into().expect("four bytes"));
    let stretch = Stretch { memory_kib: word(&rest[0..4]), passes: word(&rest[4..8]) };
    // Within reason, so a doctored file cannot ask for terabytes.
    if !(8..=4 * 1024 * 1024).contains(&stretch.memory_kib) || !(1..=64).contains(&stretch.passes) {
        return Err(Error::Malformed("key file"));
    }
    let (salt, rest) = rest[8..].split_at(16);
    let (nonce, sealed) = rest.split_at(24);
    let k = stretch_passphrase(passphrase, salt, stretch)?;
    let seeds = Zeroizing::new(
        XChaCha20Poly1305::new((&*k).into()).decrypt(XNonce::from_slice(nonce), sealed).map_err(|_| Error::WrongPassphrase)?,
    );
    let seeds: &[u8; 64] = seeds[..].try_into().map_err(|_| Error::Malformed("key file"))?;
    Ok(HybridSigningKey::from_seeds(seeds))
}

fn stretch_passphrase(passphrase: &str, salt: &[u8], s: Stretch) -> Result<Zeroizing<[u8; 32]>> {
    let params = argon2::Params::new(s.memory_kib, s.passes, 1, Some(32)).map_err(|_| Error::Malformed("key file"))?;
    let mut out = Zeroizing::new([0u8; 32]);
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut out[..])
        .map_err(|_| Error::Malformed("key file"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str) -> Manifest {
        Manifest {
            version: version.into(),
            platform: platform::LINUX_X86_64.into(),
            files: vec![FileEntry::describe("post-1.2.0-linux-x86_64.tar.gz", b"the app")],
            min_version: "1.0.0".into(),
            notes: "Mentions.".into(),
            released_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn versions_compare_as_numbers() {
        let v = |s: &str| s.parse::<Version>();
        assert!(v("1.10.0").expect("v") > v("1.9.9").expect("v"));
        assert_eq!(v("0.0.0").expect("v"), Version(0, 0, 0));
        assert_eq!(v("2.3.4").expect("v").to_string(), "2.3.4");
        for bad in ["", "1", "1.2", "1.2.3.4", "v1.2.3", "01.2.3", "1.2.-3", "1.2.3-beta", "1..3", "99999999999.0.0"] {
            assert!(v(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn signed_manifests_verify_and_changed_ones_do_not() {
        let key = HybridSigningKey::generate().expect("key");
        let public = key.verifying_key();
        let bytes = serde_json::to_vec_pretty(&manifest("1.2.0")).expect("json");
        let sig = sign(&key, &bytes).expect("sign");
        assert_eq!(verify_with(&public, &bytes, &sig).expect("verifies"), manifest("1.2.0"));

        // One byte changed, or another key, or a signature for something else.
        let mut changed = bytes.clone();
        changed[20] ^= 1;
        assert_eq!(verify_with(&public, &changed, &sig), Err(Error::BadSignature));
        let other = HybridSigningKey::generate().expect("key").verifying_key();
        assert_eq!(verify_with(&other, &bytes, &sig), Err(Error::BadSignature));
        let link = key.sign(labels::CHAIN_LINK, &bytes).expect("sign");
        let link = [&link.ed25519_bytes()[..], link.mldsa65_bytes()].concat();
        assert_eq!(verify_with(&public, &bytes, &link), Err(Error::BadSignature), "a signature made for another purpose");
        assert_eq!(verify_with(&public, &bytes, &sig[..100]), Err(Error::Malformed("signature")));

        // Stored public keys read back.
        assert_eq!(public_key_from_bytes(&public_key_to_bytes(&public)).expect("key"), public);
    }

    #[test]
    fn manifests_follow_the_rules() {
        assert!(manifest("1.2.0").validate().is_ok());
        let broken = |f: fn(&mut Manifest)| {
            let mut m = manifest("1.2.0");
            f(&mut m);
            m.validate()
        };
        assert!(broken(|m| m.version = "1.2".into()).is_err());
        assert!(broken(|m| m.min_version = "1.3.0".into()).is_err());
        assert!(broken(|m| m.files.clear()).is_err());
        assert!(broken(|m| m.platform = "Linux".into()).is_err());
        for name in ["../evil", "a/b", ".hidden", "", "a b"] {
            let mut m = manifest("1.2.0");
            m.files[0].name = name.into();
            assert_eq!(m.validate(), Err(Error::Invalid("file name")), "{name:?}");
        }
        assert!(broken(|m| m.files[0].sha256 = "ABC".into()).is_err());
        assert!(broken(|m| m.files.push(m.files[0].clone())).is_err());
    }

    #[test]
    fn files_are_checked_against_the_manifest() {
        let f = FileEntry::describe("a.tar.gz", b"bytes");
        let digest: [u8; 32] = Sha256::digest(b"bytes").into();
        assert!(f.check(5, &digest).is_ok());
        assert!(f.check(6, &digest).is_err());
        assert!(f.check(5, &[0; 32]).is_err());
    }

    #[test]
    fn key_files_open_with_the_passphrase_only() {
        let key = HybridSigningKey::generate().expect("key");
        let quick = Stretch { memory_kib: 64, passes: 1 };
        let file = seal_key(&key, "correct horse", quick).expect("seal");
        let opened = open_key(&file, "correct horse").expect("open");
        assert_eq!(opened.verifying_key(), key.verifying_key());
        assert!(matches!(open_key(&file, "wrong horse"), Err(Error::WrongPassphrase)));
        let mut damaged = file.clone();
        *damaged.last_mut().expect("bytes") ^= 1;
        assert!(matches!(open_key(&damaged, "correct horse"), Err(Error::WrongPassphrase)));
        assert!(matches!(open_key(&file[..40], "correct horse"), Err(Error::Malformed(_))));
    }
}
