//! `postern`: the Postern command-line client.
//!
//! Every subcommand calls one method of the local API (`postern-api`); the
//! same methods are served by `postern daemon` as JSON-RPC. Output is
//! human-readable by default; `--json` prints the method's result as JSON,
//! and failures exit non-zero with a JSON error.

use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use postern_api::Needs;
use postern_cli::{daemon, keyfile};
use postern_core::Core;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "postern", version, about = "Postern: private, post-quantum messaging")]
struct Cli {
    /// Profile directory. Default: $POSTERN_PROFILE, else the user data directory.
    #[arg(long, global = true, env = "POSTERN_PROFILE")]
    profile: Option<PathBuf>,
    /// File holding the database key (create one with `postern keygen`).
    #[arg(long, global = true, env = "POSTERN_DB_KEY_FILE")]
    db_key_file: Option<PathBuf>,
    /// Print results as JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new random database key file, readable only by you.
    Keygen { path: PathBuf },
    /// Create an account with an invite.
    Register {
        /// Server URL, wss://...
        #[arg(long)]
        server: String,
        #[arg(long)]
        invite: String,
        #[arg(long)]
        username: String,
        /// A bot account (needs a bot invite).
        #[arg(long)]
        bot: bool,
    },
    /// Show the account on this device.
    Whoami,
    /// Fetch everything new and send what is queued.
    Sync,
    /// Open a DM with a user.
    Dm { username: String },
    /// List your conversations.
    Conversations,
    /// Show the newest messages of a conversation.
    Messages {
        conversation: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Send a message to a conversation, or to a user's DM.
    Send {
        /// Username to DM.
        #[arg(long, conflicts_with = "conversation", required_unless_present = "conversation")]
        to: Option<String>,
        /// Conversation ID.
        #[arg(long)]
        conversation: Option<String>,
        text: String,
    },
    /// Edit one of your messages. Earlier versions stay visible.
    Edit { conversation: String, message: String, text: String },
    /// Delete one of your messages, with every version, for everyone.
    Delete { conversation: String, message: String },
    /// Show every version of a message.
    Versions { conversation: String, message: String },
    /// Change your profile. Unspecified fields stay as they are.
    Profile {
        /// Global display name ("" clears it).
        #[arg(long)]
        display_name: Option<String>,
        /// Typing template containing $$NAME$$ ("" restores the standard text).
        #[arg(long)]
        typing_template: Option<String>,
        /// Note, up to 60 characters ("" clears it).
        #[arg(long)]
        note: Option<String>,
        /// Keep the note until changed, instead of 24 hours.
        #[arg(long)]
        keep_note: bool,
    },
    /// Set your display name in one conversation ("" clears it).
    Name { conversation: String, name: String },
    /// Set your private nickname for someone ("" clears it).
    Nickname { username: String, nickname: String },
    /// Show what you know about someone.
    Person { username: String },
    /// Show the safety number between you and someone.
    SafetyNumber { username: String },
    /// Serve the local API as JSON-RPC for bots and scripts.
    Daemon {
        /// Talk over stdin and stdout.
        #[arg(long, conflicts_with = "socket", required_unless_present = "socket")]
        stdio: bool,
        /// Listen on a Unix socket at this path (mode 0600).
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Print the OpenRPC description of the local API.
    Schema,
}

fn default_profile() -> anyhow::Result<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(d).join("postern"));
    }
    if let Some(d) = std::env::var_os("APPDATA") {
        return Ok(PathBuf::from(d).join("postern"));
    }
    let home = std::env::var_os("HOME").context("cannot find a home directory; pass --profile")?;
    Ok(PathBuf::from(home).join(".local/share/postern"))
}

/// The API method and parameters for a subcommand.
fn to_call(cmd: &Command) -> Option<(&'static str, Value)> {
    Some(match cmd {
        Command::Register { server, invite, username, bot } => {
            ("register", json!({ "server": server, "invite": invite, "username": username, "bot": bot }))
        }
        Command::Whoami => ("whoami", json!({})),
        Command::Sync => ("sync", json!({})),
        Command::Dm { username } => ("start_dm", json!({ "username": username })),
        Command::Conversations => ("conversations", json!({})),
        Command::Messages { conversation, limit } => ("messages", json!({ "conversation": conversation, "limit": limit })),
        Command::Send { to, conversation, text } => ("send", json!({ "to": to, "conversation": conversation, "text": text })),
        Command::Edit { conversation, message, text } => {
            ("edit", json!({ "conversation": conversation, "message": message, "text": text }))
        }
        Command::Delete { conversation, message } => ("delete", json!({ "conversation": conversation, "message": message })),
        Command::Versions { conversation, message } => ("versions", json!({ "conversation": conversation, "message": message })),
        Command::Profile { display_name, typing_template, note, keep_note } => (
            "set_profile",
            json!({ "display_name": display_name, "typing_template": typing_template, "note": note, "note_keep": keep_note }),
        ),
        Command::Name { conversation, name } => ("set_conversation_name", json!({ "conversation": conversation, "name": name })),
        Command::Nickname { username, nickname } => ("set_nickname", json!({ "username": username, "nickname": nickname })),
        Command::Person { username } => ("person", json!({ "username": username })),
        Command::SafetyNumber { username } => ("safety_number", json!({ "username": username })),
        Command::Keygen { .. } | Command::Daemon { .. } | Command::Schema => return None,
    })
}

