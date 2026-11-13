//! Each subcommand as calls to the local API: the method's name and its
//! generated params struct (`post_api::*Params`), so a changed parameter is
//! a compile error here rather than a call the API refuses.

use anyhow::Context as _;
use post_api as api;
use post_core::{self as core, CallId, ConversationId, MessageId};
use serde::Serialize;
use serde_json::Value;

use crate::show::parse_fill_arg;
use crate::{AnnouncementsCommand, CallCommand, Command, DevicesCommand, GroupCommand, MaintenanceCommand, PictureCommand};

/// One call to the local API.
#[derive(Debug)]
pub struct Call {
    /// The method: the core's name for it.
    pub method: &'static str,
    /// Its parameters, from the generated params struct.
    pub params: Value,
}

/// A call with parameters.
pub fn call(method: &'static str, params: impl Serialize) -> Call {
    Call { method, params: serde_json::to_value(params).expect("params structs are JSON") }
}

/// A call to a method that takes no parameters.
fn bare(method: &'static str) -> Call {
    Call { method, params: Value::Null }
}

// IDs are checked here, before anything runs; the core's errors say which
// kind of ID was wrong ("not a conversation id: …").

pub fn conversation(s: &str) -> anyhow::Result<ConversationId> {
    Ok(s.parse()?)
}

fn message(s: &str) -> anyhow::Result<MessageId> {
    Ok(s.parse()?)
}

fn call_id(s: &str) -> anyhow::Result<CallId> {
    Ok(s.parse()?)
}

/// A file as an absolute path: a daemon may run in another folder.
fn absolute(f: &std::path::Path) -> String {
    std::path::absolute(f).unwrap_or_else(|_| f.to_path_buf()).to_string_lossy().into_owned()
}

