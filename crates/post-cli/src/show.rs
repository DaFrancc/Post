//! Results for people: each read into the core's own type before printing,
//! so a changed shape is a compile error, not a silently empty line. Also
//! the formats people type and read: times in UTC, colour fills.

use post_core::{self as core, AnnouncementKind as K, Fill, MessageState};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Unix milliseconds as `YYYY-MM-DD HH:MM` UTC.
pub fn time(ms: u64) -> String {
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

/// `YYYY-MM-DD HH:MM` in UTC as Unix milliseconds; the inverse of [`time`].
pub fn parse_utc(s: &str) -> Result<u64, String> {
    let bad = || format!("{s} is not a time like 2026-11-14 02:00 (UTC)");
    let (date, clock) = s.trim().split_once([' ', 'T']).ok_or_else(bad)?;
    let mut d = date.splitn(3, '-').map(str::parse::<i64>);
    let (Some(Ok(y)), Some(Ok(m)), Some(Ok(day))) = (d.next(), d.next(), d.next()) else { return Err(bad()) };
    let (h, min) = clock.split_once(':').ok_or_else(bad)?;
    let (Ok(h), Ok(min)) = (h.parse::<i64>(), min.parse::<i64>()) else { return Err(bad()) };
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || !(0..24).contains(&h) || !(0..60).contains(&min) {
        return Err(bad());
    }
    // Days since 1970-01-01 from a civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from((days * 86_400 + h * 3600 + min * 60) * 1000).map_err(|_| bad())
}

/// `#rrggbb` (or `rrggbb`) as `0xRRGGBB`.
fn parse_color(s: &str) -> anyhow::Result<u32> {
    let hex = s.trim().strip_prefix('#').unwrap_or(s.trim());
    match u32::from_str_radix(hex, 16) {
        Ok(c) if hex.len() == 6 => Ok(c),
        _ => anyhow::bail!("{s} is not a colour like #6e8bff"),
    }
}

/// A fill: `#rrggbb`, or `#rrggbb,#rrggbb` with an optional `@angle` in
/// degrees (180, top to bottom, if left out).
pub fn parse_fill(s: &str) -> anyhow::Result<Fill> {
    let bad = || anyhow::anyhow!("{s} is not a fill like #6e8bff or #6e8bff,#c2588e@135");
    let (colors, angle) = match s.split_once('@') {
        Some((c, a)) => (c, Some(a.trim().trim_end_matches("deg").parse::<u32>().map_err(|_| bad())?)),
        None => (s, None),
    };
    let colors = colors.split(',').map(parse_color).collect::<anyhow::Result<Vec<_>>>()?;
    let fill = match colors.len() {
        1 if angle.is_none() => Fill { colors, angle: 0 },
        2 => Fill { colors, angle: angle.unwrap_or(180) },
        _ => return Err(bad()),
    };
    if fill.valid() { Ok(fill) } else { Err(bad()) }
}

/// A `--banner-fill` or `--theme` argument: empty clears it.
pub fn parse_fill_arg(s: &str) -> anyhow::Result<Option<Fill>> {
    if s.is_empty() { Ok(None) } else { parse_fill(s).map(Some) }
}

/// A fill as people write it; [`parse_fill`] reads it back.
pub fn format_fill(f: &Fill) -> String {
    let colors: Vec<String> = f.colors.iter().map(|c| format!("#{c:06x}")).collect();
    if colors.len() == 2 { format!("{}@{}", colors.join(","), f.angle) } else { colors.join(",") }
}

/// An enum's name in the API (`do_not_disturb`), for printing.
fn word(v: impl Serialize) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// An announcement as a line of text, times in UTC.
pub fn announcement_text(a: &core::Announcement) -> String {
    let starts = time(a.starts_at_ms);
    let minutes = a.minutes;
    let reason = if a.text.is_empty() { String::new() } else { format!(" Reason: {}.", a.text) };
    let auto = " (Sent automatically by the server.)";
    match a.kind {
        K::Text => a.text.clone(),
        K::MaintenancePlanned => format!("Maintenance is planned for {starts} UTC, for about {minutes} minutes.{reason}{auto}"),
        K::MaintenanceReminder => {
            let left = a.starts_at_ms.saturating_sub(a.at_ms) / 60_000;
            let left = if left >= 120 { format!("{} hours", (left + 30) / 60) } else { format!("{left} minutes") };
            format!("Reminder: maintenance starts in about {left}, at {starts} UTC, for about {minutes} minutes.{reason}{auto}")
        }
        K::MaintenanceStarted => format!("Maintenance has started. The server should be back in about {minutes} minutes.{reason}{auto}"),
        K::MaintenanceOver => format!("Maintenance is over, and the server is back.{auto}"),
        K::MaintenanceCancelled => format!("The maintenance planned for {starts} UTC is cancelled.{auto}"),
        K::Unsupported => "[an announcement this version cannot show; update post]".into(),
    }
}

fn read<T: DeserializeOwned>(v: Value) -> anyhow::Result<T> {
    Ok(serde_json::from_value(v)?)
}

pub fn identity(i: &core::Identity) {
    println!("{}{} on {}", i.username, if i.bot { " (bot)" } else { "" }, i.server);
    println!("account {}", i.account_id);
    println!("device  {}", i.device_id);
}

fn messages(list: &[core::Message]) {
    for m in list {
        let body = if m.deleted {
            "[message deleted]".to_string()
        } else if m.unsupported {
            "[unsupported message; update post]".to_string()
        } else {
            m.body.clone().unwrap_or_default()
        };
        let mut tags = Vec::new();
        if m.edited {
            tags.push(format!("edited, {} versions", m.versions));
        }
        if m.state != MessageState::Received {
            tags.push(word(m.state));
        }
        let tags = if tags.is_empty() { String::new() } else { format!("  ({})", tags.join("; ")) };
        println!("{}  {}  {}: {}{}", m.id, time(m.sent_at_ms), m.sender, body, tags);
    }
}

fn person(p: &core::Person) {
    println!("{} (@{})", p.shown_name, p.username);
    for (value, label) in
        [(&p.display_name, "display name"), (&p.nickname, "your nickname"), (&p.typing_template, "typing"), (&p.arrival, "messages arrive")]
    {
        if !value.is_empty() {
            println!("{label}: {value}");
        }
    }
    if let Some(n) = &p.note {
        println!("note: {n}");
    }
    for (value, label) in [(&p.avatar, "picture"), (&p.banner, "banner")] {
        if let Some(id) = value {
            println!("{label}: {id}");
        }
    }
    for (value, label) in [(&p.banner_fill, "banner colour"), (&p.theme, "profile colours")] {
        if let Some(f) = value {
            println!("{label}: {}", format_fill(f));
        }
    }
    if let Some(since) = p.since_ms {
        println!("on Post since: {} UTC", time(since));
    }
    if !p.bio.is_empty() {
        println!("about me:");
        for line in p.bio.lines() {
            println!("  {line}");
        }
    }
    println!("safety number: {}", if p.verified { "verified" } else { "not verified" });
}

/// A method's result for people.
pub fn result(method: &str, v: Value) -> anyhow::Result<()> {
    match method {
        "identity" | "register" | "finish_link" => match read::<Option<core::Identity>>(v)? {
            Some(i) => identity(&i),
            None => println!("no account on this device"),
        },
        "start_dm" | "create_group" => {
            let c: core::Conversation = read(v)?;
            println!("{}  {}", c.id, c.title);
        }
        "members" => {
            for m in read::<Vec<core::Member>>(v)? {
                let mut tags = Vec::new();
                if m.you {
                    tags.push("you".to_string());
                }
                if m.admin {
                    tags.push("admin".to_string());
                }
                tags.push(format!("{} device{}", m.devices, if m.devices == 1 { "" } else { "s" }));
                println!("{} (@{})  {}", m.shown_name, m.username, tags.join(", "));
            }
        }
        "calls" => {
            for c in read::<Vec<core::Call>>(v)? {
                let people: Vec<&str> = c.participants.iter().map(|p| p.username.as_str()).collect();
                let ringing = if c.ringing { "  (calling you)" } else { "" };
                println!("{}  in {}  started by {}: {}{ringing}", c.id, c.conversation, c.started_by, people.join(", "));
            }
        }
        "start_call" | "join_call" => {
            let j: core::CallJoin = read(v)?;
            println!("in call {} as {}", j.call.id, j.participant_id);
        }
        "conversations" => {
            for c in read::<Vec<core::Conversation>>(v)? {
                let gone = if c.following { "" } else { "  (left)" };
                let unread = match (c.unread, c.mentions) {
                    (n, m) if m > 0 => format!("  ({n} unread, {m} mentioning you)"),
                    (n, _) if n > 0 => format!("  ({n} unread)"),
                    _ => String::new(),
                };
                println!("{}  {:5}  {}{unread}{gone}", c.id, word(c.kind), c.title);
            }
        }
        "messages" => messages(&read::<Vec<core::Message>>(v)?),
        "search" => {
            let found: core::SearchResults = read(v)?;
            println!("{} found", found.total);
            messages(&found.messages);
        }
        "send_text" | "edit_message" => {
            let m: core::Message = read(v)?;
            println!("{}  {}", m.id, word(m.state));
        }
        "message_versions" => {
            for x in read::<Vec<core::MessageVersion>>(v)? {
                let pending = if x.pending { "  (pending)" } else { "" };
                println!("v{}  {}  {}{pending}", x.version, time(x.at_ms), x.body);
            }
        }
        "person" => person(&read(v)?),
        "safety_number" => println!("{}", read::<String>(v)?),
        "create_invite" => {
            let i: core::Invite = read(v)?;
            println!("{}", i.code);
            eprintln!("single use{}, until {} UTC; sign up at {}", if i.bot { ", for a bot" } else { "" }, time(i.expires_at_ms), i.server);
        }
        "read_receipts" => println!("read receipts are {}", if read::<bool>(v)? { "on" } else { "off" }),
        "message_receipts" => {
            let r: core::MessageReceipts = read(v)?;
            if r.too_large {
                println!("this conversation is too large for receipts");
            }
            for p in &r.people {
                println!("{} (@{})  {}", p.shown_name, p.username, word(p.state));
            }
            if r.read_receipts_off {
                println!("your read receipts are off, so you see deliveries only");
            }
        }
        "announcements" => {
            for a in read::<Vec<core::Announcement>>(v)? {
                println!("{}  {}", time(a.at_ms), announcement_text(&a));
            }
        }
        "announcement_summary" => {
            let s: core::Announcements = read(v)?;
            println!("{} unread announcements from {}", s.unread, s.server);
            if s.host {
                println!("you are a host: you can announce and plan maintenance");
            }
            match s.maintenance {
                None => println!("no maintenance planned"),
                Some(m) => {
                    let state = if m.underway { "underway since" } else { "planned for" };
                    println!("maintenance {state} {} UTC, for about {} minutes", time(m.starts_at_ms), m.minutes);
                }
            }
        }
        "devices" => {
            for d in read::<Vec<core::Device>>(v)? {
                let name = if d.name.is_empty() { "(unnamed)" } else { d.name.as_str() };
                let this = if d.this_device { "  (this device)" } else { "" };
                println!("{}  {name}{this}", d.id);
            }
        }
        _ => println!("ok"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formatting() {
        assert_eq!(time(0), "1970-01-01 00:00");
        // Checked with `date -u -d @1794268800`.
        assert_eq!(time(1_794_268_800_000), "2026-11-10 00:00");
        assert_eq!(time(951_782_400_000 + 3_723_000), "2000-02-29 01:02");
    }

    #[test]
    fn utc_times_parse_back() {
        for ms in [0, 1_794_268_800_000, 951_782_400_000 + 3_720_000, 4_102_444_740_000] {
            assert_eq!(parse_utc(&time(ms)), Ok(ms));
        }
        assert_eq!(parse_utc("2026-11-14T02:00"), parse_utc("2026-11-14 02:00"));
        for bad in ["2026-13-01 00:00", "2026-11-14", "02:00", "2026-11-14 24:00", "tomorrow"] {
            assert!(parse_utc(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn fills_read_and_write_the_same() {
        let solid = parse_fill("#6E8BFF").expect("solid");
        assert_eq!(solid, Fill { colors: vec![0x6e8bff], angle: 0 });
        assert_eq!(format_fill(&solid), "#6e8bff");
        let gradient = parse_fill("#6e8bff, #c2588e@135").expect("gradient");
        assert_eq!(gradient, Fill { colors: vec![0x6e8bff, 0xc2588e], angle: 135 });
        assert_eq!(parse_fill(&format_fill(&gradient)).expect("again"), gradient);
        assert_eq!(parse_fill("#6e8bff,#c2588e").expect("default angle").angle, 180);
        for bad in ["6e8bf", "#6e8bff@90", "#a,#b", "#6e8bff,#c2588e@360", "#111111,#222222,#333333", "#6e8bff,#c2588e@x"] {
            assert!(parse_fill(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_fill_arg("").expect("clears"), None);
    }

    #[test]
    fn enums_print_as_the_api_names_them() {
        assert_eq!(word(MessageState::Delivered), "delivered");
        assert_eq!(word(core::ConversationKind::Dm), "dm");
    }
}
