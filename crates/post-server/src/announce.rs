//! Announcements and planned maintenance (design section 8,
//! "Announcements").
//!
//! The server keeps one list of announcements that every account receives.
//! Hosts (accounts the server's operator names with `postd host add`) post
//! to it from the app; the operator can also post with `postd announce`.
//! The server itself posts about maintenance: when it is planned, reminders
//! a day, an hour, 30 minutes and 5 minutes before, when it starts and when
//! it is over. Announcements are never deleted.
//!
//! Only logged-in sessions can read them, and the database is encrypted at
//! rest. They are not end-to-end encrypted: the server writes some of them
//! itself, and posts the hosts' for them.
//!
//! Every change bumps the `announcements_version` counter, which the running
//! server watches, so changes made by `postd` admin commands in another
//! process wake every session too.

use post_identity::AccountId;
use post_proto::{self as proto, AnnouncementKind, ErrorCode, MaintenanceState};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::db::{Internal as _, Result, err, now_ms, sql_i64, write_tx};

const MINUTE_MS: u64 = 60 * 1000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

/// How long before a planned maintenance the reminders go out, earliest
/// first.
pub const REMINDERS_MS: [u64; 4] = [DAY_MS, HOUR_MS, 30 * MINUTE_MS, 5 * MINUTE_MS];

/// A reminder due this soon after the plan itself was announced is skipped:
/// everyone has just been told.
const REMINDER_GAP_MS: u64 = 5 * MINUTE_MS;

/// Furthest ahead maintenance may be planned.
const MAX_PLAN_AHEAD_MS: u64 = 366 * DAY_MS;

/// Who is asking: a host's account from the app, or the operator at the
/// server's command line.
#[derive(Clone, Copy)]
pub enum By<'a> {
    /// A logged-in account, which must be a host.
    Host(&'a AccountId),
    /// The operator, through `postd` on the server.
    Operator,
}

pub(crate) fn check_host(conn: &Connection, by: By<'_>) -> Result<()> {
    match by {
        By::Operator => Ok(()),
        By::Host(a) if is_host(conn, a)? => Ok(()),
        By::Host(_) => Err(err(ErrorCode::Forbidden, "only the server's hosts can do this")),
    }
}

fn check_text(text: &str, what: &str) -> Result<()> {
    if text.len() > proto::MAX_ANNOUNCEMENT_BYTES {
        return Err(err(ErrorCode::BadRequest, format!("{what} is too long")));
    }
    Ok(())
}

/// The change counter. Bumped by every change to announcements, the hosts
/// or the maintenance plan.
pub fn version(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row("SELECT COALESCE((SELECT v FROM meta WHERE k = 'announcements_version'), 0)", [], |r| r.get::<_, i64>(0))
        .map(|v| v as u64)
}

fn bump(tx: &Connection) -> Result<u64> {
    tx.execute("INSERT INTO meta (k, v) VALUES ('announcements_version', 1) ON CONFLICT (k) DO UPDATE SET v = v + 1", []).db()?;
    version(tx).db()
}

fn post(tx: &Connection, kind: AnnouncementKind, text: &str, starts_at: u64, minutes: u32, client_msg_id: Option<&[u8]>) -> Result<()> {
    tx.execute(
        "INSERT INTO announcements (created_at, kind, text, starts_at, minutes, client_msg_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![sql_i64(now_ms()), kind as i32, text, sql_i64(starts_at), minutes, client_msg_id],
    )
    .db()?;
    Ok(())
}

// ---------------------------------------------------------------- hosts

/// Whether `account` is a host.
pub fn is_host(conn: &Connection, account: &AccountId) -> Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM hosts WHERE account_id = ?1)", [&account.0[..]], |r| r.get(0)).db()
}

/// Make `username` a host. Returns false if it already was.
pub fn add_host(conn: &mut Connection, username: &str) -> Result<bool> {
    let tx = write_tx(conn)?;
    let account: Vec<u8> = tx
        .query_row("SELECT account_id FROM accounts WHERE username = ?1", [username], |r| r.get(0))
        .optional()
        .db()?
        .ok_or_else(|| err(ErrorCode::NotFound, format!("there is no account named {username}")))?;
    let added = tx.execute("INSERT OR IGNORE INTO hosts (account_id) VALUES (?1)", [account]).db()? > 0;
    if added {
        bump(&tx)?;
    }
    tx.commit().db()?;
    Ok(added)
}