/// The calls for a subcommand, in order; `None` for the subcommands that
/// are not API calls, or take more than calls in a row (linking, sending to
/// a username), which `main` runs itself.
pub fn to_calls(cmd: &Command) -> anyhow::Result<Option<Vec<Call>>> {
    let one = |c: Call| Ok(Some(vec![c]));
    match cmd {
        Command::Register { server, invite, username, bot } => one(call(
            "register",
            api::RegisterParams { server: server.clone(), invite: invite.clone(), username: username.clone(), bot: *bot },
        )),
        Command::Devices { cmd: None } => one(bare("devices")),
        Command::Devices { cmd: Some(DevicesCommand::Rename { device, name }) } => {
            one(call("rename_device", api::RenameDeviceParams { device: device.clone(), name: name.clone() }))
        }
        Command::Devices { cmd: Some(DevicesCommand::Remove { device, .. }) } => {
            one(call("remove_device", api::RemoveDeviceParams { device: device.clone() }))
        }
        Command::RefreshKeys => one(bare("refresh_keys")),
        Command::Whoami => one(bare("identity")),
        Command::Sync => one(bare("sync")),
        Command::Dm { username } => one(call("start_dm", api::StartDmParams { username: username.clone() })),
        Command::Group { cmd } => group(cmd).map(|c| Some(vec![c])),
        Command::Call { cmd } => calls(cmd).map(|c| Some(vec![c])),
        Command::Conversations => one(bare("conversations")),
        Command::Messages { conversation: c, limit } => {
            one(call("messages", api::MessagesParams { conversation: conversation(c)?, limit: *limit }))
        }
        Command::Search { text, conversation: c, from, mentions, has_link, oldest, limit, offset } => one(call(
            "search",
            api::SearchParams {
                query: core::SearchQuery {
                    text: text.clone().unwrap_or_default(),
                    conversation: c.as_deref().map(conversation).transpose()?,
                    from: from.clone(),
                    has_link: *has_link,
                    mentions: mentions.clone(),
                    after_ms: None,
                    before_ms: None,
                    oldest_first: *oldest,
                    limit: *limit,
                    offset: *offset,
                },
            },
        )),
        Command::Send { to: Some(_), .. } => Ok(None),
        Command::Send { to: None, conversation: c, text } => {
            let c = c.as_deref().context("give --to or --conversation")?;
            one(call("send_text", api::SendTextParams { conversation: conversation(c)?, text: text.clone() }))
        }
        Command::Edit { conversation: c, message: m, text } => {
            one(call("edit_message", api::EditMessageParams { conversation: conversation(c)?, id: message(m)?, text: text.clone() }))
        }
        Command::Delete { conversation: c, message: m } => {
            one(call("delete_message", api::DeleteMessageParams { conversation: conversation(c)?, id: message(m)? }))
        }
        Command::Versions { conversation: c, message: m } => {
            one(call("message_versions", api::MessageVersionsParams { conversation: conversation(c)?, id: message(m)? }))
        }
        Command::Receipts { conversation: c, message: m } => {
            one(call("message_receipts", api::MessageReceiptsParams { conversation: conversation(c)?, id: message(m)? }))
        }
        Command::Profile { display_name, typing_template, note, keep_note, arrival, bio, banner_fill, theme } => {
            let update = core::ProfileUpdate {
                display_name: display_name.clone(),
                typing_template: typing_template.clone(),
                note: note.clone(),
                note_keep: *keep_note,
                arrival: arrival.clone(),
                bio: bio.clone(),
            };
            let mut calls = Vec::new();
            // The profile's own fields, unless only the colours change.
            if update != core::ProfileUpdate::default() || (banner_fill.is_none() && theme.is_none()) {
                calls.push(call("set_profile", api::SetProfileParams { update }));
            }
            if let Some(f) = banner_fill {
                calls.push(call("set_banner_fill", api::SetBannerFillParams { fill: parse_fill_arg(f)? }));
            }
            if let Some(t) = theme {
                calls.push(call("set_theme", api::SetThemeParams { theme: parse_fill_arg(t)? }));
            }
            Ok(Some(calls))
        }
        Command::Picture { cmd } => one(picture(core::picture::PictureKind::Avatar, cmd)),
        Command::Banner { cmd } => one(picture(core::picture::PictureKind::Banner, cmd)),
        Command::Name { conversation: c, name } => {
            one(call("set_conversation_name", api::SetConversationNameParams { conversation: conversation(c)?, name: name.clone() }))
        }
        Command::Nickname { username, nickname } => {
            one(call("set_nickname", api::SetNicknameParams { username: username.clone(), nickname: nickname.clone() }))
        }
        Command::Person { username } => one(call("person", api::PersonParams { username: username.clone() })),
        Command::SafetyNumber { username } => one(call("safety_number", api::SafetyNumberParams { username: username.clone() })),
        Command::Verify { username, undo } => {
            one(call("set_verified", api::SetVerifiedParams { username: username.clone(), verified: !undo }))
        }
        Command::Read { conversation: c } => one(call("mark_read", api::MarkReadParams { conversation: conversation(c)? })),
        Command::ReadReceipts { set: None } => one(bare("read_receipts")),
        Command::ReadReceipts { set: Some(s) } => one(call("set_read_receipts", api::SetReadReceiptsParams { on: s == "on" })),
        Command::Announcements { cmd: None, limit } => one(call("announcements", api::AnnouncementsParams { limit: *limit })),
        Command::Announcements { cmd: Some(AnnouncementsCommand::Read), .. } => one(bare("mark_announcements_read")),
        Command::Announce { text } => one(call("announce", api::AnnounceParams { text: text.clone() })),
        Command::Invite { bot, days } => one(call("create_invite", api::CreateInviteParams { bot: *bot, days: *days })),
        Command::Maintenance { cmd: MaintenanceCommand::Status } => one(bare("announcement_summary")),
        Command::Maintenance { cmd: MaintenanceCommand::Plan { at, minutes, reason, quiet } } => one(call(
            "plan_maintenance",
            api::PlanMaintenanceParams { starts_at_ms: *at, minutes: *minutes, reason: reason.clone(), quiet: *quiet },
        )),
        Command::Maintenance { cmd: MaintenanceCommand::End } => one(bare("end_maintenance")),
        Command::Presence { presence } => {
            let setting = match presence.as_str() {
                "online" => core::PresenceSetting::Online,
                "away" => core::PresenceSetting::Away,
                "dnd" => core::PresenceSetting::DoNotDisturb,
                "invisible" => core::PresenceSetting::Invisible,
                other => anyhow::bail!("{other} is not online, away, dnd or invisible"),
            };
            one(call("set_presence", api::SetPresenceParams { setting }))
        }
        Command::Typing { conversation: c } => one(call("send_typing", api::SendTypingParams { conversation: conversation(c)? })),
        Command::Keygen { .. } | Command::Daemon { .. } | Command::Schema | Command::Link { .. } => Ok(None),
    }
}

