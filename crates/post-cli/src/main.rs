//! `post`: the Post command-line client.
//!
//! Every subcommand calls the local API (`post-api`): the same methods, with
//! the core's own types, that `post daemon` serves as JSON-RPC. Output is
//! human-readable by default; `--json` prints the method's result as JSON,
//! and failures exit non-zero with a JSON error.

mod calls;
mod show;

use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use post_cli::{daemon, keyfile};
use post_core::{self as core, Core};
use serde_json::{Value, json};

use calls::Call;
use show::parse_utc;

#[derive(Parser)]
#[command(name = "post", version, about = "Post: private, post-quantum messaging")]
struct Cli {
    /// Profile directory. Default: $POST_PROFILE, else the user data directory.
    #[arg(long, global = true, env = "POST_PROFILE")]
    profile: Option<PathBuf>,
    /// File holding the database key (create one with `post keygen`).
    #[arg(long, global = true, env = "POST_DB_KEY_FILE")]
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
    /// Link a new device to your account.
    Link {
        #[command(subcommand)]
        cmd: LinkCommand,
    },
    /// List your devices, or rename or remove one.
    Devices {
        #[command(subcommand)]
        cmd: Option<DevicesCommand>,
    },
    /// Give this device fresh keys in every conversation now (otherwise weekly).
    RefreshKeys,
    /// Show the account on this device.
    Whoami,
    /// Fetch everything new and send what is queued.
    Sync,
    /// Open a DM with a user.
    Dm { username: String },
    /// Create and manage group chats.
    Group {
        #[command(subcommand)]
        cmd: GroupCommand,
    },
    /// Calls: start, join, leave or decline one, or list them. The CLI
    /// carries a call's signalling, not its audio.
    Call {
        #[command(subcommand)]
        cmd: CallCommand,
    },
    /// List your conversations.
    Conversations,
    /// Show the newest messages of a conversation.
    Messages {
        conversation: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Find messages containing every word given, newest first.
    Search {
        /// Words to look for (all of them, any case).
        text: Option<String>,
        /// Only in this conversation.
        #[arg(long)]
        conversation: Option<String>,
        /// Only sent by this username.
        #[arg(long)]
        from: Option<String>,
        /// Only messages that mention this username.
        #[arg(long)]
        mentions: Option<String>,
        /// Only messages with a link.
        #[arg(long)]
        has_link: bool,
        /// Oldest first.
        #[arg(long)]
        oldest: bool,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// Skip this many first.
        #[arg(long, default_value_t = 0)]
        offset: u32,
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
    /// Who has one of your messages, and who has read it.
    Receipts { conversation: String, message: String },
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
        /// How your messages arrive in the app: bubble, rise, decode,
        /// typewriter, focus, wave or instant ("" restores the default).
        #[arg(long)]
        arrival: Option<String>,
        /// About me: up to 500 characters, line breaks included, formatted
        /// like a message ("" clears it).
        #[arg(long)]
        bio: Option<String>,
        /// The banner when you have no banner image: a colour like #6e8bff,
        /// or a gradient at an angle like "#6e8bff,#c2588e@135" ("" restores
        /// the app's choice).
        #[arg(long)]
        banner_fill: Option<String>,
        /// Profile colours: your card in a colour or a gradient, written
        /// like --banner-fill ("" restores the app's own look).
        #[arg(long)]
        theme: Option<String>,
    },
    /// Your profile picture: set it from an image file (its middle is
    /// used, square), remove it, or save someone's to a file.
    Picture {
        #[command(subcommand)]
        cmd: PictureCommand,
    },
    /// The banner across the top of your profile (3 : 1): set, remove or save.
    Banner {
        #[command(subcommand)]
        cmd: PictureCommand,
    },
    /// Set your display name in one conversation ("" clears it).
    Name { conversation: String, name: String },
    /// Set your private nickname for someone ("" clears it).
    Nickname { username: String, nickname: String },
    /// Show what you know about someone.
    Person { username: String },
    /// Choose the presence others see: online, away, dnd or invisible.
    Presence {
        #[arg(value_parser = ["online", "away", "dnd", "invisible"])]
        presence: String,
    },
    /// Tell a conversation you are typing.
    Typing { conversation: String },
    /// Show the safety number between you and someone.
    SafetyNumber { username: String },
    /// Record that you compared safety numbers with someone.
    Verify {
        username: String,
        /// Withdraw it instead.
        #[arg(long)]
        undo: bool,
    },
    /// Mark a conversation read on all your devices.
    Read { conversation: String },
    /// Whether you send read receipts; `on` or `off` to change it, on all
    /// your devices. Off, you also do not see when others read yours.
    ReadReceipts {
        #[arg(value_parser = ["on", "off"])]
        set: Option<String>,
    },
    /// Show the server's announcements, or mark them read.
    Announcements {
        #[command(subcommand)]
        cmd: Option<AnnouncementsCommand>,
        /// How many of the newest to show.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// As one of the server's hosts: post an announcement to everyone.
    Announce { text: String },
    /// As one of the server's hosts: make a single-use invite code.
    Invite {
        /// For a bot account.
        #[arg(long)]
        bot: bool,
        /// Days until it expires (1 to 30).
        #[arg(long, default_value_t = 7)]
        days: u32,
    },
    /// Planned maintenance: see it, or as a host plan or end it.
    Maintenance {
        #[command(subcommand)]
        cmd: MaintenanceCommand,
    },
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

#[derive(Subcommand)]
enum PictureCommand {
    /// Use an image file (PNG, JPEG, WebP or GIF); it is shrunk to fit.
    Set { file: std::path::PathBuf },
    /// Remove it.
    Remove,
    /// Write a picture this device has to a file, by the ID `post person`
    /// shows.
    Save { id: String, file: std::path::PathBuf },
}

#[derive(Subcommand)]
enum AnnouncementsCommand {
    /// Mark them read, on all your devices.
    Read,
}

#[derive(Subcommand)]
enum MaintenanceCommand {
    /// Show unread announcements, whether you are a host, and any
    /// maintenance planned.
    Status,
    /// Plan maintenance. Everyone is told now (unless --quiet), reminded a
    /// day, an hour, 30 minutes and 5 minutes before, and told when it
    /// starts and ends.
    Plan {
        /// When it starts, as YYYY-MM-DD HH:MM in UTC. Leave it out to
        /// start now.
        #[arg(long, value_parser = parse_utc)]
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
}

#[derive(Subcommand)]
enum CallCommand {
    /// List the live calls.
    List,
    /// Start a call in a conversation.
    Start {
        conversation: String,
        /// As a video call.
        #[arg(long)]
        video: bool,
        /// Ring only this person (repeat for more). Default: everyone in a
        /// DM or a group of up to eight.
        #[arg(long)]
        ring: Vec<String>,
    },
    /// Ring people into a call under way.
    Ring {
        conversation: String,
        call: String,
        #[arg(required = true)]
        usernames: Vec<String>,
    },
    /// Join (or answer) a call.
    Join { conversation: String, call: String },
    /// Leave a call; in a DM this ends it.
    Leave { conversation: String, call: String },
    /// Decline a ringing call.
    Decline { conversation: String, call: String },
}

#[derive(Subcommand)]
enum GroupCommand {
    /// Create a group with you as admin.
    Create {
        name: String,
        /// Usernames of the other members.
        #[arg(required = true)]
        members: Vec<String>,
    },
    /// List the members of a conversation.
    Members { conversation: String },
    /// As an admin: add people to a group.
    Add {
        conversation: String,
        #[arg(required = true)]
        members: Vec<String>,
    },
    /// As an admin: remove people from a group.
    Remove {
        conversation: String,
        #[arg(required = true)]
        members: Vec<String>,
    },
    /// As an admin: rename a group.
    Rename { conversation: String, name: String },
    /// Leave a group, with all your devices.
    Leave {
        conversation: String,
        /// Leave without asking.
        #[arg(long)]
        yes: bool,
    },
    /// As an admin: make a member an admin (or, with --revoke, stop them being one).
    Admin {
        conversation: String,
        username: String,
        #[arg(long)]
        revoke: bool,
    },
    /// As an admin: whether new members are shown past messages.
    History {
        conversation: String,
        /// Share past messages with new members.
        #[arg(long, conflicts_with = "hidden", required_unless_present = "hidden")]
        shared: bool,
        /// New members see only what is sent after they join.
        #[arg(long)]
        hidden: bool,
        /// With --shared: how many days back (0, the default, means all).
        #[arg(long, default_value_t = 0, requires = "shared")]
        days: u32,
    },
}

#[derive(Subcommand)]
enum LinkCommand {
    /// On the new device: show a code, and wait for it to be entered on one
    /// of your existing devices (up to ten minutes).
    New {
        /// Server URL, wss://...
        #[arg(long)]
        server: String,
        /// A name for this device, shown only on your own devices. Default:
        /// the computer's host name.
        #[arg(long)]
        name: Option<String>,
        /// The account is a bot account.
        #[arg(long)]
        bot: bool,
    },
    /// On one of your devices: add the device showing this code.
    Add {
        code: String,
        /// Approve without asking.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum DevicesCommand {
    /// Name one of your devices.
    Rename {
        /// Device ID, or its first few characters.
        device: String,
        name: String,
    },
    /// Remove another of your devices from your account and every conversation.
    Remove {
        /// Device ID, or its first few characters.
        device: String,
        /// Remove without asking.
        #[arg(long)]
        yes: bool,
    },
}

/// The computer's host name, for naming a newly linked device.
fn default_device_name() -> String {
    let from_file = std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string());
    from_file
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "command line".into())
}

/// Ask a yes/no question on the terminal. Without a terminal, refuse unless
/// the caller passed `--yes`, so a script never approves something by accident.
fn confirm(question: &str) -> anyhow::Result<bool> {
    use std::io::{BufRead as _, IsTerminal as _, Write as _};
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("{question}\nno terminal to ask on; pass --yes to go ahead");
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

fn default_profile() -> anyhow::Result<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(d).join("post"));
    }
    if let Some(d) = std::env::var_os("APPDATA") {
        return Ok(PathBuf::from(d).join("post"));
    }
    let home = std::env::var_os("HOME").context("cannot find a home directory; pass --profile")?;
    Ok(PathBuf::from(home).join(".local/share/post"))
}

/// Run one call: its result, or the API's error.
async fn api(core: &Core, c: &Call) -> Result<Value, post_api::ApiError> {
    post_api::call(core, c.method, c.params.clone()).await
}

/// Run one call where a failure ends the command.
async fn api_typed<T: serde::de::DeserializeOwned>(core: &Core, c: &Call) -> anyhow::Result<T> {
    let v = api(core, c).await.map_err(|e| anyhow::anyhow!(e.message))?;
    Ok(serde_json::from_value(v)?)
}

/// Run `calls` in order, stopping at the first failure, and print the last
/// result (or the failure).
async fn run_calls(core: &Core, calls: &[Call], json_out: bool) -> anyhow::Result<()> {
    let mut last = None;
    for c in calls {
        match api(core, c).await {
            Ok(v) => last = Some((c.method, v)),
            Err(e) if json_out => {
                println!("{}", json!({ "error": { "code": e.code, "message": e.message } }));
                std::process::exit(1);
            }
            Err(e) => anyhow::bail!(e.message),
        }
    }
    match last {
        Some((_, v)) if json_out => println!("{}", serde_json::to_string_pretty(&v)?),
        Some((method, v)) => show::result(method, v)?,
        None => {}
    }
    Ok(())
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match &cli.command {
        Command::Keygen { path } => {
            keyfile::generate(path)?;
            println!("wrote {}", path.display());
            return Ok(());
        }
        Command::Schema => {
            println!("{}", serde_json::to_string_pretty(&post_api::openrpc())?);
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

    if let Command::Link { cmd } = &cli.command {
        return run_link(&core, cmd, cli.json).await;
    }
    if let Command::Devices { cmd: Some(DevicesCommand::Remove { device, yes: false }) } = &cli.command {
        let name = core
            .devices()
            .await?
            .into_iter()
            .find(|d| d.id.starts_with(&device.to_ascii_lowercase()))
            .map(|d| if d.name.is_empty() { d.id } else { format!("\"{}\" ({})", d.name, d.id) })
            .unwrap_or_else(|| device.clone());
        if !confirm(&format!("Remove device {name}? It is signed out and removed from every conversation."))? {
            anyhow::bail!("not removed");
        }
    }

    if let Command::Group { cmd: GroupCommand::Leave { conversation, yes: false } } = &cli.command {
        let title = conversation
            .parse()
            .ok()
            .and_then(|id| core.conversation(id).ok())
            .map_or_else(|| conversation.clone(), |c| format!("\"{}\"", c.title));
        if !confirm(&format!("Leave {title}? You will need an admin to add you back."))? {
            anyhow::bail!("still in the group");
        }
    }

    // To a username: open the DM (creating it if needed), then send.
    if let Command::Send { to: Some(username), text, .. } = &cli.command {
        let dm: core::Conversation =
            api_typed(&core, &calls::call("start_dm", post_api::StartDmParams { username: username.clone() })).await?;
        let send = calls::call("send_text", post_api::SendTextParams { conversation: dm.id, text: text.clone() });
        return run_calls(&core, &[send], cli.json).await;
    }

    let calls = calls::to_calls(&cli.command)?.context("unknown command")?;
    run_calls(&core, &calls, cli.json).await
}

/// Print a result line now, even when stdout is a pipe, because the command
/// keeps running after it.
fn print_now(line: impl std::fmt::Display) -> anyhow::Result<()> {
    use std::io::Write as _;
    println!("{line}");
    std::io::stdout().flush()?;
    Ok(())
}

/// Linking takes two calls with a person in between, so it does not fit the
/// one-call-per-subcommand shape of the rest.
async fn run_link(core: &Core, cmd: &LinkCommand, json_out: bool) -> anyhow::Result<()> {
    match cmd {
        LinkCommand::New { server, name, bot } => {
            let device_name = name.clone().unwrap_or_else(default_device_name);
            let begin = calls::call("begin_link", post_api::BeginLinkParams { server: server.clone(), device_name, bot: *bot });
            let code: String = api_typed(core, &begin).await?;
            if json_out {
                print_now(json!(code))?;
            } else {
                print_now(format_args!(
                    "On one of your devices, run:\n\n    post link add {code}\n\nWaiting for that (the code works for ten minutes)..."
                ))?;
            }
            let id: core::Identity = api_typed(core, &Call { method: "finish_link", params: Value::Null }).await?;
            if json_out {
                print_now(json!(id))?;
            } else {
                println!("Linked. This device is now part of {}'s account.", id.username);
                show::identity(&id);
            }
        }
        LinkCommand::Add { code, yes } => {
            core.connect().await?;
            let inspect = calls::call("link_inspect", post_api::LinkInspectParams { code: code.clone() });
            let req: core::LinkRequest = api_typed(core, &inspect).await?;
            let name = if req.name.is_empty() { "unnamed".to_string() } else { req.name.clone() };
            let question = format!(
                "Add the device \"{name}\" ({}) to your account? It will see new messages in every conversation you are in.",
                req.device_id
            );
            if !yes && !confirm(&question)? {
                anyhow::bail!("not linked");
            }
            let approve = calls::call("link_approve", post_api::LinkApproveParams { code: code.clone() });
            let done: core::LinkRequest = api_typed(core, &approve).await?;
            if json_out {
                print_now(json!(done))?;
            } else {
                println!("Linked \"{name}\". It has been added to your conversations.");
            }
        }
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
        eprintln!("post: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subcommand_maps_to_known_methods() {
        let id = || "00112233445566778899aabbccddeeff".to_string();
        let cmds = [
            Command::Whoami,
            Command::Sync,
            Command::Dm { username: "a".into() },
            Command::Conversations,
            Command::Person { username: "a".into() },
            Command::SafetyNumber { username: "a".into() },
            Command::Nickname { username: "a".into(), nickname: "b".into() },
            Command::Devices { cmd: None },
            Command::RefreshKeys,
            Command::Presence { presence: "dnd".into() },
            Command::Typing { conversation: id() },
            Command::Send { to: None, conversation: Some(id()), text: "hi".into() },
            Command::Group { cmd: GroupCommand::Create { name: "g".into(), members: vec!["a".into()] } },
            Command::Group { cmd: GroupCommand::Members { conversation: id() } },
            Command::Group { cmd: GroupCommand::Add { conversation: id(), members: vec!["a".into()] } },
            Command::Group { cmd: GroupCommand::Remove { conversation: id(), members: vec!["a".into()] } },
            Command::Group { cmd: GroupCommand::Rename { conversation: id(), name: "n".into() } },
            Command::Group { cmd: GroupCommand::Leave { conversation: id(), yes: true } },
            Command::Group { cmd: GroupCommand::Admin { conversation: id(), username: "a".into(), revoke: false } },
            Command::Group { cmd: GroupCommand::History { conversation: id(), shared: true, hidden: false, days: 7 } },
            Command::Call { cmd: CallCommand::Join { conversation: id(), call: id() } },
            Command::Devices { cmd: Some(DevicesCommand::Rename { device: "abcd".into(), name: "x".into() }) },
            Command::Devices { cmd: Some(DevicesCommand::Remove { device: "abcd".into(), yes: true }) },
            Command::Announcements { cmd: None, limit: 5 },
            Command::Announcements { cmd: Some(AnnouncementsCommand::Read), limit: 5 },
            Command::Announce { text: "hi".into() },
            Command::ReadReceipts { set: None },
            Command::Invite { bot: false, days: 7 },
            Command::Receipts { conversation: id(), message: id() },
            Command::ReadReceipts { set: Some("off".into()) },
            Command::Maintenance { cmd: MaintenanceCommand::Status },
            Command::Maintenance { cmd: MaintenanceCommand::Plan { at: None, minutes: 5, reason: String::new(), quiet: false } },
            Command::Maintenance { cmd: MaintenanceCommand::End },
            Command::Picture { cmd: PictureCommand::Set { file: "a.png".into() } },
            Command::Picture { cmd: PictureCommand::Remove },
            Command::Banner { cmd: PictureCommand::Save { id: "00".into(), file: "b.png".into() } },
            Command::Profile {
                display_name: None,
                typing_template: None,
                note: None,
                keep_note: false,
                arrival: None,
                bio: None,
                banner_fill: Some("#6e8bff".into()),
                theme: Some(String::new()),
            },
        ];
        for c in &cmds {
            let calls = calls::to_calls(c).expect("valid").expect("API calls");
            assert!(!calls.is_empty());
            for call in calls {
                assert!(post_api::method(call.method).is_some(), "{}", call.method);
            }
        }
    }

    #[test]
    fn ids_are_checked_before_calling() {
        let bad = Command::Typing { conversation: "00".into() };
        assert!(calls::to_calls(&bad).is_err());
    }

    #[test]
    fn only_the_colours_change_without_the_rest_of_the_profile() {
        let theme_only = Command::Profile {
            display_name: None,
            typing_template: None,
            note: None,
            keep_note: false,
            arrival: None,
            bio: None,
            banner_fill: None,
            theme: Some("#6e8bff".into()),
        };
        let methods: Vec<&str> = calls::to_calls(&theme_only).expect("valid").expect("calls").iter().map(|c| c.method).collect();
        assert_eq!(methods, ["set_theme"]);
    }
}
