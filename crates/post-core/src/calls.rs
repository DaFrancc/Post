//! Calls as this device sees them (design section 10, "Calls").
//!
//! A call is a run of `CallEvent` messages in a conversation: one member
//! starts it, others join, leave or decline, and it ends when everyone has
//! left (or, in a DM, when either side hangs up or declines). The state is
//! kept in memory only, never in the database: who was in which call is
//! exactly what Post does not keep. Timeline notices ("Alice started a
//! call") are all that stays.
//!
//! Media keys come from the conversation's current MLS epoch:
//! `call_base = MLS-Exporter("post/call/v1", call_id, 32)`, and each
//! participant's key is HKDF of it with their participant ID, so everyone in
//! the conversation can derive everyone's key and nobody else can.

use std::collections::{BTreeMap, HashMap};

use hkdf::Hkdf;
use post_identity::{AccountId, DeviceId};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::content::{CallEvent, CallKind};

/// How long a call rings after it starts.
pub(crate) const RING_FOR_MS: u64 = 60_000;
/// A call started without naming whom to ring rings everyone in groups
/// up to this size; larger ones show it to join instead (design section 10).
pub(crate) const RING_ALL_MAX: usize = 8;
/// Events older than this when they arrive are history, not a live call.
pub(crate) const STALE_MS: u64 = 2 * 3600 * 1000;
/// MLS exporter label for a call's keys.
pub(crate) const EXPORTER_LABEL: &str = "post/call/v1";
/// Key slots in LiveKit's key ring: the epoch modulo this picks the slot.
pub(crate) const KEY_RING: u64 = 16;

/// One live call.
#[derive(Clone, Debug)]
pub(crate) struct CallState {
    pub group: [u8; 16],
    pub started_by: AccountId,
    pub started_at: u64,
    pub video: bool,
    /// Who is in it: participant ID to their account and device.
    pub participants: BTreeMap<[u8; 16], (AccountId, DeviceId)>,
    /// This device's participant ID, while it is in the call.
    pub mine: Option<[u8; 16]>,
    /// It rings for the user until then (Unix ms; 0 for not ringing).
    pub ring_until: u64,
    /// It rang for the user at some point: only then is an unanswered call
    /// a missed one.
    pub rang: bool,
    /// One of the user's devices started or joined it at some point.
    pub answered: bool,
    /// When someone first joined the starter: the call's real start.
    pub connected_at: Option<u64>,
    pub ended: bool,
}

/// A call event as received (or sent), with who sent it.
pub(crate) struct Seen {
    pub sender: AccountId,
    pub device: DeviceId,
    pub sent_at: u64,
    pub event: CallEvent,
}

impl CallState {
    /// Ringing for the user now.
    pub fn ringing(&self, now: u64) -> bool {
        !self.ended && self.ring_until > now
    }
}

/// What an event did, for the timeline.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Nothing,
    Started,
    /// A start that arrived long after the fact: history, not a live call.
    StartedLongAgo,
    Joined,
    Left,
    Declined,
    /// Someone rang people into the call.
    Rang,
    /// Over. `missed`: it called the user and none of their devices
    /// answered. `lasted`: from the first join to the end.
    Ended { missed: bool, lasted_ms: Option<u64> },
}

/// Who this device is, and where the event happened.
pub(crate) struct Viewer {
    pub account: AccountId,
    pub device: DeviceId,
    pub is_dm: bool,
    pub now: u64,
}

#[derive(Default)]
pub(crate) struct Calls {
    live: HashMap<[u8; 16], CallState>,
}

