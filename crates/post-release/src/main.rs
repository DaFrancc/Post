//! `post-release`: make the release key, describe a release's files in a
//! manifest, sign it, and check it (design section 12, "App updates").
//!
//! The release key's secret half stays on the maintainer's machine, sealed
//! with a passphrase; CI builds the files and an unsigned manifest, and the
//! maintainer signs it here (see `scripts/release.sh sign`).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use post_release::{FileEntry, Manifest, Stretch};
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(name = "post-release", about = "Make and sign Post releases")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Make a new release key: its secret half sealed with a passphrase, and
    /// its public half (hex) for apps to build in.
    Keygen {
        /// Where to write the sealed secret key. Keep it offline, with a backup.
        #[arg(long)]
        out: PathBuf,
        /// Where to write the public key (crates/post-release/release-key.pub).
        #[arg(long)]
        public: PathBuf,
    },
    /// Describe a release's files for one platform, unsigned.
    Manifest {
        #[arg(long)]
        version: String,
        #[arg(long)]
        platform: String,
        /// The oldest version still able to talk to servers. Default: 0.0.0.
        #[arg(long, default_value = "0.0.0")]
        min_version: String,
        /// What changed, as plain text.
        #[arg(long)]
        notes_file: Option<PathBuf>,
        /// Where to write the manifest.
        #[arg(long)]
        out: PathBuf,
        /// The release's files.
        files: Vec<PathBuf>,
    },
    /// Sign a manifest, after checking its files (in the manifest's
    /// directory, unless --dir), writing MANIFEST.sig.
    Sign {
        /// The sealed release key.
        #[arg(long)]
        key: PathBuf,
        /// Where the files are. Default: next to the manifest.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Sign even if the key is not the one built into apps (for testing).
        #[arg(long)]
        any_key: bool,
        manifest: PathBuf,
    },
    /// Check a signed manifest against the built-in release key, and its
    /// files if they are there.
    Check {
        #[arg(long)]
        dir: Option<PathBuf>,
        manifest: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Keygen { out, public } => keygen(&out, &public),
        Command::Manifest { version, platform, min_version, notes_file, out, files } => {
            let notes = match notes_file {
                Some(p) => fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?,
                None => String::new(),
            };
            let mut entries = Vec::new();
            for f in &files {
                let name = f.file_name().and_then(|n| n.to_str()).context("a file needs a plain name")?;
                let (size, sha256) = hash_file(f)?;
                entries.push(FileEntry { name: name.into(), size, sha256: hex::encode(sha256) });
            }
            let released_at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64;
            let m = Manifest { version, platform, files: entries, min_version, notes: notes.trim().into(), released_at_ms };
            m.validate()?;
            let mut bytes = serde_json::to_vec_pretty(&m)?;
            bytes.push(b'\n');
            fs::write(&out, bytes)?;
            println!("wrote {}", out.display());
            Ok(())
        }
        Command::Sign { key, dir, any_key, manifest } => {
            let bytes = fs::read(&manifest)?;
            let m: Manifest = serde_json::from_slice(&bytes).context("reading the manifest")?;
            m.validate()?;
            check_files(&m, &dir_of(&manifest, dir))?;
            let sealed = fs::read(&key).with_context(|| format!("reading {}", key.display()))?;
            let passphrase = rpassword::prompt_password("Release key passphrase: ")?;
            let signer = post_release::open_key(&sealed, &passphrase)?;
            if !any_key && post_release::release_key().ok() != Some(signer.verifying_key()) {
                bail!("this is not the release key built into apps (pass --any-key to sign anyway)");
            }
            let sig = post_release::sign(&signer, &bytes)?;
            post_release::verify_with(&signer.verifying_key(), &bytes, &sig)?;
            let out = PathBuf::from(format!("{}.sig", manifest.display()));
            fs::write(&out, sig)?;
            println!("signed {} for {}: {}", m.version, m.platform, out.display());
            Ok(())
        }
        Command::Check { dir, manifest } => {
            let bytes = fs::read(&manifest)?;
            let sig = fs::read(format!("{}.sig", manifest.display())).context("reading the signature (MANIFEST.sig)")?;
            let m = post_release::verify(&bytes, &sig)?;
            let dir = dir_of(&manifest, dir);
            if m.files.iter().all(|f| dir.join(&f.name).exists()) {
                check_files(&m, &dir)?;
                println!("{} {} verifies, and its files match", m.platform, m.version);
            } else {
                println!("{} {} verifies (files not checked: not all in {})", m.platform, m.version, dir.display());
            }
            Ok(())
        }
    }
}

fn keygen(out: &Path, public: &Path) -> anyhow::Result<()> {
    if out.exists() {
        bail!("{} already exists; a release key is never overwritten", out.display());
    }
    let passphrase = rpassword::prompt_password("New passphrase for the release key: ")?;
    if passphrase.chars().count() < 12 {
        bail!("use a passphrase of at least 12 characters");
    }
    if rpassword::prompt_password("Again: ")? != passphrase {
        bail!("the passphrases differ");
    }
    let key = post_identity::HybridSigningKey::generate()?;
    let sealed = post_release::seal_key(&key, &passphrase, Stretch::DEFAULT)?;
    // Check it opens before anything depends on it.
    post_release::open_key(&sealed, &passphrase)?;
    write_private(out, &sealed)?;
    fs::write(public, format!("{}\n", hex::encode(post_release::public_key_to_bytes(&key.verifying_key()))))?;
    println!("secret key: {} (keep it offline, with a backup; losing it means apps must be reinstalled by hand)", out.display());
    println!("public key: {}", public.display());
    Ok(())
}

/// Write a file only its owner can read.
fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    std::io::Write::write_all(&mut opts.open(path)?, bytes)?;
    Ok(())
}

fn dir_of(manifest: &Path, dir: Option<PathBuf>) -> PathBuf {
    dir.unwrap_or_else(|| manifest.parent().map(Path::to_path_buf).unwrap_or_default())
}

fn check_files(m: &Manifest, dir: &Path) -> anyhow::Result<()> {
    for f in &m.files {
        let (size, sha256) = hash_file(&dir.join(&f.name))?;
        f.check(size, &sha256)?;
    }
    Ok(())
}

fn hash_file(path: &Path) -> anyhow::Result<(u64, [u8; 32])> {
    let mut file = fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        h.update(&buf[..n]);
    }
    Ok((size, h.finalize().into()))
}
