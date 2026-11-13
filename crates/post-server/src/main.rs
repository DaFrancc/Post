//! `postd` command line.
//!
//! ```text
//! postd keygen /etc/post/db.key
//! postd serve  --domain chat.example.org --db /var/lib/post/post.db --db-key-file /etc/post/db.key
//! postd invite --db /var/lib/post/post.db --db-key-file /etc/post/db.key [--bot] [--days 7]
//! postd host add|remove USER, postd host list
//! postd announce TEXT
//! postd maintenance plan [--at TIME] --minutes N [--reason TEXT] [--quiet]
//! postd maintenance end|status
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
use post_server::announce::{self, By};
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

/// Calls, through the LiveKit server beside postd (design section 10).
#[derive(clap::Args)]
struct CallArgs {
    /// LiveKit's WebSocket URL as apps reach it, such as
    /// wss://rtc.example.org. Without it, this server offers no calls.
    #[arg(long, env = "POST_LIVEKIT_URL")]
    livekit_url: Option<String>,
    /// LiveKit's API key name (the `keys:` entry in its config).
    #[arg(long, env = "POST_LIVEKIT_KEY", default_value = "post")]
    livekit_key: String,
    /// File holding LiveKit's API secret. Default: the credential
    /// post-livekit-secret ($CREDENTIALS_DIRECTORY).
    #[arg(long, env = "POST_LIVEKIT_SECRET_FILE")]
    livekit_secret_file: Option<PathBuf>,
    /// The most each participant may send in a call, kbit/s.
    #[arg(long, env = "POST_CALL_MAX_BITRATE_KBPS", default_value_t = 2500)]
    call_max_bitrate_kbps: u32,
    /// The most participants a call may have.
    #[arg(long, env = "POST_CALL_MAX_PARTICIPANTS", default_value_t = 25)]
    call_max_participants: u32,
}

impl CallArgs {
    fn config(self) -> anyhow::Result<Option<post_server::CallConfig>> {
        let Some(url) = self.livekit_url else { return Ok(None) };
        let path = match self.livekit_secret_file {
            Some(p) => p,
            None => match std::env::var_os("CREDENTIALS_DIRECTORY") {
                Some(dir) => Path::new(&dir).join("post-livekit-secret"),
                None => bail!("calls need LiveKit's API secret: pass --livekit-secret-file, or provide the credential post-livekit-secret"),
            },
        };
        let file = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        // The secret alone, or LiveKit's own key file (`<key>: <secret>`
        // lines), so one file serves both.
        let secret = file
            .lines()
            .find_map(|l| l.trim().strip_prefix(&format!("{}:", self.livekit_key)).map(str::trim))
            .unwrap_or_else(|| file.trim());
        if secret.len() < 32 {
            bail!("LiveKit's API secret in {} is too short: use at least 32 random characters", path.display());
        }
        Ok(Some(post_server::CallConfig {
            url,
            api_key: self.livekit_key,
            api_secret: Zeroizing::new(secret.as_bytes().to_vec()),
            max_bitrate_kbps: self.call_max_bitrate_kbps,
            max_participants: self.call_max_participants,
        }))
    }
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
        /// Look for a new app release every hour.
        #[arg(long, env = "POST_RELEASES_CHECK", default_value_t = true, action = clap::ArgAction::Set)]
        releases_check: bool,
        /// Download a new release when one is found, ready to go live.
        #[arg(long, env = "POST_RELEASES_DOWNLOAD", default_value_t = true, action = clap::ArgAction::Set)]
        releases_download: bool,
        /// Offer a new release to apps as soon as it has downloaded. Off,
        /// it goes live at the next start, or with a reload (SIGHUP).
        #[arg(long, env = "POST_RELEASES_LIVE_WHILE_RUNNING", default_value_t = false, action = clap::ArgAction::Set)]
        releases_live_while_running: bool,
        /// On starting, offer apps the newest downloaded release, and one
        /// the look for releases at start finds. Off, the release offered
        /// before the restart stays.
        #[arg(long, env = "POST_RELEASES_LIVE_ON_START", default_value_t = true, action = clap::ArgAction::Set)]
        releases_live_on_start: bool,
        #[command(flatten)]
        calls: CallArgs,
        /// Where profile pictures and banners are kept, encrypted. Default:
        /// a `blobs` folder beside the database.
        #[arg(long, env = "POST_BLOBS_DIR")]
        blobs_dir: Option<PathBuf>,
        /// The most room profile pictures and banners may take together, in
        /// MB. Each is at most half a megabyte.
        #[arg(long, env = "POST_BLOB_STORE_MB", default_value_t = 4096)]
        blob_store_mb: u64,
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
    /// Choose the hosts: accounts that may post announcements and plan
    /// maintenance from the app.
    Host {
        #[command(flatten)]
        db: DbArgs,
        #[command(subcommand)]
        action: HostAction,
    },
    /// Post an announcement to everyone on the server. The running server
    /// sends it out within a few seconds.
    Announce {
        #[command(flatten)]
        db: DbArgs,
        /// The text; `-` reads it from standard input.
        text: String,
    },
    /// Plan maintenance, so everyone is told and reminded beforehand; end
    /// it; or show the plan.
    Maintenance {
        #[command(flatten)]
        db: DbArgs,
        #[command(subcommand)]
        action: MaintenanceAction,
    },
}

#[derive(Subcommand)]
enum HostAction {
    /// Make an account a host.
    Add { username: String },
    /// Stop an account being a host.
    Remove { username: String },
    /// List the hosts.
    List,
}

