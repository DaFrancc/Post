//! The profile database's key in the platform keystore (design section 11,
//! "Storage and crash safety"): 32 random bytes, made when a profile is
//! first opened and never written to disk by Post. On Linux that is the
//! Secret Service (GNOME Keyring, KWallet), reached over encrypted D-Bus.
//!
//! Everything here blocks on D-Bus; call it off the async runtime's workers.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context as _, anyhow, bail};
use keyring_core::{Entry, Error as KeyError};
use post_core::DbKey;
use zeroize::Zeroizing;

/// The keystore service name Post's entries are filed under.
const SERVICE: &str = "com.dafrancc.post";

/// Connect the platform keystore once per process.
fn ensure_store() -> anyhow::Result<()> {
    static STORE: OnceLock<Result<(), String>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            #[cfg(target_os = "linux")]
            {
                zbus_secret_service_keyring_store::Store::new().map(|s| keyring_core::set_default_store(s)).map_err(|e| e.to_string())
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err("this platform's keystore is not supported yet".to_string())
            }
        })
        .clone()
        .map_err(|e| anyhow!("no keyring is available ({e}); on Linux, a Secret Service such as GNOME Keyring or KWallet must be running"))
}

/// The entry name for a profile: its directory's canonical path, so two
/// profiles never share a key.
fn entry_for(dir: &Path) -> anyhow::Result<Entry> {
    let id = dir.canonicalize().with_context(|| format!("finding {}", dir.display()))?;
    Ok(Entry::new(SERVICE, &format!("profile:{}", id.display()))?)
}

/// The key for the profile in `dir`, creating the directory and a fresh key
/// for a new profile. A profile whose key is missing from the keystore is
/// refused rather than replaced, since a new key could never open it.
pub fn key_for(dir: &Path) -> anyhow::Result<DbKey> {
    ensure_store()?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    load_or_create(&entry_for(dir)?, dir.join("profile.db").exists())
}

/// Delete the profile in `dir` (see `post_core::delete_profile`), then its
/// key from the keystore. In that order: a key without its profile is only
/// clutter, but a profile without its key could never be opened again.
pub fn delete_profile(dir: &Path) -> anyhow::Result<()> {
    ensure_store()?;
    // Named by the canonical path, so found before the folder goes.
    let entry = entry_for(dir)?;
    post_core::delete_profile(dir)?;
    match entry.delete_credential() {
        Ok(()) | Err(KeyError::NoEntry) => Ok(()),
        Err(e) => Err(anyhow!("the profile is deleted, but the keyring kept its key: {e}")),
    }
}

fn load_or_create(entry: &Entry, profile_exists: bool) -> anyhow::Result<DbKey> {
    match entry.get_secret() {
        Ok(secret) => key_from(Zeroizing::new(secret)),
        Err(KeyError::NoEntry) if profile_exists => {
            bail!("this profile's database key is not in the keyring, so it cannot be opened")
        }
        Err(KeyError::NoEntry) => {
            let key = DbKey::generate()?;
            entry.set_secret(key.bytes()).context("storing the database key in the keyring")?;
            // Read it back before any data depends on it.
            let back = key_from(Zeroizing::new(entry.get_secret().context("reading the new key back")?))?;
            if back.bytes() != key.bytes() {
                bail!("the keyring returned a different key than was stored");
            }
            Ok(key)
        }
        Err(e) => Err(anyhow!("the keyring refused: {e}")),
    }
}

fn key_from(secret: Zeroizing<Vec<u8>>) -> anyhow::Result<DbKey> {
    let bytes: [u8; 32] = secret.as_slice().try_into().map_err(|_| anyhow!("the keyring holds a malformed database key"))?;
    Ok(DbKey::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_made_once_and_never_replaced() {
        keyring_core::set_default_store(keyring_core::mock::Store::new().expect("mock store"));
        let entry = Entry::new(SERVICE, "profile:/test/a").expect("entry");

        // A profile that exists without its key is refused, not given a new one.
        assert!(load_or_create(&entry, true).is_err());
        // A new profile gets a key, and gets the same one next time.
        let first = load_or_create(&entry, false).expect("create");
        let again = load_or_create(&entry, true).expect("load");
        assert_eq!(first.bytes(), again.bytes());
        // A malformed entry is an error, never silently replaced.
        entry.set_secret(b"short").expect("set");
        assert!(load_or_create(&entry, true).is_err());
    }
}