impl Calls {
    pub fn get(&self, id: &[u8; 16]) -> Option<&CallState> {
        self.live.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = (&[u8; 16], &CallState)> {
        self.live.iter()
    }

    /// Forget a call that has ended.
    pub fn remove(&mut self, id: &[u8; 16]) {
        self.live.remove(id);
    }

    /// Apply one event in conversation `group`.
    pub fn apply(&mut self, group: [u8; 16], v: &Viewer, s: &Seen) -> ([u8; 16], Change) {
        let Ok(kind) = s.event.check() else { return ([0; 16], Change::Nothing) };
        let id: [u8; 16] = s.event.call_id.as_slice().try_into().expect("checked");
        let participant: Option<[u8; 16]> = s.event.participant_id.as_slice().try_into().ok();
        let from_me = s.sender == v.account;
        let from_this_device = from_me && s.device == v.device;
        let age = v.now.saturating_sub(s.sent_at);
        let names_me = s.event.ring.iter().any(|a| a.as_slice() == v.account.0.as_slice());

        if kind == CallKind::Start {
            if self.live.contains_key(&id) {
                return (id, Change::Nothing);
            }
            if age > STALE_MS {
                return (id, Change::StartedLongAgo);
            }
            let p = participant.expect("checked");
            // Everyone, unless it names whom to ring or rings nobody.
            let rings_me = !from_me && !s.event.quiet && (s.event.ring.is_empty() || names_me);
            self.live.insert(
                id,
                CallState {
                    group,
                    started_by: s.sender,
                    started_at: s.sent_at,
                    video: s.event.video,
                    participants: BTreeMap::from([(p, (s.sender, s.device))]),
                    mine: from_this_device.then_some(p),
                    ring_until: if rings_me { s.sent_at + RING_FOR_MS } else { 0 },
                    rang: rings_me,
                    answered: from_me,
                    connected_at: None,
                    ended: false,
                },
            );
            return (id, Change::Started);
        }

        let Some(call) = self.live.get_mut(&id) else { return (id, Change::Nothing) };
        if call.ended || call.group != group {
            return (id, Change::Nothing);
        }
        let change = match kind {
            CallKind::Start => unreachable!("handled above"),
            CallKind::Join => {
                let p = participant.expect("checked");
                call.participants.insert(p, (s.sender, s.device));
                if from_this_device {
                    call.mine = Some(p);
                }
                if from_me {
                    call.ring_until = 0;
                    call.answered = true;
                }
                if s.sender != call.started_by || call.participants.len() > 1 {
                    call.connected_at.get_or_insert(s.sent_at);
                }
                Change::Joined
            }
            CallKind::Leave => {
                let p = participant.expect("checked");
                // Only the one who joined under a name can leave under it.
                if call.participants.get(&p).is_some_and(|(a, _)| *a == s.sender) {
                    call.participants.remove(&p);
                }
                if call.mine == Some(p) && from_this_device {
                    call.mine = None;
                }
                if call.participants.is_empty() { end(call, s.sent_at) } else { Change::Left }
            }
            CallKind::Decline => {
                if from_me {
                    call.ring_until = 0;
                }
                // In a DM, the other side declining ends the call.
                if v.is_dm && s.sender != call.started_by { end(call, s.sent_at) } else { Change::Declined }
            }
            CallKind::End => {
                // Hanging up ends a DM call; in a group only the starter
                // can end it for everyone (it otherwise ends as people leave).
                if v.is_dm || s.sender == call.started_by { end(call, s.sent_at) } else { Change::Nothing }
            }
            CallKind::Ring => {
                // Rung into a call the user's devices are not in: ring again,
                // for another minute, from when it was sent.
                let in_it = call.participants.values().any(|(a, _)| *a == v.account);
                if names_me && !from_me && !in_it && age < RING_FOR_MS {
                    call.ring_until = s.sent_at + RING_FOR_MS;
                    call.rang = true;
                }
                Change::Rang
            }
        };
        (id, change)
    }
}

fn end(call: &mut CallState, at: u64) -> Change {
    call.ended = true;
    call.ring_until = 0;
    call.mine = None;
    Change::Ended { missed: call.rang && !call.answered, lasted_ms: call.connected_at.map(|c| at.saturating_sub(c)) }
}

/// A participant's media key, from the call's base secret for this epoch.
pub(crate) fn sender_key(call_base: &[u8], call_id: &[u8], participant: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut info = b"post/call/sender".to_vec();
    info.extend_from_slice(participant);
    let mut key = Zeroizing::new(vec![0u8; 32]);
    Hkdf::<Sha256>::new(Some(call_id), call_base).expand(&info, &mut key).expect("32 bytes is a valid HKDF length");
    key
}

/// How long a call lasted, for its notice: "3 min", "1 h 5 min", "40 s".
pub(crate) fn lasted(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use post_identity::LinkHash;

    const ALICE: AccountId = LinkHash([1; 32]);
    const BOB: AccountId = LinkHash([2; 32]);
    const PHONE: DeviceId = DeviceId([1; 16]);
    const LAPTOP: DeviceId = DeviceId([2; 16]);
    const BOB_DEV: DeviceId = DeviceId([3; 16]);
    const G: [u8; 16] = [9; 16];
    const CALL: [u8; 16] = [7; 16];

    fn seen(sender: AccountId, device: DeviceId, at: u64, kind: CallKind, participant: Option<u8>) -> Seen {
        Seen {
            sender,
            device,
            sent_at: at,
            event: CallEvent {
                kind: kind as i32,
                call_id: CALL.to_vec(),
                participant_id: participant.map(|p| vec![p; 16]).unwrap_or_default(),
                ..Default::default()
            },
        }
    }

    fn ringing(calls: &Calls, now: u64) -> bool {
        calls.get(&CALL).expect("call").ringing(now)
    }

    fn viewer(account: AccountId, device: DeviceId, is_dm: bool, now: u64) -> Viewer {
        Viewer { account, device, is_dm, now }
    }

    #[test]
    fn a_dm_call_rings_is_answered_and_ends() {
        let mut calls = Calls::default();
        let alice_phone = viewer(ALICE, PHONE, true, 1_000);
        // Bob calls.
        assert_eq!(calls.apply(G, &alice_phone, &seen(BOB, BOB_DEV, 1_000, CallKind::Start, Some(10))).1, Change::Started);
        assert!(ringing(&calls, 1_000));
        assert!(!ringing(&calls, 1_000 + RING_FOR_MS), "it stops ringing after a minute");
        // Alice answers on her laptop: her phone stops ringing.
        let later = viewer(ALICE, PHONE, true, 2_000);
        assert_eq!(calls.apply(G, &later, &seen(ALICE, LAPTOP, 2_000, CallKind::Join, Some(11))).1, Change::Joined);
        let c = calls.get(&CALL).expect("call");
        assert!(!c.ringing(2_000) && c.answered && c.mine.is_none(), "answered elsewhere, not on this device");
        assert_eq!(c.participants.len(), 2);
        // Bob hangs up after five minutes.
        let end = calls.apply(G, &later, &seen(BOB, BOB_DEV, 2_000 + 300_000, CallKind::End, None)).1;
        assert_eq!(end, Change::Ended { missed: false, lasted_ms: Some(300_000) });
        assert_eq!(lasted(300_000), "5 min");
    }

    #[test]
    fn unanswered_calls_are_missed_and_old_ones_do_not_ring() {
        let mut calls = Calls::default();
        let v = viewer(ALICE, PHONE, true, 1_000);
        calls.apply(G, &v, &seen(BOB, BOB_DEV, 1_000, CallKind::Start, Some(10)));
        let end = calls.apply(G, &v, &seen(BOB, BOB_DEV, 61_000, CallKind::Leave, Some(10))).1;
        assert_eq!(end, Change::Ended { missed: true, lasted_ms: None });

        // Fetched after a minute: a live call, but no longer ringing.
        let mut calls = Calls::default();
        calls.apply(G, &viewer(ALICE, PHONE, true, 1_000 + RING_FOR_MS + 1), &seen(BOB, BOB_DEV, 1_000, CallKind::Start, Some(10)));
        assert!(!ringing(&calls, 1_000 + RING_FOR_MS + 1));
        // Fetched hours later: history.
        let mut calls = Calls::default();
        let (_, change) = calls.apply(G, &viewer(ALICE, PHONE, true, STALE_MS + 2_000), &seen(BOB, BOB_DEV, 1_000, CallKind::Start, Some(10)));
        assert_eq!(change, Change::StartedLongAgo);
        assert!(calls.get(&CALL).is_none());
    }

    #[test]
    fn group_calls_end_when_everyone_leaves_and_names_are_not_stolen() {
        let mut calls = Calls::default();
        let v = viewer(ALICE, PHONE, false, 1_000);
        calls.apply(G, &v, &seen(ALICE, PHONE, 1_000, CallKind::Start, Some(10)));
        assert_eq!(calls.get(&CALL).expect("call").mine, Some([10; 16]), "started here: this device is in it");
        calls.apply(G, &v, &seen(BOB, BOB_DEV, 2_000, CallKind::Join, Some(20)));
        // Bob cannot make Alice leave by sending her name.
        assert_eq!(calls.apply(G, &v, &seen(BOB, BOB_DEV, 3_000, CallKind::Leave, Some(10))).1, Change::Left);
        assert!(calls.get(&CALL).expect("call").participants.contains_key(&[10; 16]));
        // Nor end a group call he did not start.
        assert_eq!(calls.apply(G, &v, &seen(BOB, BOB_DEV, 3_000, CallKind::End, None)).1, Change::Nothing);
        calls.apply(G, &v, &seen(BOB, BOB_DEV, 4_000, CallKind::Leave, Some(20)));
        let end = calls.apply(G, &v, &seen(ALICE, PHONE, 5_000, CallKind::Leave, Some(10))).1;
        assert_eq!(end, Change::Ended { missed: false, lasted_ms: Some(3_000) });
        // An event for another group's call with the same ID changes nothing.
        let mut calls = Calls::default();
        calls.apply(G, &v, &seen(ALICE, PHONE, 1_000, CallKind::Start, Some(10)));
        assert_eq!(calls.apply([8; 16], &v, &seen(BOB, BOB_DEV, 2_000, CallKind::Join, Some(20))).1, Change::Nothing);
    }

    #[test]
    fn keys_differ_per_participant_and_per_epoch() {
        let a = sender_key(&[1; 32], &CALL, &[10; 16]);
        assert_eq!(a.len(), 32);
        assert_eq!(a, sender_key(&[1; 32], &CALL, &[10; 16]));
        assert_ne!(a, sender_key(&[1; 32], &CALL, &[11; 16]), "another participant");
        assert_ne!(a, sender_key(&[2; 32], &CALL, &[10; 16]), "another epoch's base");
        assert_ne!(a, sender_key(&[1; 32], &[8; 16], &[10; 16]), "another call");
    }

    #[test]
    fn a_call_rings_only_whom_it_names_and_can_ring_more_later() {
        const CAROL: AccountId = LinkHash([3; 32]);
        let names = |who: &[AccountId]| who.iter().map(|a| a.0.to_vec()).collect::<Vec<_>>();
        let mut start = seen(BOB, BOB_DEV, 1_000, CallKind::Start, Some(10));
        start.event.ring = names(&[ALICE]);
        // Alice is named: it rings for her.
        let mut for_alice = Calls::default();
        for_alice.apply(G, &viewer(ALICE, PHONE, false, 1_000), &start);
        assert!(ringing(&for_alice, 1_000));
        // Carol is not: the call is there to join, silently.
        let mut for_carol = Calls::default();
        let carol = viewer(CAROL, DeviceId([4; 16]), false, 1_000);
        for_carol.apply(G, &carol, &start);
        assert!(!ringing(&for_carol, 1_000));
        // Bob rings her into the call later, and it rings for a minute from then.
        let mut ring = seen(BOB, BOB_DEV, 30_000, CallKind::Ring, None);
        ring.event.ring = names(&[CAROL]);
        let carol_later = viewer(CAROL, DeviceId([4; 16]), false, 30_000);
        assert_eq!(for_carol.apply(G, &carol_later, &ring).1, Change::Rang);
        assert!(ringing(&for_carol, 30_000 + RING_FOR_MS - 1));
        // Nobody missed a call they were never rung for.
        let mut for_dave = Calls::default();
        let dave = viewer(LinkHash([5; 32]), DeviceId([5; 16]), false, 1_000);
        for_dave.apply(G, &dave, &start);
        let end = for_dave.apply(G, &dave, &seen(BOB, BOB_DEV, 2_000, CallKind::Leave, Some(10))).1;
        assert_eq!(end, Change::Ended { missed: false, lasted_ms: None });

        // A quiet start rings nobody.
        let mut quiet = seen(BOB, BOB_DEV, 1_000, CallKind::Start, Some(10));
        quiet.event.quiet = true;
        let mut calls = Calls::default();
        calls.apply(G, &viewer(ALICE, PHONE, false, 1_000), &quiet);
        assert!(!ringing(&calls, 1_000));
    }

    #[test]
    fn malformed_events_are_ignored() {
        let mut calls = Calls::default();
        let v = viewer(ALICE, PHONE, true, 1_000);
        let mut bad = seen(BOB, BOB_DEV, 1_000, CallKind::Start, None);
        assert_eq!(calls.apply(G, &v, &bad).1, Change::Nothing, "a start without a participant");
        bad.event.kind = 99;
        assert_eq!(calls.apply(G, &v, &bad).1, Change::Nothing);
    }
}
