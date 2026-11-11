//! Where the CLI finds its database key file (design section 11, "Headless
//! key storage"). Reading and creating key files is in
//! [`postern_core::keyfile`], shared with the app.

use std::path::{Path, PathBuf};

use anyhow::bail;

pub use postern_core::keyfile::{generate, read};

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