/// Stop `username` being a host. Returns false if it was not one.
pub fn remove_host(conn: &mut Connection, username: &str) -> Result<bool> {
    let tx = write_tx(conn)?;
    let removed =
        tx.execute("DELETE FROM hosts WHERE account_id = (SELECT account_id FROM accounts WHERE username = ?1)", [username]).db()? > 0;
    if removed {
        bump(&tx)?;
    }
    tx.commit().db()?;
    Ok(removed)
}

/// The hosts' usernames, sorted.
pub fn hosts(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT a.username FROM hosts h JOIN accounts a USING (account_id) ORDER BY a.username").db()?;
    stmt.query_map([], |r| r.get(0)).db()?.collect::<rusqlite::Result<_>>().db()
}

// ---------------------------------------------------------------- reading

/// Announcements after `after_id`, oldest first, with what `account` needs
/// to know besides: whether it is a host, and the maintenance plan.
pub fn fetch(conn: &Connection, account: &AccountId, after_id: u64, limit: u32) -> Result<proto::Announcements> {
    let limit = if limit == 0 { proto::ANNOUNCEMENT_FETCH_LIMIT } else { limit.min(proto::ANNOUNCEMENT_FETCH_LIMIT) };
    let mut stmt = conn
        .prepare_cached("SELECT id, created_at, kind, text, starts_at, minutes FROM announcements WHERE id > ?1 ORDER BY id LIMIT ?2")
        .db()?;
    let mut entries: Vec<proto::Announcement> = stmt
        .query_map(params![sql_i64(after_id), limit + 1], |r| {
            Ok(proto::Announcement {
                id: r.get::<_, i64>(0)? as u64,
                created_at_ms: r.get::<_, i64>(1)? as u64,
                kind: r.get(2)?,
                text: r.get(3)?,
                starts_at_ms: r.get::<_, i64>(4)? as u64,
                minutes: r.get(5)?,
            })
        })
        .db()?
        .collect::<rusqlite::Result<_>>()
        .db()?;
    let more = entries.len() > limit as usize;
    entries.truncate(limit as usize);
    Ok(proto::Announcements { entries, more, host: is_host(conn, account)?, maintenance: current(conn)?.map(|m| m.proto()) })
}

/// The maintenance planned or underway.
struct Plan {
    id: i64,
    starts_at: u64,
    minutes: u32,
    reason: String,
    reminders_done: usize,
    started: bool,
}

impl Plan {
    fn ends_at(&self) -> u64 {
        self.starts_at + u64::from(self.minutes) * MINUTE_MS
    }

    fn proto(&self) -> proto::Maintenance {
        let state = if self.started { MaintenanceState::Underway } else { MaintenanceState::Planned };
        proto::Maintenance { starts_at_ms: self.starts_at, minutes: self.minutes, reason: self.reason.clone(), state: state as i32 }
    }

    fn post(&self, tx: &Connection, kind: AnnouncementKind) -> Result<()> {
        post(tx, kind, &self.reason, self.starts_at, self.minutes, None)
    }

    fn update(&self, tx: &Connection, finished: bool) -> Result<()> {
        tx.execute(
            "UPDATE maintenance SET reminders_done = ?1, started = ?2, finished = ?3 WHERE id = ?4",
            params![self.reminders_done as i64, self.started, finished, self.id],
        )
        .db()?;
        Ok(())
    }
}

fn current(conn: &Connection) -> Result<Option<Plan>> {
    conn.query_row(
        "SELECT id, starts_at, minutes, reason, reminders_done, started FROM maintenance WHERE finished = 0 ORDER BY id DESC LIMIT 1",
        [],
        |r| {
            Ok(Plan {
                id: r.get(0)?,
                starts_at: r.get::<_, i64>(1)? as u64,
                minutes: r.get(2)?,
                reason: r.get(3)?,
                reminders_done: r.get::<_, i64>(4)? as usize,
                started: r.get(5)?,
            })
        },
    )
    .optional()
    .db()
}

/// The maintenance planned or underway, for the operator.
pub fn maintenance(conn: &Connection) -> Result<Option<proto::Maintenance>> {
    Ok(current(conn)?.map(|p| p.proto()))
}

// ---------------------------------------------------------------- writing

