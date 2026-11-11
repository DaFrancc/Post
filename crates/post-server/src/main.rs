//! `postd` command line.
//!
//! ```text
//! postd keygen /etc/post/db.key
//! postd serve  --domain chat.example.org --db /var/lib/post/post.db --db-key-file /etc/post/db.key
//! postd invite --db /var/lib/post/post.db --db-key-file /etc/post/db.key [--bot] [--days 7]
//! ```
//!
//! The database key is read from a file, never from the command line or an
//! environment variable's value, so it does not appear in process listings.
//! Without `--db-key-file`, `$CREDENTIALS_DIRECTORY/post-db-key` is used
//! (systemd credentials, Podman secrets mounted the same way).
//!
//! Admin commands open the database directly; SQLite's locking makes that
//! safe while the server runs. There is no admin HTTP surface.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(name = "postd", version, about = "Post server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct DbArgs {
    /// SQLite database path.
    #[arg(long, env = "POST_DB", default_value = "/var/lib/post/post.db")]
    db: PathBuf,
    /// File holding the 32-byte database key (raw, or 64 hex characters).
    #[arg(long, env = "POST_DB_KEY_FILE")]
    db_key_file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new random database key file, readable only by its owner.
    Keygen {
        /// Where to write the key. Must not exist.
        path: PathBuf,
    },
    /// Run the server.
    Serve {
        /// Domain clients use for this server. Accounts are bound to it.
        #[arg(long, env = "POST_DOMAIN")]
        domain: String,
        #[command(flatten)]
        db: DbArgs,
        /// Address to listen on. Put a TLS proxy (Caddy) in front.
        #[arg(long, env = "POST_LISTEN", default_value = "127.0.0.1:8080")]
        listen: String,
        /// Where app releases are kept, to hand out to apps. Releases can
        /// also be copied in by hand (a folder per version).
        #[arg(long, env = "POST_RELEASES_DIR", default_value = "/var/lib/post/releases")]
        releases_dir: PathBuf,
        /// The GitHub repository to fetch signed releases from; empty to
        /// fetch none.
        #[arg(long, env = "POST_RELEASES_FROM", default_value = "DaFrancc/Post")]
        releases_from: String,
    },
    /// Create a single-use invite code.
    Invite {
        #[command(flatten)]
        db: DbArgs,
        /// The invite registers a bot account.
        #[arg(long)]
        bot: bool,
        /// Days until the invite expires.
        #[arg(long, default_value_t = 7)]
        days: u64,
    },
}

fn read_key(args: &DbArgs) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let path = match &args.db_key_file {
        Some(p) => p.clone(),
        None => match std::env::var_os("CREDENTIALS_DIRECTORY") {
            Some(dir) => Path::new(&dir).join("post-db-key"),
            None => bail!("no database key: pass --db-key-file, or provide the credential post-db-key"),
        },
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).with_context(|| format!("reading {}", path.display()))?.permissions().mode();
        if mode & 0o077 != 0 {
            bail!("{} is readable by others (mode {:o}); chmod 600 it", path.display(), mode & 0o777);
        }
    }
    let raw = Zeroizing::new(std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?);
    let mut key = Zeroizing::new([0u8; 32]);
    match raw.len() {
        32 => key.copy_from_slice(&raw),
        64 | 65 => {
            let text = std::str::from_utf8(&raw).context("key file is not hex")?.trim();
            hex::decode_to_slice(text, &mut key[..]).context("key file is not 64 hex characters")?;
        }
        n => bail!("key file must hold 32 raw bytes or 64 hex characters, found {n} bytes"),
    }
    Ok(key)
}

fn keygen(path: &Path) -> anyhow::Result<()> {
    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut key[..]).map_err(|e| anyhow::anyhow!("randomness unavailable: {e}"))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    use std::io::Write as _;
    let mut f = opts.open(path).with_context(|| format!("creating {}", path.display()))?;
    f.write_all(hex::encode(*key).as_bytes())?;
    f.sync_all()?;
    println!("wrote {}", path.display());
    Ok(())
}

/// Wait for Ctrl-C or SIGTERM. Containers and systemd stop services with
/// SIGTERM, and as a container's process 1 an unhandled SIGTERM is ignored.
async fn shutdown_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            r = tokio::signal::ctrl_c() => r?,
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    match Cli::parse().command {
        Command::Keygen { path } => keygen(&path)?,
        Command::Serve { domain, db, listen, releases_dir, releases_from } => {
            let db_key = read_key(&db)?;
            let listener = tokio::net::TcpListener::bind(&listen).await.with_context(|| format!("binding {listen}"))?;
            let key = match post_release::release_key() {
                Ok(k) => Some(k),
                Err(e) => {
                    tracing::warn!(error = %e, "no app releases will be handed out");
                    None
                }
            };
            let releases = post_server::ReleaseConfig {
                dir: releases_dir,
                github_repo: (!releases_from.is_empty()).then_some(releases_from),
                github_api: "https://api.github.com".into(),
                key,
                every: std::time::Duration::from_secs(3600),
            };
            let config = post_server::Config { server_domain: domain.clone(), db_path: db.db, db_key, releases };
            let running = post_server::spawn(listener, config)?;
            tracing::info!(addr = %running.addr, %domain, "postd listening");
            // SIGHUP (`systemctl reload`, `podman kill --signal HUP`): look
            // for new app releases now.
            let running = std::sync::Arc::new(running);
            #[cfg(unix)]
            let reload = {
                let running = running.clone();
                let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
                tokio::spawn(async move {
                    while hup.recv().await.is_some() {
                        running.refresh_releases();
                    }
                })
            };
            shutdown_signal().await?;
            #[cfg(unix)]
            {
                reload.abort();
                let _ = reload.await;
            }
            tracing::info!("shutting down");
            drop(running);
        }
        Command::Invite { db, bot, days } => {
            let key = read_key(&db)?;
            let mut conn = post_server::db::open(&db.db, &key).with_context(|| format!("opening {}", db.db.display()))?;
            let code = post_server::db::create_invite(&mut conn, bot, days * 24 * 3600 * 1000)?;
            println!("{code}");
        }
    }
    Ok(())
}
