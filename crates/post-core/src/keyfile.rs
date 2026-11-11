//! Database key files, for where there is no platform keystore: headless
//! use (design section 11, "Headless key storage") and test profiles. A key
//! file holds 64 hex characters (or 32 raw bytes) and must be readable by
//! its owner only. There is no unencrypted mode.

use std::path::Path;

use zeroize::Zeroizing;

use crate::{DbKey, Error, Result};

/// Read a key file: 32 raw bytes or 64 hex characters, readable only by its
/// owner.
pub fn read(path: &Path) -> Result<DbKey> {
    let io = |e: std::io::Error| Error::Storage(format!("reading {}: {e}", path.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path).map_err(io)?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(Error::InvalidInput(format!(
                "{} is readable by others (mode {:o}); run chmod 600 on it",
                path.display(),
                mode & 0o777
            )));
        }
    }
    let raw = Zeroizing::new(std::fs::read(path).map_err(io)?);
    let mut key = Zeroizing::new([0u8; 32]);
    match raw.len() {
        32 => key.copy_from_slice(&raw),
        64 | 65 => {
            let bad = || Error::InvalidInput("key file is not 64 hex characters".into());
            let text = std::str::from_utf8(&raw).map_err(|_| bad())?.trim();
            hex::decode_to_slice(text, &mut key[..]).map_err(|_| bad())?;
        }
        n => return Err(Error::InvalidInput(format!("key file must hold 32 raw bytes or 64 hex characters, found {n} bytes"))),
    }
    Ok(DbKey::from_bytes(*key))
}

/// Create a new random key file, readable only by its owner. Refuses to
/// overwrite an existing file.
pub fn generate(path: &Path) -> Result<()> {
    use std::io::Write as _;
    let key = DbKey::generate()?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let io = |e: std::io::Error| Error::Storage(format!("creating {}: {e}", path.display()));
    let mut f = opts.open(path).map_err(io)?;
    f.write_all(Zeroizing::new(hex::encode(key.bytes())).as_bytes()).map_err(io)?;
    f.sync_all().map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_then_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("key");
        generate(&path).expect("generate");
        let a = read(&path).expect("read");
        assert!(generate(&path).is_err(), "never overwrites");
        let b = read(&path).expect("read again");
        assert_eq!(a.bytes(), b.bytes());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_readable_key() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("key");
        generate(&path).expect("generate");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(read(&path).is_err());
    }
}