/// Post an announcement. Retries with the same `client_msg_id` post it once.
/// Returns the new change counter.
pub fn announce(conn: &mut Connection, by: By<'_>, client_msg_id: Option<&[u8]>, text: &str) -> Result<u64> {
    let text = text.trim();
    if text.is_empty() {
        return Err(err(ErrorCode::BadRequest, "an announcement needs some text"));
    }
    check_text(text, "the announcement")?;
    if client_msg_id.is_some_and(|id| id.len() != 16) {
        return Err(err(ErrorCode::BadRequest, "client_msg_id must be 16 bytes"));
    }
    let tx = write_tx(conn)?;
    check_host(&tx, by)?;
    if let Some(id) = client_msg_id {
        let seen: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM announcements WHERE client_msg_id = ?1)", [id], |r| r.get(0)).db()?;
        if seen {
            return version(&tx).db();
        }
    }
    post(&tx, AnnouncementKind::Text, text, 0, 0, client_msg_id)?;
    let v = bump(&tx)?;
    tx.commit().db()?;
    Ok(v)
}

/// Plan maintenance, announcing it now unless `quiet`. Starting now (or in
/// the past) announces the start instead. Returns the new change counter.
pub fn plan(conn: &mut Connection, by: By<'_>, req: &proto::PlanMaintenance) -> Result<u64> {
    plan_at(conn, by, req, now_ms())
}

fn plan_at(conn: &mut Connection, by: By<'_>, req: &proto::PlanMaintenance, now: u64) -> Result<u64> {
    if req.minutes == 0 || req.minutes > proto::MAX_MAINTENANCE_MINUTES {
        return Err(err(ErrorCode::BadRequest, "maintenance lasts from a minute to a week"));
    }
    if req.starts_at_ms > now + MAX_PLAN_AHEAD_MS {
        return Err(err(ErrorCode::BadRequest, "maintenance can be planned at most a year ahead"));
    }
    let reason = req.reason.trim();
    check_text(reason, "the reason")?;
    let starts_at = req.starts_at_ms.max(now);
    let started = starts_at == now;
    // Reminders already past are skipped, and so is one due right after
    // the plan is announced.
    let cutoff = if req.quiet { now } else { now + REMINDER_GAP_MS };
    let reminders_done =
        if started { REMINDERS_MS.len() } else { REMINDERS_MS.iter().take_while(|&&r| starts_at.saturating_sub(r) <= cutoff).count() };

    let tx = write_tx(conn)?;
    check_host(&tx, by)?;
    if current(&tx)?.is_some() {
        return Err(err(ErrorCode::Conflict, "maintenance is already planned or underway: cancel or end it first"));
    }
    tx.execute(
        "INSERT INTO maintenance (starts_at, minutes, reason, reminders_done, started) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![sql_i64(starts_at), req.minutes, reason, reminders_done as i64, started],
    )
    .db()?;
    if started {
        post(&tx, AnnouncementKind::MaintenanceStarted, reason, starts_at, req.minutes, None)?;
    } else if !req.quiet {
        post(&tx, AnnouncementKind::MaintenancePlanned, reason, starts_at, req.minutes, None)?;
    }
    let v = bump(&tx)?;
    tx.commit().db()?;
    Ok(v)
}

/// What [`end`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// The planned maintenance was cancelled before it started.
    Cancelled,
    /// The maintenance underway was announced over.
    Over,
}

/// Cancel the planned maintenance, or end the one underway. Returns what
/// happened and the new change counter.
pub fn end(conn: &mut Connection, by: By<'_>) -> Result<(Ended, u64)> {
    let tx = write_tx(conn)?;
    check_host(&tx, by)?;
    let plan = current(&tx)?.ok_or_else(|| err(ErrorCode::NotFound, "no maintenance is planned or underway"))?;
    let ended = if plan.started { Ended::Over } else { Ended::Cancelled };
    plan.post(&tx, if plan.started { AnnouncementKind::MaintenanceOver } else { AnnouncementKind::MaintenanceCancelled })?;
    plan.update(&tx, true)?;
    let v = bump(&tx)?;
    tx.commit().db()?;
    Ok((ended, v))
}

/// Post what the maintenance plan calls for by now: the latest reminder
/// due, the start, or the end once its time is up. Called every few seconds.
/// After the server was down, it posts only what is still worth saying: the
/// latest reminder rather than every one missed, and "over" rather than
/// "started" for a maintenance that has already ended.
pub fn tick(conn: &mut Connection) -> Result<bool> {
    tick_at(conn, now_ms())
}

