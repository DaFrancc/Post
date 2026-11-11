//! Where the CLI finds its database key file (design section 11, "Headless
//! key storage"). Reading and creating key files is in
//! [`post_core::keyfile`], shared with the app.

use std::path::{Path, PathBuf};

use anyhow::bail;

pub use post_core::keyfile::{generate, read};

/// Where the key comes from: `--db-key-file`, else systemd/Podman credentials
/// at `$CREDENTIALS_DIRECTORY/post-db-key`.
pub fn locate(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    match std::env::var_os("CREDENTIALS_DIRECTORY") {
        Some(dir) => Ok(Path::new(&dir).join("post-db-key")),
        None => bail!("no database key: pass --db-key-file (create one with `post keygen PATH`)"),
    }
}