fn group(cmd: &GroupCommand) -> anyhow::Result<Call> {
    Ok(match cmd {
        GroupCommand::Create { name, members } => {
            call("create_group", api::CreateGroupParams { name: name.clone(), usernames: members.clone() })
        }
        GroupCommand::Members { conversation: c } => call("members", api::MembersParams { conversation: conversation(c)? }),
        GroupCommand::Add { conversation: c, members } => {
            call("add_members", api::AddMembersParams { conversation: conversation(c)?, usernames: members.clone() })
        }
        GroupCommand::Remove { conversation: c, members } => {
            call("remove_members", api::RemoveMembersParams { conversation: conversation(c)?, usernames: members.clone() })
        }
        GroupCommand::Leave { conversation: c, .. } => call("leave_group", api::LeaveGroupParams { conversation: conversation(c)? }),
        GroupCommand::Rename { conversation: c, name } => {
            call("rename_group", api::RenameGroupParams { conversation: conversation(c)?, name: name.clone() })
        }
        GroupCommand::Admin { conversation: c, username, revoke } => {
            call("set_admin", api::SetAdminParams { conversation: conversation(c)?, username: username.clone(), admin: !revoke })
        }
        GroupCommand::History { conversation: c, shared, days, .. } => {
            call("set_history", api::SetHistoryParams { conversation: conversation(c)?, shared: *shared, days: *days })
        }
    })
}

fn calls(cmd: &CallCommand) -> anyhow::Result<Call> {
    Ok(match cmd {
        CallCommand::List => bare("calls"),
        CallCommand::Start { conversation: c, video, ring } => {
            call("start_call", api::StartCallParams { conversation: conversation(c)?, video: *video, ring: ring.clone() })
        }
        CallCommand::Ring { conversation: c, call: id, usernames } => {
            call("ring_call", api::RingCallParams { conversation: conversation(c)?, call: call_id(id)?, usernames: usernames.clone() })
        }
        CallCommand::Join { conversation: c, call: id } => {
            call("join_call", api::JoinCallParams { conversation: conversation(c)?, call: call_id(id)? })
        }
        CallCommand::Leave { conversation: c, call: id } => {
            call("leave_call", api::LeaveCallParams { conversation: conversation(c)?, call: call_id(id)? })
        }
        CallCommand::Decline { conversation: c, call: id } => {
            call("decline_call", api::DeclineCallParams { conversation: conversation(c)?, call: call_id(id)? })
        }
    })
}

fn picture(kind: core::picture::PictureKind, cmd: &PictureCommand) -> Call {
    match cmd {
        PictureCommand::Set { file } => call("set_picture_file", api::SetPictureFileParams { kind, path: Some(absolute(file)) }),
        PictureCommand::Remove => call("set_picture_file", api::SetPictureFileParams { kind, path: None }),
        PictureCommand::Save { id, file } => call("save_picture_file", api::SavePictureFileParams { id: id.clone(), path: absolute(file) }),
    }
}