fn tick_at(conn: &mut Connection, now: u64) -> Result<bool> {
    // Most ticks find nothing due: decide that without a write lock.
    let Some(plan) = current(conn)? else { return Ok(false) };
    if !due(&plan, now) {
        return Ok(false);
    }
    let tx = write_tx(conn)?;
    let Some(mut plan) = current(&tx)? else { return Ok(false) };
    let posted = step(&tx, &mut plan, now)?;
    if posted {
        bump(&tx)?;
    }
    tx.commit().db()?;
    Ok(posted)
}

fn reminders_due(plan: &Plan, now: u64) -> usize {
    REMINDERS_MS.iter().take_while(|&&r| plan.starts_at.saturating_sub(r) <= now).count()
}

fn due(plan: &Plan, now: u64) -> bool {
    if plan.started { now >= plan.ends_at() } else { now >= plan.starts_at || reminders_due(plan, now) > plan.reminders_done }
}

fn step(tx: &Transaction<'_>, plan: &mut Plan, now: u64) -> Result<bool> {
    if now >= plan.ends_at() {
        plan.reminders_done = REMINDERS_MS.len();
        plan.started = true;
        plan.post(tx, AnnouncementKind::MaintenanceOver)?;
        plan.update(tx, true)?;
        return Ok(true);
    }
    if plan.started {
        return Ok(false);
    }
    if now >= plan.starts_at {
        plan.reminders_done = REMINDERS_MS.len();
        plan.started = true;
        plan.post(tx, AnnouncementKind::MaintenanceStarted)?;
        plan.update(tx, false)?;
        return Ok(true);
    }
    let due = reminders_due(plan, now);
    if due > plan.reminders_done {
        plan.reminders_done = due;
        plan.post(tx, AnnouncementKind::MaintenanceReminder)?;
        plan.update(tx, false)?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = crate::db::open(&dir.path().join("server.db"), &[7; 32]).expect("open");
        (dir, conn)
    }

    fn kinds(conn: &Connection) -> Vec<AnnouncementKind> {
        let mut stmt = conn.prepare("SELECT kind FROM announcements ORDER BY id").expect("prepare");
        stmt.query_map([], |r| r.get::<_, i32>(0))
            .expect("query")
            .map(|k| AnnouncementKind::try_from(k.expect("row")).expect("kind"))
            .collect()
    }

    fn plan_req(starts_at_ms: u64, minutes: u32, quiet: bool) -> proto::PlanMaintenance {
        proto::PlanMaintenance { starts_at_ms, minutes, reason: "moving".into(), quiet }
    }

    use AnnouncementKind::{
        MaintenanceCancelled as Cancelled, MaintenanceOver as Over, MaintenancePlanned as Planned, MaintenanceReminder as Reminder,
        MaintenanceStarted as Started,
    };

    #[test]
    fn a_plan_two_days_ahead_announces_then_reminds_four_times_then_starts_and_ends() {
        let (_d, mut c) = db();
        let t0 = 1_000 * DAY_MS;
        let starts = t0 + 2 * DAY_MS;
        plan_at(&mut c, By::Operator, &plan_req(starts, 15, false), t0).expect("plan");
        assert_eq!(kinds(&c), [Planned]);
        // Nothing due yet; ticking again changes nothing.
        assert!(!tick_at(&mut c, t0 + HOUR_MS).expect("tick"));
        for (i, r) in REMINDERS_MS.iter().enumerate() {
            assert!(tick_at(&mut c, starts - r).expect("tick"), "reminder {i}");
            assert!(!tick_at(&mut c, starts - r + 1).expect("tick"), "reminder {i} only once");
        }
        assert!(tick_at(&mut c, starts).expect("tick"));
        assert!(!tick_at(&mut c, starts + MINUTE_MS).expect("tick"));
        assert!(tick_at(&mut c, starts + 15 * MINUTE_MS).expect("tick"));
        assert_eq!(kinds(&c), [Planned, Reminder, Reminder, Reminder, Reminder, Started, Over]);
        assert!(current(&c).expect("current").is_none());
    }

    #[test]
    fn reminders_already_past_or_right_after_the_plan_are_skipped() {
        let (_d, mut c) = db();
        let t0 = 1_000 * DAY_MS;
        // 62 minutes ahead: the day and hour reminders are skipped (the hour
        // one would come two minutes after the plan).
        let starts = t0 + 62 * MINUTE_MS;
        plan_at(&mut c, By::Operator, &plan_req(starts, 10, false), t0).expect("plan");
        let mut t = t0;
        while current(&c).expect("current").is_some() {
            tick_at(&mut c, t).expect("tick");
            t += MINUTE_MS;
        }
        assert_eq!(kinds(&c), [Planned, Reminder, Reminder, Started, Over]);
    }

    #[test]
    fn a_quiet_plan_still_reminds() {
        let (_d, mut c) = db();
        let t0 = 1_000 * DAY_MS;
        let starts = t0 + 62 * MINUTE_MS;
        plan_at(&mut c, By::Operator, &plan_req(starts, 10, true), t0).expect("plan");
        assert!(kinds(&c).is_empty());
        let mut t = t0;
        while current(&c).expect("current").is_some() {
            tick_at(&mut c, t).expect("tick");
            t += MINUTE_MS;
        }
        assert_eq!(kinds(&c), [Reminder, Reminder, Reminder, Started, Over]);
    }

    #[test]
    fn after_downtime_only_what_is_still_true_is_said() {
        let (_d, mut c) = db();
        let t0 = 1_000 * DAY_MS;
        let starts = t0 + 2 * DAY_MS;
        plan_at(&mut c, By::Operator, &plan_req(starts, 30, false), t0).expect("plan");
        // Down from before the day reminder until 10 minutes before: one
        // reminder, not three.
        assert!(tick_at(&mut c, starts - 10 * MINUTE_MS).expect("tick"));
        assert!(!tick_at(&mut c, starts - 9 * MINUTE_MS).expect("tick"));
        // Down again through the whole maintenance: "over", never "started".
        assert!(tick_at(&mut c, starts + HOUR_MS).expect("tick"));
        assert_eq!(kinds(&c), [Planned, Reminder, Over]);
    }

    #[test]
    fn starting_now_announces_the_start_and_ending_announces_it_over() {
        let (_d, mut c) = db();
        let t0 = 1_000 * DAY_MS;
        plan_at(&mut c, By::Operator, &plan_req(0, 20, false), t0).expect("plan");
        assert_eq!(maintenance(&c).expect("m").expect("plan").state(), MaintenanceState::Underway);
        let err = plan_at(&mut c, By::Operator, &plan_req(0, 20, false), t0).expect_err("one at a time");
        assert_eq!(err.code(), ErrorCode::Conflict);
        assert_eq!(end(&mut c, By::Operator).expect("end").0, Ended::Over);
        assert_eq!(end(&mut c, By::Operator).expect_err("nothing left").code(), ErrorCode::NotFound);
        assert_eq!(kinds(&c), [Started, Over]);
    }

    #[test]
    fn cancelling_before_the_start_says_so_and_stops_the_reminders() {
        let (_d, mut c) = db();
        let t0 = 1_000 * DAY_MS;
        let starts = t0 + 2 * DAY_MS;
        plan_at(&mut c, By::Operator, &plan_req(starts, 15, false), t0).expect("plan");
        assert_eq!(end(&mut c, By::Operator).expect("cancel").0, Ended::Cancelled);
        assert!(!tick_at(&mut c, starts).expect("tick"));
        assert_eq!(kinds(&c), [Planned, Cancelled]);
    }

    #[test]
    fn plans_are_checked() {
        let (_d, mut c) = db();
        let t0 = 1_000 * DAY_MS;
        for bad in [
            plan_req(t0 + HOUR_MS, 0, false),
            plan_req(t0 + HOUR_MS, proto::MAX_MAINTENANCE_MINUTES + 1, false),
            plan_req(t0 + 400 * DAY_MS, 5, false),
        ] {
            assert_eq!(plan_at(&mut c, By::Operator, &bad, t0).expect_err("refused").code(), ErrorCode::BadRequest);
        }
    }

    #[test]
    fn announcements_are_idempotent_and_counted() {
        let (_d, mut c) = db();
        let v0 = version(&c).expect("version");
        let v1 = announce(&mut c, By::Operator, Some(&[1; 16]), "hello").expect("announce");
        assert!(v1 > v0);
        assert_eq!(announce(&mut c, By::Operator, Some(&[1; 16]), "hello").expect("retry"), v1);
        assert_eq!(kinds(&c), [AnnouncementKind::Text]);
        assert_eq!(announce(&mut c, By::Operator, None, "  ").expect_err("empty").code(), ErrorCode::BadRequest);
        let long = "x".repeat(proto::MAX_ANNOUNCEMENT_BYTES + 1);
        assert_eq!(announce(&mut c, By::Operator, None, &long).expect_err("long").code(), ErrorCode::BadRequest);
    }
}