/// Unix milliseconds as `YYYY-MM-DD HH:MM` UTC.
fn time(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn print_human(method: &str, v: &Value) {
    match method {
        "register" | "whoami" if v.is_null() => println!("no account on this device"),
        "register" | "whoami" => {
            println!("{}{} on {}", s(v, "username"), if v["bot"] == true { " (bot)" } else { "" }, s(v, "server"));
            println!("account {}", s(v, "account_id"));
            println!("device  {}", s(v, "device_id"));
        }
        "start_dm" => println!("{}  {}", s(v, "id"), s(v, "title")),
        "conversations" => {
            for c in v.as_array().into_iter().flatten() {
                let gone = if c["following"] == false { "  (left)" } else { "" };
                println!("{}  {:5}  {}{gone}", s(c, "id"), s(c, "kind"), s(c, "title"));
            }
        }
        "messages" => {
            for m in v.as_array().into_iter().flatten() {
                let body = if m["deleted"] == true {
                    "[message deleted]".to_string()
                } else if m["unsupported"] == true {
                    "[unsupported message; update postern]".to_string()
                } else {
                    s(m, "body").to_string()
                };
                let mut tags = Vec::new();
                if m["edited"] == true {
                    tags.push(format!("edited, {} versions", m["versions"]));
                }
                if matches!(s(m, "state"), "pending" | "failed") {
                    tags.push(s(m, "state").to_string());
                }
                let tags = if tags.is_empty() { String::new() } else { format!("  ({})", tags.join("; ")) };
                println!("{}  {}  {}: {}{}", s(m, "id"), time(m["sent_at_ms"].as_u64().unwrap_or(0)), s(m, "sender"), body, tags);
            }
        }
        "send" | "edit" => println!("{}  {}", s(v, "id"), s(v, "state")),
        "versions" => {
            for x in v.as_array().into_iter().flatten() {
                let pending = if x["pending"] == true { "  (pending)" } else { "" };
                println!("v{}  {}  {}{pending}", x["version"], time(x["at_ms"].as_u64().unwrap_or(0)), s(x, "body"));
            }
        }
        "person" => {
            println!("{} (@{})", s(v, "shown_name"), s(v, "username"));
            for (k, label) in [("display_name", "display name"), ("nickname", "your nickname"), ("typing_template", "typing")] {
                if !s(v, k).is_empty() {
                    println!("{label}: {}", s(v, k));
                }
            }
            if let Some(n) = v["note"].as_str() {
                println!("note: {n}");
            }
        }
        "safety_number" => println!("{}", s(v, "safety_number")),
        _ => println!("ok"),
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match &cli.command {
        Command::Keygen { path } => {
            keyfile::generate(path)?;
            println!("wrote {}", path.display());
            return Ok(());
        }
        Command::Schema => {
            println!("{}", serde_json::to_string_pretty(&postern_api::openrpc())?);
            return Ok(());
        }
        _ => {}
    }
    let profile = match &cli.profile {
        Some(p) => p.clone(),
        None => default_profile()?,
    };
    let key = keyfile::read(&keyfile::locate(cli.db_key_file.as_deref())?)?;
    let core = Core::open(&profile, &key).with_context(|| format!("opening profile {}", profile.display()))?;

    if let Command::Daemon { stdio, socket } = &cli.command {
        let supervisor = tokio::spawn(daemon::supervise(core.clone()));
        if *stdio {
            daemon::serve(core, tokio::io::stdin(), tokio::io::stdout()).await;
        } else if let Some(path) = socket {
            #[cfg(unix)]
            daemon::serve_socket(core, path).await?;
            #[cfg(not(unix))]
            anyhow::bail!("--socket needs Unix; named pipes for Windows are not built yet ({})", path.display());
        }
        supervisor.abort();
        return Ok(());
    }

    let (method, params) = to_call(&cli.command).context("unknown command")?;
    let info = postern_api::method(method).context("unknown method")?;
    if info.needs == Needs::Connection {
        core.connect().await?;
    }
    let result = match postern_api::call(&core, method, params.clone()).await {
        Err(e) if e.code == 1003 && info.needs == Needs::Local => {
            core.connect().await?;
            postern_api::call(&core, method, params).await
        }
        r => r,
    };
    match result {
        Ok(v) if cli.json => println!("{}", serde_json::to_string_pretty(&v)?),
        Ok(v) => print_human(method, &v),
        Err(e) if cli.json => {
            println!("{}", json!({ "error": { "code": e.code, "message": e.message } }));
            std::process::exit(1);
        }
        Err(e) => anyhow::bail!(e.message),
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    // Logs go to stderr: stdout carries results, or the protocol with --stdio.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("postern: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subcommand_maps_to_a_known_method() {
        let cmds = [
            Command::Whoami,
            Command::Sync,
            Command::Dm { username: "a".into() },
            Command::Conversations,
            Command::Person { username: "a".into() },
            Command::SafetyNumber { username: "a".into() },
            Command::Nickname { username: "a".into(), nickname: "b".into() },
        ];
        for c in &cmds {
            let (m, _) = to_call(c).expect("maps");
            assert!(postern_api::method(m).is_some(), "{m}");
        }
    }

    #[test]
    fn time_formatting() {
        assert_eq!(time(0), "1970-01-01 00:00");
        // Checked with `date -u -d @1794268800`.
        assert_eq!(time(1_794_268_800_000), "2026-11-10 00:00");
        assert_eq!(time(951_782_400_000 + 3_723_000), "2000-02-29 01:02");
    }
}