#[derive(Subcommand)]
enum MaintenanceAction {
    /// Plan maintenance. Everyone is told now (unless --quiet), reminded a
    /// day, an hour, 30 minutes and 5 minutes before, and told when it
    /// starts and when it is over.
    Plan {
        /// When it starts: RFC 3339 (2026-11-14T02:00:00+01:00), or
        /// @UNIX-SECONDS. Leave it out to start now.
        #[arg(long, value_parser = parse_time)]
        at: Option<u64>,
        /// How long it should take.
        #[arg(long)]
        minutes: u32,
        /// Why, shown to everyone.
        #[arg(long, default_value = "")]
        reason: String,
        /// Do not announce the plan now; only the reminders.
        #[arg(long)]
        quiet: bool,
    },
    /// Cancel the planned maintenance, or announce the one underway is over.
    End,
    /// Show the maintenance planned or underway.
    Status,
}

/// A time as Unix milliseconds, from RFC 3339 or `@` and Unix seconds.
fn parse_time(s: &str) -> Result<u64, String> {
    if let Some(secs) = s.strip_prefix('@') {
        return secs.parse::<u64>().map(|n| n * 1000).map_err(|_| format!("{s} is not @ and a number of seconds"));
    }
    let t = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .map_err(|e| format!("{s} is not an RFC 3339 time such as 2026-11-14T02:00:00+01:00: {e}"))?;
    u64::try_from(t.unix_timestamp_nanos() / 1_000_000).map_err(|_| format!("{s} is before 1970"))
}

fn format_time(ms: u64) -> String {
    let t = i64::try_from(ms / 1000).ok().and_then(|s| time::OffsetDateTime::from_unix_timestamp(s).ok());
    t.and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()).unwrap_or_else(|| format!("@{}", ms / 1000))
}

fn open_db(db: &DbArgs) -> anyhow::Result<rusqlite::Connection> {
    let key = read_key(db)?;
    post_server::db::open(&db.db, &key).with_context(|| format!("opening {}", db.db.display()))
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
        Command::Serve {
            domain,
            db,
            listen,
            releases_dir,
            releases_from,
            releases_check,
            releases_download,
            releases_live_while_running,
            releases_live_on_start,
            calls,
            blobs_dir,
            blob_store_mb,
        } => {
            let db_key = read_key(&db)?;
            let calls = calls.config()?;
            match &calls {
                Some(c) => tracing::info!(livekit = %c.url, "calls on"),
                None => tracing::info!("calls off: no --livekit-url"),
            }
            let listener = tokio::net::TcpListener::bind(&listen).await.with_context(|| format!("binding {listen}"))?;
            let key = match post_release::release_key() {
                Ok(k) => Some(k),
                Err(e) => {
                    tracing::warn!(error = %e, "no app releases will be handed out");
                    None
                }
            };
            let releases = post_server::ReleaseConfig {
                github_repo: (!releases_from.is_empty()).then_some(releases_from),
                check: releases_check,
                download: releases_download,
                live_while_running: releases_live_while_running,
                live_on_start: releases_live_on_start,
                ..post_server::ReleaseConfig::new(releases_dir, key)
            };
            let mut blobs = post_server::BlobConfig::beside(&db.db);
            if let Some(dir) = blobs_dir {
                blobs.dir = dir;
            }
            blobs.max_total_bytes = blob_store_mb.saturating_mul(1024 * 1024);
            let config = post_server::Config { server_domain: domain.clone(), db_path: db.db, db_key, releases, calls, blobs };
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
            let mut conn = open_db(&db)?;
            let code = post_server::db::create_invite(&mut conn, bot, days * 24 * 3600 * 1000)?;
            println!("{code}");
        }
        Command::Host { db, action } => {
            let mut conn = open_db(&db)?;
            match action {
                HostAction::Add { username } => match announce::add_host(&mut conn, &username)? {
                    true => println!("{username} is now a host"),
                    false => println!("{username} was already a host"),
                },
                HostAction::Remove { username } => match announce::remove_host(&mut conn, &username)? {
                    true => println!("{username} is no longer a host"),
                    false => println!("{username} was not a host"),
                },
                HostAction::List => {
                    for h in announce::hosts(&conn)? {
                        println!("{h}");
                    }
                }
            }
        }
        Command::Announce { db, text } => {
            let text = if text == "-" { std::io::read_to_string(std::io::stdin()).context("reading the announcement")? } else { text };
            let mut conn = open_db(&db)?;
            announce::announce(&mut conn, By::Operator, None, &text)?;
            println!("announced");
        }
        Command::Maintenance { db, action } => {
            let mut conn = open_db(&db)?;
            match action {
                MaintenanceAction::Plan { at, minutes, reason, quiet } => {
                    let req = post_proto::PlanMaintenance { starts_at_ms: at.unwrap_or(0), minutes, reason, quiet };
                    announce::plan(&mut conn, By::Operator, &req)?;
                    print_maintenance(&conn)?;
                }
                MaintenanceAction::End => match announce::end(&mut conn, By::Operator)?.0 {
                    announce::Ended::Cancelled => println!("the planned maintenance is cancelled; everyone has been told"),
                    announce::Ended::Over => println!("maintenance is over; everyone has been told"),
                },
                MaintenanceAction::Status => print_maintenance(&conn)?,
            }
        }
    }
    Ok(())
}

fn print_maintenance(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    match announce::maintenance(conn)? {
        None => println!("no maintenance is planned"),
        Some(m) => {
            let state = match m.state() {
                post_proto::MaintenanceState::Underway => "underway since",
                _ => "planned for",
            };
            let reason = if m.reason.is_empty() { String::new() } else { format!(": {}", m.reason) };
            println!("maintenance {state} {}, for about {} minutes{reason}", format_time(m.starts_at_ms), m.minutes);
        }
    }
    Ok(())
}
