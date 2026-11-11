//! Database key files for headless use (design section 11, "Headless key
//! storage"). There is no unencrypted mode.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use postern_core::DbKey;
use zeroize::Zeroizing;

/// Where the key comes from: `--db-key-file`, else systemd/Podman credentials
/// at `$CREDENTIALS_DIRECTORY/postern-db-key`.
pub fn locate(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    match std::env::var_os("CREDENTIALS_DIRECTORY") {
        Some(dir) => Ok(Path::new(&dir).join("postern-db-key")),
        None => bail!("no database key: pass --db-key-file (create one with `postern keygen PATH`)"),
    }
}

/// Read a key file: 32 raw bytes or 64 hex characters, readable only by its
/// owner.
pub fn read(path: &Path) -> anyhow::Result<DbKey> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?.permissions().mode();
        if mode & 0o077 != 0 {
            bail!("{} is readable by others (mode {:o}); run chmod 600 on it", path.display(), mode & 0o777);
        }
    }
    let raw = Zeroizing::new(std::fs::read(path).with_context(|| format!("reading {}", path.display()))?);
    let mut key = [0u8; 32];
    match raw.len() {
        32 => key.copy_from_slice(&raw),
        64 | 65 => {
            let text = std::str::from_utf8(&raw).context("key file is not hex")?.trim();
            hex::decode_to_slice(text, &mut key).context("key file is not 64 hex characters")?;
        }
        n => bail!("key file must hold 32 raw bytes or 64 hex characters, found {n} bytes"),
    }
    let k = DbKey::from_bytes(key);
    zeroize::Zeroize::zeroize(&mut key);
    Ok(k)
}

/// Create a new random key file, readable only by its owner. Refuses to
/// overwrite an existing file.
pub fn generate(path: &Path) -> anyhow::Result<()> {
    let key = DbKey::generate()?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    use std::io::Write as _;
    let mut f = opts.open(path).with_context(|| format!("creating {}", path.display()))?;
    f.write_all(Zeroizing::new(hex::encode(key.bytes())).as_bytes())?;
    f.sync_all()?;
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
