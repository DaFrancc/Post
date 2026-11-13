//! End to end: a real server and real clients in one process (design section
//! 15). Phase 2's done-when: two devices exchange, edit and delete DMs and see
//! each other's profiles.

use std::path::Path;
use std::time::Duration;

use post_core::{Core, DbKey, Event, MessageState, ProfileUpdate};
use tempfile::TempDir;
use zeroize::Zeroizing;

struct Server {
    _running: post_server::Running,
    url: String,
    db: std::path::PathBuf,
    key: [u8; 32],
    _dir: TempDir,
}

impl Server {
    async fn start() -> Self {
        Self::start_with(None).await
    }

    async fn start_with(calls: Option<post_server::CallConfig>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("server.db");
        let key = [42u8; 32];
        let config = post_server::Config {
            server_domain: format!("127.0.0.1:{}", addr.port()),
            db_path: db.clone(),
            db_key: Zeroizing::new(key),
            releases: post_server::ReleaseConfig::none(),
            calls,
            blobs: post_server::BlobConfig::beside(&db),
        };
        let running = post_server::spawn(listener, config).expect("server starts");
        Self { url: running.url(), _running: running, db, key, _dir: dir }
    }

    fn invite(&self, bot: bool) -> String {
        let mut conn = post_server::db::open(&self.db, &self.key).expect("open server db");
        post_server::db::create_invite(&mut conn, bot, 3_600_000).expect("invite")
    }

    /// The sum of every group's epoch: each commit anywhere adds one.
    fn total_epochs(&self) -> i64 {
        let conn = post_server::db::open(&self.db, &self.key).expect("open server db");
        conn.query_row("SELECT COALESCE(SUM(epoch), 0) FROM groups", [], |r| r.get(0)).expect("epochs")
    }

    /// Entries in every group log on the server.
    fn log_entries(&self) -> i64 {
        let conn = post_server::db::open(&self.db, &self.key).expect("open server db");
        conn.query_row("SELECT COUNT(*) FROM group_log", [], |r| r.get(0)).expect("count")
    }

    /// Make a device look unseen for `days`, and run the server's hourly
    /// stale-device check.
    fn age_device(&self, device_hex: &str, days: i64) {
        let mut conn = post_server::db::open(&self.db, &self.key).expect("open server db");
        let id = hex::decode(device_hex).expect("hex");
        conn.execute("UPDATE devices SET last_seen_day = last_seen_day - ?1 WHERE device_id = ?2", (days, id)).expect("age");
        post_server::db::mark_stale_devices(&mut conn).expect("mark stale");
    }

    async fn client(&self, dir: &Path, username: &str) -> Core {
        let core = Core::open(&dir.join(username), &DbKey::from_bytes([7; 32])).expect("open profile");
        core.register(&self.url, &self.invite(false), username, false).await.expect("register");
        core
    }
}

async fn next_event(rx: &mut tokio::sync::broadcast::Receiver<Event>, want: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let e = rx.recv().await.expect("event stream open");
            if want(&e) {
                return e;
            }
        }
    })
    .await
    .expect("event arrived in time")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dm_messages_edits_deletes_and_profiles() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;

    alice
        .set_profile(ProfileUpdate {
            display_name: Some("Alice A".into()),
            typing_template: Some("the one who types is $$NAME$$".into()),
            note: Some("at the beach".into()),
            note_keep: false,
            arrival: Some("decode".into()),
        })
        .await
        .expect("profile");
    let bad = ProfileUpdate { arrival: Some("Not A Name".into()), ..Default::default() };
    assert!(alice.set_profile(bad).await.is_err(), "an arrival style must be a short lowercase name");

    // Alice opens a DM and writes.
    let dm = alice.start_dm("bob").await.expect("start dm");
    assert_eq!(alice.start_dm("bob").await.expect("same dm").id, dm.id, "one DM per pair");
    let hello = alice.send_text(dm.id, "hello bob").await.expect("send");
    assert_eq!(hello.state, MessageState::Sent);
    assert_eq!(hello.sender, "Alice A");

    // Bob joins from the Welcome and reads it, with Alice's profile.
    bob.sync().await.expect("bob sync");
    let convs = bob.conversations().expect("conversations");
    assert_eq!(convs.len(), 1);
    assert_eq!(convs[0].id, dm.id);
    assert_eq!(convs[0].title, "Alice A");
    let msgs = bob.messages(dm.id, 50).expect("messages");
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].body.as_deref(), Some("hello bob"));
    assert_eq!(msgs[0].sender, "Alice A");
    assert_eq!(msgs[0].sender_username, "alice");
    let alice_seen_by_bob = bob.person("alice").await.expect("person");
    assert_eq!(alice_seen_by_bob.typing_template, "the one who types is $$NAME$$");
    assert_eq!(alice_seen_by_bob.note.as_deref(), Some("at the beach"));
    assert_eq!(alice_seen_by_bob.arrival, "decode", "her messages arrive her way on Bob's screen too");

    // Bob replies.
    bob.send_text(dm.id, "hi alice").await.expect("reply");
    alice.sync().await.expect("alice sync");
    let a_msgs = alice.messages(dm.id, 50).expect("messages");
    assert_eq!(a_msgs.iter().map(|m| m.body.as_deref()).collect::<Vec<_>>(), vec![Some("hello bob"), Some("hi alice")]);
    assert!(a_msgs[0].outgoing && !a_msgs[1].outgoing);

    // Edits: every version is kept, everyone sees the same ones.
    alice.edit_message(dm.id, hello.id, "hello bob!").await.expect("edit 1");
    alice.edit_message(dm.id, hello.id, "hello, bob").await.expect("edit 2");
    bob.sync().await.expect("bob sync");
    let edited = bob.message(dm.id, hello.id).expect("message");
    assert_eq!(edited.body.as_deref(), Some("hello, bob"));
    assert!(edited.edited);
    let versions: Vec<String> = bob.message_versions(dm.id, hello.id).expect("versions").into_iter().map(|v| v.body).collect();
    assert_eq!(versions, vec!["hello bob", "hello bob!", "hello, bob"]);
    assert_eq!(
        alice.message_versions(dm.id, hello.id).expect("versions").into_iter().map(|v| v.body).collect::<Vec<_>>(),
        versions
    );

    // Only the sender may edit or delete.
    assert!(bob.edit_message(dm.id, hello.id, "forged").await.is_err());
    assert!(bob.delete_message(dm.id, hello.id).await.is_err());

    // Deleting removes every version for everyone.
    alice.delete_message(dm.id, hello.id).await.expect("delete");
    bob.sync().await.expect("bob sync");
    let gone = bob.message(dm.id, hello.id).expect("marker stays");
    assert!(gone.deleted);
    assert_eq!(gone.body, None);
    assert!(bob.message_versions(dm.id, hello.id).expect("versions").is_empty());
    assert!(alice.edit_message(dm.id, hello.id, "too late").await.is_err());

    // Names: per-conversation display name, then a private nickname over it.
    alice.set_conversation_name(dm.id, "Al").await.expect("conversation name");
    bob.sync().await.expect("bob sync");
    assert_eq!(bob.messages(dm.id, 50).expect("messages")[0].sender, "Al");
    bob.set_nickname("alice", "Ally").await.expect("nickname");
    assert_eq!(bob.messages(dm.id, 50).expect("messages")[0].sender, "Ally");
    assert_eq!(bob.conversations().expect("conversations")[0].title, "Ally");
    // Alice never learns Bob's nickname for her.
    alice.sync().await.expect("alice sync");
    assert_eq!(alice.person("alice").await.expect("self").nickname, "");
    bob.set_nickname("alice", "").await.expect("clear nickname");
    assert_eq!(bob.messages(dm.id, 50).expect("messages")[0].sender, "Al");

    // Both see the same safety number.
    assert_eq!(alice.safety_number("bob").await.expect("sn"), bob.safety_number("alice").await.expect("sn"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_finds_messages() {
    use post_core::SearchQuery;
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let _carol = server.client(dir.path(), "carol").await;

    let dm = alice.start_dm("bob").await.expect("dm");
    alice.send_text(dm.id, "Meet at the École at noon").await.expect("send");
    let typo = alice.send_text(dm.id, "bring the rope").await.expect("send");
    alice.edit_message(dm.id, typo.id, "bring the quickdraws").await.expect("edit");
    let gone = alice.send_text(dm.id, "secret rope plan").await.expect("send");
    alice.delete_message(dm.id, gone.id).await.expect("delete");
    let group = alice.create_group("climbing", &["bob".into(), "carol".into()]).await.expect("group");
    alice.send_text(group.id, "rope check at noon, see https://example.org/topo").await.expect("send");
    bob.sync().await.expect("sync");
    bob.send_text(dm.id, "noon works").await.expect("reply");
    alice.sync().await.expect("sync");

    let q = |text: &str| SearchQuery { text: text.into(), limit: 50, ..Default::default() };
    let bodies = |found: post_core::SearchResults| found.messages.into_iter().filter_map(|m| m.body).collect::<Vec<_>>();
    let search = |query: &SearchQuery| alice.search(query).expect("search");
    // Every word, in any case; newest first, across conversations.
    assert_eq!(
        bodies(search(&q("NOON"))),
        vec!["noon works", "rope check at noon, see https://example.org/topo", "Meet at the École at noon"]
    );
    // Accents ignored either way, and a word matches by its start.
    assert_eq!(bodies(search(&q("ecole noon"))), vec!["Meet at the École at noon"]);
    assert_eq!(bodies(search(&q("ÉCOLE"))), vec!["Meet at the École at noon"]);
    assert_eq!(bodies(search(&q("quickd"))), vec!["bring the quickdraws"]);
    // An edit is found by its new text only; a deleted message not at all.
    assert!(bodies(search(&q("bring rope"))).is_empty());
    assert_eq!(bodies(search(&q("rope"))), vec!["rope check at noon, see https://example.org/topo"]);
    // What is typed is never read as query syntax.
    assert!(bodies(search(&q("noon\" OR \"x"))).is_empty());
    assert!(bodies(search(&q("NEAR(noon"))).is_empty());
    // Narrowed to one conversation, one sender, or messages with links.
    assert_eq!(bodies(search(&SearchQuery { conversation: Some(dm.id), ..q("noon") })), vec!["noon works", "Meet at the École at noon"]);
    assert_eq!(bodies(search(&SearchQuery { from: Some("bob".into()), ..q("") })), vec!["noon works"]);
    assert!(bodies(search(&SearchQuery { from: Some("nobody".into()), ..q("noon") })).is_empty());
    assert_eq!(bodies(search(&SearchQuery { has_link: true, ..q("") })), vec!["rope check at noon, see https://example.org/topo"]);
    // Dates: nothing before the epoch's first second, everything after it.
    assert!(bodies(search(&SearchQuery { before_ms: Some(1000), ..q("noon") })).is_empty());
    assert_eq!(bodies(search(&SearchQuery { after_ms: Some(1000), ..q("noon") })).len(), 3);
    // Oldest first, and by pages: the total counts every match.
    let page = search(&SearchQuery { oldest_first: true, limit: 2, offset: 1, ..q("noon") });
    assert_eq!(page.total, 3);
    assert_eq!(bodies(page), vec!["rope check at noon, see https://example.org/topo", "noon works"]);
    assert_eq!(search(&q("   ")).total, 0, "nothing to look for finds nothing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mentions_are_counted_and_found() {
    use post_core::SearchQuery;
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let _carol = server.client(dir.path(), "carol").await;

    let group = alice.create_group("climbing", &["bob".into(), "carol".into()]).await.expect("group");
    alice.send_text(group.id, "@bob can you drive?").await.expect("send");
    alice.send_text(group.id, "and @carol brings the rope").await.expect("send");
    let fixed = alice.send_text(group.id, "who has the topo?").await.expect("send");
    let gone = alice.send_text(group.id, "@bob ignore this").await.expect("send");
    alice.send_text(group.id, "not one: `@bob` or bob@example.org").await.expect("send");
    bob.sync().await.expect("sync");

    let conv = bob.conversation(group.id).expect("conversation");
    assert_eq!((conv.unread, conv.mentions), (5, 2));
    // An edit that adds a mention counts; a deleted message stops counting.
    alice.edit_message(group.id, fixed.id, "@bob who has the topo?").await.expect("edit");
    alice.delete_message(group.id, gone.id).await.expect("delete");
    bob.sync().await.expect("sync");
    let conv = bob.conversation(group.id).expect("conversation");
    assert_eq!((conv.unread, conv.mentions), (4, 2));
    // Your own mentions of yourself are not unread.
    bob.send_text(group.id, "on it, @bob reporting").await.expect("send");
    assert_eq!(bob.conversation(group.id).expect("conversation").mentions, 2);
    bob.mark_read(group.id).await.expect("read");
    assert_eq!(bob.conversation(group.id).expect("conversation").mentions, 0);

    // Search finds them by who they mention, with words or alone.
    let bodies = |found: post_core::SearchResults| found.messages.into_iter().filter_map(|m| m.body).collect::<Vec<_>>();
    let q = |who: &str, text: &str| SearchQuery { mentions: Some(who.into()), text: text.into(), limit: 50, ..Default::default() };
    assert_eq!(bodies(bob.search(&q("bob", "")).expect("search")), vec!["on it, @bob reporting", "@bob who has the topo?", "@bob can you drive?"]);
    assert_eq!(bodies(bob.search(&q("bob", "topo")).expect("search")), vec!["@bob who has the topo?"]);
    assert_eq!(bodies(bob.search(&q("carol", "")).expect("search")), vec!["and @carol brings the rope"]);
    assert!(bodies(bob.search(&q("dave", "")).expect("search")).is_empty());

    // Survives a restart (the database's own function is there again).
    drop(bob);
    let bob = Core::open(&dir.path().join("bob"), &DbKey::from_bytes([7; 32])).expect("reopen");
    assert_eq!(bodies(bob.search(&q("carol", "")).expect("search")), vec!["and @carol brings the rope"]);
}

fn call_config() -> post_server::CallConfig {
    post_server::CallConfig {
        url: "wss://rtc.example.org".into(),
        api_key: "post".into(),
        api_secret: Zeroizing::new(b"a test secret that is long enough".to_vec()),
        max_bitrate_kbps: 1200,
        max_participants: 8,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dm_call_rings_connects_and_ends() {
    let server = Server::start_with(Some(call_config())).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let dm = alice.start_dm("bob").await.expect("dm");
    alice.send_text(dm.id, "hi").await.expect("send");
    bob.sync().await.expect("sync");

    // Alice calls: she gets a token for the room, with the host's limits.
    let joined = alice.start_call(dm.id, false, &[]).await.expect("start");
    assert_eq!((joined.url.as_str(), joined.max_bitrate_kbps, joined.max_participants), ("wss://rtc.example.org", 1200, 8));
    assert_eq!(joined.token.split('.').count(), 3, "a JWT");
    assert!(joined.call.joined && !joined.call.ringing);
    let call = joined.call.id;

    // Bob's device rings.
    let mut bob_events = bob.subscribe();
    bob.sync().await.expect("sync");
    let Event::CallChanged(ringing) = next_event(&mut bob_events, |e| matches!(e, Event::CallChanged(_))).await else { unreachable!() };
    assert!(ringing.ringing && !ringing.joined);
    assert_eq!(ringing.started_by, "alice");

    // He answers, under his own participant ID.
    let bob_joined = bob.join_call(dm.id, call).await.expect("join");
    assert_ne!(bob_joined.participant_id, joined.participant_id);
    assert_ne!(bob_joined.token, joined.token);
    alice.sync().await.expect("sync");
    let in_call = alice.calls().await.expect("calls");
    assert_eq!(in_call.len(), 1);
    assert_eq!(in_call[0].participants.len(), 2);

    // Both derive the same key for each participant, and keys differ by
    // participant.
    let (ka, kb) = (alice.call_keys(dm.id, call).await.expect("keys"), bob.call_keys(dm.id, call).await.expect("keys"));
    assert_eq!(ka, kb);
    assert_eq!(ka.keys.len(), 2);
    assert_ne!(ka.keys[0].1, ka.keys[1].1);

    // Bob hangs up: in a DM that ends the call for both.
    bob.leave_call(dm.id, call).await.expect("leave");
    alice.sync().await.expect("sync");
    assert!(alice.calls().await.expect("calls").is_empty());
    assert!(bob.calls().await.expect("calls").is_empty());
    let notices = |c: &Core| c.notices(Some(dm.id), 10).expect("notices").into_iter().map(|n| n.text).collect::<Vec<_>>();
    assert_eq!(notices(&alice)[..2], ["you started a call".to_string(), "call ended · 0 s".to_string()]);
    assert_eq!(notices(&bob)[..2], ["alice started a call".to_string(), "call ended · 0 s".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_call_keys_move_on_with_the_members() {
    let server = Server::start_with(Some(call_config())).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let carol = server.client(dir.path(), "carol").await;
    let group = alice.create_group("climbing", &["bob".into(), "carol".into()]).await.expect("group");
    bob.sync().await.expect("sync");
    carol.sync().await.expect("sync");

    let call = alice.start_call(group.id, true, &[]).await.expect("start").call.id;
    bob.sync().await.expect("sync");
    bob.join_call(group.id, call).await.expect("join");
    carol.sync().await.expect("sync");
    assert!(carol.calls().await.expect("calls")[0].ringing, "a small group rings");
    alice.sync().await.expect("sync");
    let before = alice.call_keys(group.id, call).await.expect("keys");
    assert_eq!(before, bob.call_keys(group.id, call).await.expect("keys"));

    // Carol is removed: the epoch moves on, and every key with it.
    alice.remove_members(group.id, &["carol".into()]).await.expect("remove");
    bob.sync().await.expect("sync");
    let after = alice.call_keys(group.id, call).await.expect("keys");
    assert_ne!(after.key_index, before.key_index, "a new slot, so old frames still decrypt during the change");
    assert!(before.keys.iter().zip(&after.keys).all(|(b, a)| b.0 == a.0 && b.1 != a.1), "same participants, new keys");
    assert_eq!(after, bob.call_keys(group.id, call).await.expect("keys"));

    // A group call ends when the last one leaves.
    bob.leave_call(group.id, call).await.expect("leave");
    alice.sync().await.expect("sync");
    assert_eq!(alice.calls().await.expect("calls")[0].participants.len(), 1);
    alice.leave_call(group.id, call).await.expect("leave");
    assert!(alice.calls().await.expect("calls").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_group_call_rings_whom_it_names_and_rings_more_later() {
    let server = Server::start_with(Some(call_config())).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let carol = server.client(dir.path(), "carol").await;
    let group = alice.create_group("climbing", &["bob".into(), "carol".into()]).await.expect("group");
    bob.sync().await.expect("sync");
    carol.sync().await.expect("sync");

    // Alice rings only Bob.
    let call = alice.start_call(group.id, false, &["bob".into()]).await.expect("start").call.id;
    bob.sync().await.expect("sync");
    carol.sync().await.expect("sync");
    assert!(bob.calls().await.expect("calls")[0].ringing, "Bob is rung");
    let carols = carol.calls().await.expect("calls");
    assert!(!carols[0].ringing, "Carol is not rung, but sees the call to join");

    // Then she rings Carol into it; Bob, already rung, is not rung again
    // once he is in it.
    bob.join_call(group.id, call).await.expect("join");
    alice.sync().await.expect("sync");
    alice.ring_call(group.id, call, &["carol".into()]).await.expect("ring");
    carol.sync().await.expect("sync");
    assert!(carol.calls().await.expect("calls")[0].ringing, "Carol is rung now");
    let e = alice.ring_call(group.id, call, &["bob".into()]).await.expect_err("Bob is in it");
    assert!(e.to_string().contains("already in the call"), "{e}");
    let e = alice.ring_call(group.id, call, &["mallory".into()]).await.expect_err("not a member");
    assert!(e.to_string().contains("mallory"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dm_call_hangs_up_from_a_device_that_lost_its_call() {
    let server = Server::start_with(Some(call_config())).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let dm = bob.start_dm("alice").await.expect("dm");
    let call = bob.start_call(dm.id, false, &[]).await.expect("start").call.id;
    alice.sync().await.expect("sync");
    assert_eq!(alice.calls().await.expect("calls").len(), 1);

    // Bob's app restarts: calls live in memory, so his is gone. Hanging up
    // still ends a DM call.
    drop(bob);
    let bob = Core::open(&dir.path().join("bob"), &DbKey::from_bytes([7; 32])).expect("reopen");
    bob.connect().await.expect("connect");
    assert!(bob.calls().await.expect("calls").is_empty());
    bob.leave_call(dm.id, call).await.expect("hang up");
    alice.sync().await.expect("sync");
    assert!(alice.calls().await.expect("calls").is_empty(), "ended for Alice too");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_without_calls_refuses_before_anyone_rings() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let dm = alice.start_dm("bob").await.expect("dm");
    let e = alice.start_call(dm.id, false, &[]).await.expect_err("no calls here");
    assert!(e.to_string().contains("does not offer calls"), "{e}");
    bob.sync().await.expect("sync");
    assert!(bob.calls().await.expect("calls").is_empty(), "nobody was rung");
    assert!(alice.calls().await.expect("calls").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unread_counts_and_previews() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;

    let dm = alice.start_dm("bob").await.expect("dm");
    alice.send_text(dm.id, "one").await.expect("send");
    let two = alice.send_text(dm.id, "two").await.expect("send");
    bob.sync().await.expect("bob sync");

    let conv = bob.conversation(dm.id).expect("conversation");
    assert_eq!(conv.unread, 2);
    assert_eq!(conv.last_message.as_ref().and_then(|m| m.body.as_deref()), Some("two"));
    // Your own messages are never unread.
    assert_eq!(alice.conversation(dm.id).expect("conversation").unread, 0);

    let mut events = bob.subscribe();
    bob.mark_read(dm.id).await.expect("mark read");
    let e = next_event(&mut events, |e| matches!(e, Event::ConversationChanged(_))).await;
    let Event::ConversationChanged(c) = e else { unreachable!() };
    assert_eq!(c.unread, 0);

    // A deleted message stops counting; a new one counts.
    alice.send_text(dm.id, "three").await.expect("send");
    let four = alice.send_text(dm.id, "four").await.expect("send");
    alice.delete_message(dm.id, four.id).await.expect("delete");
    bob.sync().await.expect("bob sync");
    let conv = bob.conversation(dm.id).expect("conversation");
    assert_eq!(conv.unread, 1);
    assert!(conv.last_message.as_ref().is_some_and(|m| m.deleted), "the newest message is the deleted one");

    // Reading survives a restart, and an edit does not make a message unread.
    bob.mark_read(dm.id).await.expect("mark read");
    alice.edit_message(dm.id, two.id, "two, edited").await.expect("edit");
    bob.sync().await.expect("bob sync");
    assert_eq!(bob.conversation(dm.id).expect("conversation").unread, 0);
    drop(bob);
    let bob = Core::open(&dir.path().join("bob"), &DbKey::from_bytes([7; 32])).expect("reopen");
    assert_eq!(bob.conversations().expect("conversations")[0].unread, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_delivery_through_wakes() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;

    let mut bob_events = bob.subscribe();
    let listener = {
        let bob = bob.clone();
        tokio::spawn(async move { bob.listen().await })
    };

    let dm = alice.start_dm("bob").await.expect("dm");
    next_event(&mut bob_events, |e| matches!(e, Event::ConversationAdded(c) if c.id == dm.id)).await;
    alice.send_text(dm.id, "are you there?").await.expect("send");
    let e = next_event(&mut bob_events, |e| matches!(e, Event::MessageAdded(_))).await;
    let Event::MessageAdded(m) = e else { unreachable!() };
    assert_eq!(m.body.as_deref(), Some("are you there?"));
    listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn announcements_reach_everyone() {
    use post_core::AnnouncementKind as K;
    use post_server::announce::{self, By};
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let mut alice_events = alice.subscribe();
    let mut bob_events = bob.subscribe();
    let listeners = [alice.clone(), bob.clone()].map(|c| tokio::spawn(async move { c.listen().await }));
    let added = |kind: K| move |e: &Event| matches!(e, Event::AnnouncementAdded(a) if a.kind == kind);

    // The operator, at the server's command line (another process in real
    // life): the running server notices within seconds and wakes everyone.
    {
        let mut conn = post_server::db::open(&server.db, &server.key).expect("open server db");
        announce::announce(&mut conn, By::Operator, None, "Welcome to Post!").expect("announce");
    }
    let e = next_event(&mut bob_events, added(K::Text)).await;
    let Event::AnnouncementAdded(a) = e else { unreachable!() };
    assert_eq!(a.text, "Welcome to Post!");

    // Only hosts may post.
    let refused = bob.announce("me too").await.expect_err("bob is no host");
    assert!(matches!(&refused, post_core::Error::Server(e) if e.code() == post_proto::ErrorCode::Forbidden), "{refused}");
    assert!(!bob.announcement_summary().expect("summary").host);

    // Alice is made a host; her app learns it without logging in again.
    {
        let mut conn = post_server::db::open(&server.db, &server.key).expect("open server db");
        assert!(announce::add_host(&mut conn, "alice").expect("add host"));
    }
    next_event(&mut alice_events, |e| matches!(e, Event::AnnouncementsChanged(s) if s.host)).await;

    alice.announce("Calls work in groups now.\n– Alice").await.expect("host announces");
    next_event(&mut bob_events, added(K::Text)).await;

    // Maintenance in two days: announced now, with the time as data.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as u64;
    let starts = now + 2 * 24 * 3600 * 1000;
    alice.plan_maintenance(starts, 15, "moving to a new machine", false).await.expect("plan");
    let e = next_event(&mut bob_events, added(K::MaintenancePlanned)).await;
    let Event::AnnouncementAdded(a) = e else { unreachable!() };
    assert_eq!((a.starts_at_ms, a.minutes, a.text.as_str()), (starts, 15, "moving to a new machine"));
    next_event(&mut bob_events, |e| matches!(e, Event::AnnouncementsChanged(s) if s.maintenance.is_some())).await;
    let again = alice.plan_maintenance(starts, 15, "", true).await.expect_err("one at a time");
    assert!(matches!(&again, post_core::Error::Server(e) if e.code() == post_proto::ErrorCode::Conflict), "{again}");
    alice.end_maintenance().await.expect("cancel");
    next_event(&mut bob_events, added(K::MaintenanceCancelled)).await;
    next_event(&mut bob_events, |e| matches!(e, Event::AnnouncementsChanged(s) if s.maintenance.is_none())).await;

    // Kept in order, on every account; reading them follows the account.
    let kinds: Vec<K> = bob.announcements(50).expect("announcements").iter().map(|a| a.kind).collect();
    assert_eq!(kinds, [K::Text, K::Text, K::MaintenancePlanned, K::MaintenanceCancelled]);
    assert_eq!(alice.announcements(50).expect("announcements").len(), 4);
    assert_eq!(bob.announcement_summary().expect("summary").unread, 4);
    for l in listeners {
        l.abort();
    }
    let phone = link(&server, dir.path(), &bob, "phone").await;
    phone.sync().await.expect("phone sync");
    assert_eq!(phone.announcement_summary().expect("summary").unread, 4);
    bob.mark_announcements_read().await.expect("read");
    assert_eq!(bob.announcement_summary().expect("summary").unread, 0);
    phone.sync().await.expect("phone sync");
    assert_eq!(phone.announcement_summary().expect("summary").unread, 0, "read on the laptop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hosts_make_invites() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let refused = bob.create_invite(false, 7).await.expect_err("bob is no host");
    assert!(matches!(&refused, post_core::Error::Server(e) if e.code() == post_proto::ErrorCode::Forbidden), "{refused}");
    {
        let mut conn = post_server::db::open(&server.db, &server.key).expect("open server db");
        post_server::announce::add_host(&mut conn, "alice").expect("add host");
    }
    assert!(alice.create_invite(false, 31).await.is_err(), "at most 30 days");
    let invite = alice.create_invite(false, 3).await.expect("host makes an invite");
    assert_eq!(invite.server, server.url);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as u64;
    let three_days = 3 * 24 * 3600 * 1000;
    assert!(invite.expires_at_ms > now + three_days - 60_000 && invite.expires_at_ms <= now + three_days + 60_000);

    // Someone new signs up with it, once.
    let carol = Core::open(&dir.path().join("carol"), &DbKey::from_bytes([7; 32])).expect("open profile");
    carol.register(&invite.server, &invite.code, "carol", false).await.expect("register with the host's invite");
    let dave = Core::open(&dir.path().join("dave"), &DbKey::from_bytes([7; 32])).expect("open profile");
    assert!(dave.register(&invite.server, &invite.code, "dave", false).await.is_err(), "single use");
}

fn state_of(c: &Core, conv: post_core::ConversationId, id: post_core::MessageId) -> MessageState {
    c.message(conv, id).expect("message").state
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receipts_tick_sent_delivered_read() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let carol = server.client(dir.path(), "carol").await;

    // A DM: one tick, two once Bob's device has it, blue once he reads it.
    let dm = alice.start_dm("bob").await.expect("dm");
    let hi = alice.send_text(dm.id, "hi bob").await.expect("send");
    assert_eq!(hi.state, MessageState::Sent);
    alice.sync().await.expect("sync");
    assert_eq!(state_of(&alice, dm.id, hi.id), MessageState::Sent, "Bob has not fetched it");
    bob.sync().await.expect("bob sync");
    let mut alice_events = alice.subscribe();
    alice.sync().await.expect("sync");
    assert_eq!(state_of(&alice, dm.id, hi.id), MessageState::Delivered);
    next_event(&mut alice_events, |e| matches!(e, Event::MessageChanged(m) if m.id == hi.id && m.state == MessageState::Delivered)).await;
    bob.mark_read(dm.id).await.expect("read");
    alice.sync().await.expect("sync");
    assert_eq!(state_of(&alice, dm.id, hi.id), MessageState::Read);
    // Receipts are not messages: nothing new shows on either side.
    assert_eq!(bob.messages(dm.id, 50).expect("messages").len(), 1);
    assert_eq!(alice.messages(dm.id, 50).expect("messages").len(), 1);
    assert!(alice.messages(dm.id, 50).expect("messages").iter().all(|m| !m.unsupported));

    // Bob turns read receipts off: Alice sees two ticks, never blue; and
    // Bob no longer sees when Alice reads his.
    assert!(bob.read_receipts().expect("setting"));
    bob.set_read_receipts(false).await.expect("off");
    assert!(!bob.read_receipts().expect("setting"));
    let again = alice.send_text(dm.id, "still there?").await.expect("send");
    bob.sync().await.expect("bob sync");
    bob.mark_read(dm.id).await.expect("read");
    alice.sync().await.expect("sync");
    assert_eq!(state_of(&alice, dm.id, again.id), MessageState::Delivered, "delivered cannot be turned off; read can");
    let yes = bob.send_text(dm.id, "yes").await.expect("reply");
    alice.sync().await.expect("sync");
    alice.mark_read(dm.id).await.expect("alice reads");
    bob.sync().await.expect("bob sync");
    assert_eq!(state_of(&bob, dm.id, yes.id), MessageState::Delivered, "with his own off, Bob sees no blue ticks");
    // The setting follows Bob's account to a new device.
    let phone = link(&server, dir.path(), &bob, "phone").await;
    phone.sync().await.expect("phone sync");
    assert!(!phone.read_receipts().expect("setting"), "from the snapshot");
    bob.set_read_receipts(true).await.expect("on again");
    phone.sync().await.expect("phone sync");
    assert!(phone.read_receipts().expect("setting"));
    alice.sync().await.expect("sync");
    assert_eq!(state_of(&alice, dm.id, again.id), MessageState::Read, "turning it on says how far he has read");

    // A group: two ticks only once everyone has it, blue once everyone read.
    let group = alice.create_group("climbing", &["bob".into(), "carol".into()]).await.expect("group");
    let plan = alice.send_text(group.id, "saturday?").await.expect("send");
    bob.sync().await.expect("bob sync");
    phone.sync().await.expect("phone sync");
    alice.sync().await.expect("sync");
    assert_eq!(state_of(&alice, group.id, plan.id), MessageState::Sent, "Carol has not fetched it");
    let who = |r: &post_core::MessageReceipts| r.people.iter().map(|p| (p.username.clone(), p.state)).collect::<Vec<_>>();
    use post_core::ReceiptState as R;
    let r = alice.message_receipts(group.id, plan.id).await.expect("receipts");
    assert_eq!(who(&r), [("bob".to_string(), R::Delivered), ("carol".to_string(), R::Waiting)]);
    assert!(!r.too_large && !r.read_receipts_off);
    assert!(bob.message_receipts(group.id, plan.id).await.is_err(), "only for your own messages");
    carol.sync().await.expect("carol sync");
    alice.sync().await.expect("sync");
    assert_eq!(state_of(&alice, group.id, plan.id), MessageState::Delivered);
    bob.mark_read(group.id).await.expect("bob reads");
    alice.sync().await.expect("sync");
    assert_eq!(state_of(&alice, group.id, plan.id), MessageState::Delivered, "Carol has not read it");
    let r = alice.message_receipts(group.id, plan.id).await.expect("receipts");
    assert_eq!(who(&r), [("bob".to_string(), R::Read), ("carol".to_string(), R::Delivered)]);
    carol.mark_read(group.id).await.expect("carol reads");
    alice.sync().await.expect("sync");
    assert_eq!(state_of(&alice, group.id, plan.id), MessageState::Read);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn state_survives_restart() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let dm = alice.start_dm("bob").await.expect("dm");
    alice.send_text(dm.id, "one").await.expect("send");
    bob.sync().await.expect("sync");
    drop(bob);

    // Reopen Bob's profile from disk: same account, same conversation.
    let bob = Core::open(&dir.path().join("bob"), &DbKey::from_bytes([7; 32])).expect("reopen");
    assert_eq!(bob.identity().await.expect("identity").username, "bob");
    bob.connect().await.expect("reconnect");
    alice.send_text(dm.id, "two").await.expect("send");
    bob.sync().await.expect("sync");
    let bodies: Vec<_> = bob.messages(dm.id, 10).expect("messages").into_iter().filter_map(|m| m.body).collect();
    assert_eq!(bodies, vec!["one", "two"]);

    // The wrong key does not open it.
    assert!(Core::open(&dir.path().join("bob"), &DbKey::from_bytes([8; 32])).is_err());
}

/// Link a second device to `account`'s owner. Returns it once it has joined.
async fn link(server: &Server, dir: &Path, existing: &Core, name: &str) -> Core {
    let device = Core::open(&dir.join(name), &DbKey::from_bytes([7; 32])).expect("open profile");
    let code = device.begin_link(&server.url, name, false).await.expect("begin link");
    assert_eq!(existing.link_inspect(&code).await.expect("inspect").name, name);
    let finishing = {
        let device = device.clone();
        tokio::spawn(async move { device.finish_link().await })
    };
    let approved = existing.link_approve(&code).await.expect("approve");
    let id = finishing.await.expect("join").expect("finish link");
    assert_eq!(approved.device_id, id.device_id);
    device
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linked_device_joins_conversations() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    // Nothing in a normal linking should alarm anyone.
    let (mut alice_events, mut bob_events) = (alice.subscribe(), bob.subscribe());
    alice.set_profile(ProfileUpdate { display_name: Some("Alice A".into()), ..Default::default() }).await.expect("profile");
    let dm = alice.start_dm("bob").await.expect("dm");
    alice.send_text(dm.id, "before the phone").await.expect("send");
    bob.sync().await.expect("bob sync");
    alice.set_nickname("bob", "Bobby").await.expect("nickname");

    let phone = link(&server, dir.path(), &alice, "phone").await;
    let (laptop_id, phone_id) = (alice.identity().await.expect("id"), phone.identity().await.expect("id"));
    assert_eq!(phone_id.username, "alice");
    assert_eq!(phone_id.account_id, laptop_id.account_id, "one account");
    assert_ne!(phone_id.device_id, laptop_id.device_id, "two devices");

    // The phone is in the DM, without its history (that comes in phase 6).
    phone.sync().await.expect("phone sync");
    let convs = phone.conversations().expect("conversations");
    assert_eq!(convs.iter().map(|c| c.id).collect::<Vec<_>>(), vec![dm.id]);
    assert!(phone.messages(dm.id, 10).expect("messages").is_empty());
    // Its profile and Alice's private nicknames came from the self group.
    assert_eq!(phone.person("alice").await.expect("self").display_name, "Alice A");
    assert_eq!(phone.person("bob").await.expect("bob").nickname, "Bobby");

    // Nickname changes on either device reach the other; Bob never sees them.
    phone.set_nickname("bob", "B").await.expect("nickname on phone");
    alice.sync().await.expect("alice sync");
    assert_eq!(alice.person("bob").await.expect("bob").nickname, "B");
    alice.set_nickname("bob", "").await.expect("clear on laptop");
    phone.sync().await.expect("phone sync");
    assert_eq!(phone.person("bob").await.expect("bob").nickname, "");
    assert_eq!(phone.conversations().expect("conversations")[0].title, "bob");

    // Bob reaches both devices; each device sees what the other sends.
    bob.sync().await.expect("bob sync");
    bob.send_text(dm.id, "hi both").await.expect("send");
    phone.sync().await.expect("phone sync");
    alice.sync().await.expect("alice sync");
    let last = |c: &Core| c.messages(dm.id, 10).expect("messages").last().cloned().expect("a message");
    assert_eq!(last(&phone).body.as_deref(), Some("hi both"));
    assert_eq!(last(&alice).body.as_deref(), Some("hi both"));

    phone.send_text(dm.id, "from the phone").await.expect("phone sends");
    bob.sync().await.expect("bob sync");
    alice.sync().await.expect("alice sync");
    let at_bob = last(&bob);
    assert_eq!((at_bob.body.as_deref(), at_bob.sender_username.as_str()), (Some("from the phone"), "alice"));
    let at_laptop = last(&alice);
    assert_eq!(at_laptop.body.as_deref(), Some("from the phone"));
    assert!(at_laptop.outgoing, "a message from my other device is mine");

    // A conversation started after linking includes the phone from the start.
    let carol = server.client(dir.path(), "carol").await;
    let dm2 = carol.start_dm("alice").await.expect("carol dm");
    carol.send_text(dm2.id, "hello alice").await.expect("send");
    phone.sync().await.expect("phone sync");
    assert_eq!(last_in(&phone, dm2.id).as_deref(), Some("hello alice"));

    let mut bob_saw_device_added = false;
    for (who, rx) in [("alice", &mut alice_events), ("bob", &mut bob_events)] {
        while let Ok(e) = rx.try_recv() {
            assert!(!matches!(e, Event::Notice { .. }), "unexpected notice for {who}: {e:?}");
            if who == "bob" && e == (Event::DevicesChanged { username: "alice".into(), added: 1, removed: 0 }) {
                bob_saw_device_added = true;
            }
            // The laptop made the change itself, so it is not told about it.
            assert!(who == "bob" || !matches!(e, Event::DevicesChanged { .. }), "laptop told of its own change: {e:?}");
        }
    }
    assert!(bob_saw_device_added, "Bob is told Alice added a device");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verification_and_reading_follow_the_account() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let dm = bob.start_dm("alice").await.expect("dm");
    bob.send_text(dm.id, "hello").await.expect("send");
    alice.sync().await.expect("alice sync");

    // Verified before the phone existed: the phone learns it on joining.
    alice.set_verified("bob", true).await.expect("verify");
    assert!(alice.person("bob").await.expect("bob").verified);
    let phone = link(&server, dir.path(), &alice, "phone").await;
    phone.sync().await.expect("phone sync");
    assert!(phone.person("bob").await.expect("bob").verified, "from the snapshot");
    // It is private to Alice.
    bob.sync().await.expect("bob sync");
    assert!(!bob.person("alice").await.expect("alice").verified);

    // Reading on one device clears the count on the other.
    bob.send_text(dm.id, "one").await.expect("send");
    bob.send_text(dm.id, "two").await.expect("send");
    alice.sync().await.expect("alice sync");
    phone.sync().await.expect("phone sync");
    assert_eq!(phone.conversation(dm.id).expect("conversation").unread, 2);
    assert_eq!(alice.conversation(dm.id).expect("conversation").unread, 3, "hello was never read here");
    let mut alice_events = alice.subscribe();
    phone.mark_read(dm.id).await.expect("read on the phone");
    alice.sync().await.expect("alice sync");
    let e = next_event(&mut alice_events, |e| matches!(e, Event::ConversationChanged(c) if c.id == dm.id)).await;
    let Event::ConversationChanged(c) = e else { unreachable!() };
    assert_eq!(c.unread, 0);

    // Withdrawing on the laptop reaches the phone.
    alice.set_verified("bob", false).await.expect("unverify");
    phone.sync().await.expect("phone sync");
    assert!(!phone.person("bob").await.expect("bob").verified);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removed_device_is_cut_off() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let dm = alice.start_dm("bob").await.expect("dm");
    let phone = link(&server, dir.path(), &alice, "phone").await;
    phone.sync().await.expect("phone sync");
    bob.sync().await.expect("bob sync");

    // Both devices are listed, with the phone's name, on both devices.
    alice.rename_device(&alice.identity().await.expect("id").device_id, "laptop").await.expect("rename");
    phone.sync().await.expect("phone sync");
    async fn names(c: &Core) -> Vec<(String, bool)> {
        let mut v: Vec<(String, bool)> = c.devices().await.expect("devices").into_iter().map(|d| (d.name, d.this_device)).collect();
        v.sort();
        v
    }
    assert_eq!(names(&alice).await, vec![("laptop".into(), true), ("phone".into(), false)]);
    assert_eq!(names(&phone).await, vec![("laptop".into(), false), ("phone".into(), true)]);

    // A device cannot remove itself here, and an unknown prefix is refused.
    let phone_id = phone.identity().await.expect("id").device_id;
    assert!(phone.remove_device(&phone_id).await.is_err());
    assert!(alice.remove_device("zz").await.is_err());

    // The laptop removes the phone by a prefix of its ID.
    let mut bob_events = bob.subscribe();
    alice.remove_device(&phone_id[..8]).await.expect("remove");
    assert_eq!(names(&alice).await, vec![("laptop".into(), true)]);

    // The server refuses the phone at once.
    let reopened = Core::open(&dir.path().join("phone"), &DbKey::from_bytes([7; 32])).expect("reopen");
    drop(phone);
    assert!(reopened.connect().await.is_err(), "a removed device cannot log in");

    // Bob's client accepts the removal commit (it checks the removal against
    // Alice's chain), and the conversation goes on with the laptop.
    bob.send_text(dm.id, "just you now").await.expect("send");
    alice.sync().await.expect("alice sync");
    assert_eq!(last_in(&alice, dm.id).as_deref(), Some("just you now"));
    // Alice's next message carries her new chain head, so Bob fetches the
    // removal and is told.
    alice.send_text(dm.id, "yes").await.expect("reply");
    bob.sync().await.expect("bob sync");
    let mut told = false;
    while let Ok(e) = bob_events.try_recv() {
        assert!(!matches!(e, Event::Notice { .. }), "bob refused something: {e:?}");
        told |= e == (Event::DevicesChanged { username: "alice".into(), added: 0, removed: 1 });
    }
    assert!(told, "Bob is told Alice removed a device");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logging_out_leaves_the_account() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let dm = alice.start_dm("bob").await.expect("dm");
    let phone = link(&server, dir.path(), &alice, "phone").await;
    phone.sync().await.expect("phone sync");
    bob.sync().await.expect("bob sync");

    // The phone logs out while staying connected in the background, as the
    // app does: the connection loop ends with it.
    let running = tokio::spawn({
        let phone = phone.clone();
        async move { phone.run().await }
    });
    phone.send_text(dm.id, "bye from the phone").await.expect("send");
    phone.log_out().await.expect("log out");
    tokio::time::timeout(Duration::from_secs(5), running).await.expect("run stops on closing").expect("run");
    assert!(matches!(phone.connect().await, Err(post_core::Error::Closed)));

    // Alice's laptop sees one device left, and the conversation goes on
    // without the phone: Bob's client removes its leaf, justified by the
    // chain, and Alice accepts that.
    alice.sync().await.expect("alice sync");
    let devices = alice.devices().await.expect("devices");
    assert_eq!(devices.len(), 1);
    assert!(devices[0].this_device);
    bob.sync().await.expect("bob sync");
    assert_eq!(last_in(&bob, dm.id).as_deref(), Some("bye from the phone"), "queued before leaving, so it was sent");
    bob.send_text(dm.id, "see you").await.expect("send");
    alice.sync().await.expect("alice sync");
    assert_eq!(last_in(&alice, dm.id).as_deref(), Some("see you"));

    // The laptop is the last device: logging out there would lose the
    // account, so it is refused and nothing changes.
    assert!(matches!(alice.log_out().await, Err(post_core::Error::InvalidInput(_))));
    alice.send_text(dm.id, "still here").await.expect("still works");
    bob.sync().await.expect("bob sync");
    assert_eq!(last_in(&bob, dm.id).as_deref(), Some("still here"));

    // Deleting the profile removes Post's files and the folder.
    let phone_dir = dir.path().join("phone");
    assert!(phone_dir.join("profile.db").exists());
    drop(phone);
    post_core::delete_profile(&phone_dir).expect("delete");
    assert!(!phone_dir.exists());
    // In a folder holding other things too, those stay.
    let shared = dir.path().join("shared");
    drop(Core::open(&shared, &DbKey::from_bytes([9; 32])).expect("open"));
    std::fs::write(shared.join("notes.txt"), "mine").expect("write");
    post_core::delete_profile(&shared).expect("delete");
    assert!(!shared.join("profile.db").exists());
    assert_eq!(std::fs::read_to_string(shared.join("notes.txt")).expect("kept"), "mine");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invited_by_a_device_that_then_left() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let phone = link(&server, dir.path(), &alice, "phone").await;

    // Bob starts a DM; the laptop adds the phone to it. That invitation
    // lists the laptop, which logs out before the phone has fetched it.
    let dm = bob.start_dm("alice").await.expect("dm");
    bob.send_text(dm.id, "hello").await.expect("send");
    alice.sync().await.expect("alice sync");
    alice.log_out().await.expect("log out");
    let mut phone_events = phone.subscribe();
    phone.sync().await.expect("phone sync");
    while let Ok(e) = phone_events.try_recv() {
        assert!(!matches!(e, Event::Notice { .. }), "the phone refused something: {e:?}");
    }
    assert!(phone.conversation(dm.id).is_ok(), "the phone joined the DM");

    // The conversation goes on between Bob and the phone, with the
    // laptop's leaf gone.
    bob.send_text(dm.id, "for the phone").await.expect("send");
    phone.sync().await.expect("phone sync");
    assert_eq!(last_in(&phone, dm.id).as_deref(), Some("for the phone"));
    phone.send_text(dm.id, "got it").await.expect("reply");
    bob.sync().await.expect("bob sync");
    assert_eq!(last_in(&bob, dm.id).as_deref(), Some("got it"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_chat() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let carol = server.client(dir.path(), "carol").await;
    let bob_phone = link(&server, dir.path(), &bob, "bob-phone").await;
    for (c, name) in [(&alice, "Alice A"), (&bob, "Bob B"), (&carol, "Carol C")] {
        c.set_profile(ProfileUpdate { display_name: Some(name.into()), ..Default::default() }).await.expect("profile");
    }

    // Names are checked, and a group needs someone else in it.
    assert!(alice.create_group("", &["bob".into()]).await.is_err());
    assert!(alice.create_group("solo", &["alice".into()]).await.is_err());
    assert!(alice.create_group("x", &["nobody".into()]).await.is_err());

    let g = alice.create_group("climbing", &["bob".into(), "carol".into(), "bob".into()]).await.expect("create");
    assert_eq!(g.title, "climbing");
    alice.send_text(g.id, "saturday?").await.expect("send");
    for c in [&bob, &bob_phone, &carol] {
        c.sync().await.expect("sync");
        assert!(c.conversations().expect("conversations").iter().any(|x| x.id == g.id && x.title == "climbing"));
        assert_eq!(last_in(c, g.id).as_deref(), Some("saturday?"));
    }

    // Everyone learned everyone's profile from joining, even though Bob and
    // Carol never shared a conversation before.
    carol.send_text(g.id, "in").await.expect("send");
    for c in [&alice, &bob, &bob_phone] {
        c.sync().await.expect("sync");
    }
    bob.sync().await.expect("sync");
    let msgs = bob.messages(g.id, 10).expect("messages");
    assert_eq!(msgs.last().map(|m| (m.sender.as_str(), m.body.as_deref())), Some(("Carol C", Some("in"))));
    carol.sync().await.expect("sync");
    assert_eq!(carol.person("bob").await.expect("bob").display_name, "Bob B");

    // Members: Alice is admin; Bob is there with both devices.
    let members = carol.members(g.id).await.expect("members");
    let summary: Vec<(String, bool, u32, bool)> =
        members.iter().map(|m| (m.username.clone(), m.admin, m.devices, m.you)).collect();
    assert_eq!(
        summary,
        vec![
            ("carol".into(), false, 1, true),
            ("alice".into(), true, 1, false),
            ("bob".into(), false, 2, false),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_admins() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let carol = server.client(dir.path(), "carol").await;
    let g = alice.create_group("book club", &["bob".into()]).await.expect("create");
    bob.sync().await.expect("sync");
    let title = |c: &Core| c.conversations().expect("conversations").into_iter().find(|x| x.id == g.id).map(|x| x.title);
    let admins = |ms: Vec<post_core::Member>| ms.into_iter().filter(|m| m.admin).map(|m| m.username).collect::<Vec<_>>();

    // Only admins change the group.
    assert!(bob.rename_group(g.id, "hijacked").await.is_err());
    assert!(bob.add_members(g.id, &["carol".into()]).await.is_err());

    // The admin adds Carol, who sees what is sent from then on.
    alice.add_members(g.id, &["carol".into()]).await.expect("add carol");
    alice.send_text(g.id, "welcome carol").await.expect("send");
    carol.sync().await.expect("carol sync");
    assert_eq!(last_in(&carol, g.id).as_deref(), Some("welcome carol"));
    assert_eq!(carol.members(g.id).await.expect("members").len(), 3);

    // Renaming reaches everyone.
    alice.rename_group(g.id, "book club (fiction)").await.expect("rename");
    bob.sync().await.expect("sync");
    carol.sync().await.expect("sync");
    for c in [&alice, &bob, &carol] {
        assert_eq!(title(c).as_deref(), Some("book club (fiction)"));
    }

    // Alice hands the group to Bob. The last admin cannot step down.
    alice.set_admin(g.id, "bob", true).await.expect("promote bob");
    alice.set_admin(g.id, "alice", false).await.expect("alice steps down");
    bob.sync().await.expect("sync");
    assert_eq!(admins(bob.members(g.id).await.expect("members")), vec!["bob"]);
    assert!(bob.set_admin(g.id, "bob", false).await.is_err(), "a group keeps an admin");
    assert!(alice.rename_group(g.id, "mine again").await.is_err());

    // Bob removes Carol: she keeps what she had but gets nothing new.
    bob.remove_members(g.id, &["carol".into()]).await.expect("remove carol");
    bob.send_text(g.id, "just us").await.expect("send");
    carol.sync().await.expect("carol sync");
    let at_carol = carol.conversations().expect("conversations").into_iter().find(|x| x.id == g.id).expect("still listed");
    assert!(!at_carol.following, "carol was removed");
    assert_eq!(last_in(&carol, g.id).as_deref(), Some("welcome carol"));
    alice.sync().await.expect("alice sync");
    assert_eq!(last_in(&alice, g.id).as_deref(), Some("just us"));
    assert_eq!(alice.members(g.id).await.expect("members").len(), 2);

    // The history setting is the admin's too.
    assert!(alice.set_history(g.id, false, 0).await.is_err());
    bob.set_history(g.id, true, 30).await.expect("history");

    // Carol can be added back, and her device rejoins the same conversation.
    bob.add_members(g.id, &["carol".into()]).await.expect("add carol back");
    bob.send_text(g.id, "welcome back").await.expect("send");
    carol.sync().await.expect("carol sync");
    let back = carol.conversations().expect("conversations").into_iter().find(|x| x.id == g.id).expect("listed");
    assert!(back.following, "carol is in again");
    assert_eq!(last_in(&carol, g.id).as_deref(), Some("welcome back"));
    carol.send_text(g.id, "thanks").await.expect("carol sends");
    alice.sync().await.expect("alice sync");
    assert_eq!(last_in(&alice, g.id).as_deref(), Some("thanks"));

    // Removed and added back while she was not looking: on her next sync she
    // sees the removal, then takes the new Welcome.
    bob.remove_members(g.id, &["carol".into()]).await.expect("remove again");
    bob.add_members(g.id, &["carol".into()]).await.expect("add again");
    bob.send_text(g.id, "again").await.expect("send");
    carol.sync().await.expect("carol sync");
    assert!(carol.conversation(g.id).expect("conversation").following);
    assert_eq!(last_in(&carol, g.id).as_deref(), Some("again"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leaving_a_group() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let carol = server.client(dir.path(), "carol").await;
    let bob_phone = link(&server, dir.path(), &bob, "bob-phone").await;
    let g = alice.create_group("trip", &["bob".into(), "carol".into()]).await.expect("create");
    for c in [&bob, &bob_phone, &carol] {
        c.sync().await.expect("sync");
    }
    let following = |c: &Core| c.conversations().expect("conversations").into_iter().find(|x| x.id == g.id).map(|x| x.following);
    let usernames = |ms: Vec<post_core::Member>| ms.into_iter().map(|m| m.username).collect::<Vec<_>>();

    // DMs cannot be left; groups can.
    let dm = alice.start_dm("carol").await.expect("dm");
    assert!(alice.leave_group(dm.id).await.is_err());

    // Bob leaves from his laptop: it shows as left at once, and he cannot send.
    bob.leave_group(g.id).await.expect("leave");
    assert_eq!(following(&bob), Some(false));
    assert!(bob.send_text(g.id, "wait").await.is_err());

    // The next member to sync commits his removal, with both his devices.
    alice.sync().await.expect("alice sync");
    assert_eq!(usernames(alice.members(g.id).await.expect("members")), vec!["alice", "carol"]);
    alice.send_text(g.id, "bob left").await.expect("send");
    for c in [&bob, &bob_phone] {
        c.sync().await.expect("sync");
        assert_eq!(following(c), Some(false));
        assert_ne!(last_in(c, g.id).as_deref(), Some("bob left"));
    }
    carol.sync().await.expect("carol sync");
    assert_eq!(last_in(&carol, g.id).as_deref(), Some("bob left"));

    // The only admin leaving hands the group to the longest-standing member.
    alice.leave_group(g.id).await.expect("alice leaves");
    carol.sync().await.expect("carol sync");
    let members = carol.members(g.id).await.expect("members");
    assert_eq!(members.iter().map(|m| (m.username.as_str(), m.admin)).collect::<Vec<_>>(), vec![("carol", true)]);
    carol.rename_group(g.id, "carol's trip").await.expect("carol can manage it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn key_refresh() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let mut bob_events = bob.subscribe();
    let dm = alice.start_dm("bob").await.expect("dm");
    let g = alice.create_group("pair", &["bob".into()]).await.expect("group");
    bob.sync().await.expect("sync");

    // Both refresh at once: one commit per epoch wins, the other retries.
    // Four groups (two self groups, the DM, the group); the DM and group get
    // a refresh from each side, and each self group one.
    let before = server.total_epochs();
    let (a, b) = tokio::join!(alice.refresh_keys(), bob.refresh_keys());
    a.expect("alice refresh");
    b.expect("bob refresh");
    alice.sync().await.expect("sync");
    bob.sync().await.expect("sync");
    assert_eq!(server.total_epochs() - before, 6, "every leaf got fresh keys");

    // Done for the week: syncing again commits nothing more.
    alice.sync().await.expect("sync");
    bob.sync().await.expect("sync");
    assert_eq!(server.total_epochs() - before, 6);

    for conv in [dm.id, g.id] {
        alice.send_text(conv, "after refresh").await.expect("send");
        bob.sync().await.expect("sync");
        assert_eq!(last_in(&bob, conv).as_deref(), Some("after refresh"));
        bob.send_text(conv, "still here").await.expect("send");
        alice.sync().await.expect("sync");
        assert_eq!(last_in(&alice, conv).as_deref(), Some("still here"));
    }
    while let Ok(e) = bob_events.try_recv() {
        assert!(!matches!(e, Event::Notice { .. }), "unexpected notice: {e:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typing_and_presence() {
    use post_core::{Presence, PresenceSetting};
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    alice
        .set_profile(ProfileUpdate {
            display_name: Some("Alice A".into()),
            typing_template: Some("the one who types is $$NAME$$".into()),
            ..Default::default()
        })
        .await
        .expect("profile");
    let dm = alice.start_dm("bob").await.expect("dm");
    bob.sync().await.expect("sync");
    bob.set_nickname("alice", "Ally").await.expect("nickname");

    let mut bob_events = bob.subscribe();
    let listener = {
        let bob = bob.clone();
        tokio::spawn(async move { bob.listen().await })
    };
    // Let Bob's listener subscribe before anything is relayed.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let entries = server.log_entries();

    // Typing arrives with Alice's template and the name Bob sees for her.
    alice.send_typing(dm.id).await.expect("typing");
    let e = next_event(&mut bob_events, |e| matches!(e, Event::Typing { .. })).await;
    assert_eq!(
        e,
        Event::Typing { conversation: dm.id, username: "alice".into(), text: "the one who types is Ally".into() }
    );

    // Presence changes arrive, and are remembered in memory.
    alice.set_presence(PresenceSetting::DoNotDisturb).await.expect("presence");
    let e = next_event(&mut bob_events, |e| matches!(e, Event::Presence { .. })).await;
    assert_eq!(e, Event::Presence { username: "alice".into(), presence: Presence::DoNotDisturb });
    assert_eq!(bob.presence("alice").await.expect("presence"), Presence::DoNotDisturb);
    assert_eq!(alice.own_presence().await.expect("own"), PresenceSetting::DoNotDisturb);

    // Going invisible says "offline" once and then nothing.
    alice.set_presence(PresenceSetting::Invisible).await.expect("invisible");
    let e = next_event(&mut bob_events, |e| matches!(e, Event::Presence { .. })).await;
    assert_eq!(e, Event::Presence { username: "alice".into(), presence: Presence::Offline });

    // None of it was stored on the server.
    assert_eq!(server.log_entries(), entries, "signals never enter a group log");
    listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_devices_leave_and_return() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let alice = server.client(dir.path(), "alice").await;
    let bob = server.client(dir.path(), "bob").await;
    let phone = link(&server, dir.path(), &bob, "bob-phone").await;
    let dm = alice.start_dm("bob").await.expect("dm");
    let g = alice.create_group("climbing", &["bob".into()]).await.expect("group");
    phone.sync().await.expect("phone sync");
    bob.sync().await.expect("bob sync");
    let phone_id = phone.identity().await.expect("id").device_id;
    drop(phone);
    let bob_devices = |c: &Core, conv: post_core::ConversationId| {
        let c = c.clone();
        async move { c.members(conv).await.expect("members").into_iter().find(|m| m.username == "bob").map(|m| m.devices) }
    };
    assert_eq!(bob_devices(&alice, dm.id).await, Some(2));
    assert_eq!(bob_devices(&alice, g.id).await, Some(2));

    // The phone has not been seen for 31 days. In the group, Alice is admin
    // and removes its leaf; in the DM, only Bob's own laptop may.
    server.age_device(&phone_id, 31);
    alice.sync().await.expect("alice sync");
    assert_eq!(bob_devices(&alice, g.id).await, Some(1), "the admin removed the stale phone");
    assert_eq!(bob_devices(&alice, dm.id).await, Some(2), "a DM partner does not");
    let mut bob_events = bob.subscribe();
    bob.sync().await.expect("bob sync");
    alice.sync().await.expect("alice sync");
    assert_eq!(bob_devices(&alice, dm.id).await, Some(1), "Bob's laptop removed it from the DM");
    while let Ok(e) = bob_events.try_recv() {
        assert!(!matches!(e, Event::Notice { .. }), "bob refused something: {e:?}");
    }

    // An account's last device is never removed: Bob's laptop going stale
    // too leaves him in the group.
    let laptop_id = bob.identity().await.expect("id").device_id;
    server.age_device(&laptop_id, 31);
    alice.sync().await.expect("alice sync");
    assert_eq!(bob_devices(&alice, g.id).await, Some(1));

    // Both of Bob's devices come back; logging in clears a device's flag.
    // The laptop adds the phone back to both conversations, and the phone,
    // having first seen its removals, rejoins them.
    drop(bob);
    let bob = Core::open(&dir.path().join("bob"), &DbKey::from_bytes([7; 32])).expect("reopen laptop");
    bob.connect().await.expect("laptop logs in");
    let phone = Core::open(&dir.path().join("bob-phone"), &DbKey::from_bytes([7; 32])).expect("reopen");
    let mut phone_events = phone.subscribe();
    phone.connect().await.expect("phone logs in");
    bob.sync().await.expect("bob sync");
    phone.sync().await.expect("phone sync");
    while let Ok(e) = phone_events.try_recv() {
        assert!(!matches!(e, Event::Notice { .. }), "the phone refused something: {e:?}");
    }
    alice.sync().await.expect("alice sync");
    for conv in [dm.id, g.id] {
        assert_eq!(bob_devices(&alice, conv).await, Some(2), "the phone is back");
        alice.send_text(conv, "welcome back phone").await.expect("send");
        phone.sync().await.expect("phone sync");
        assert_eq!(last_in(&phone, conv).as_deref(), Some("welcome back phone"));
    }
}

fn last_in(c: &Core, conv: post_core::ConversationId) -> Option<String> {
    c.messages(conv, 10).expect("messages").last().and_then(|m| m.body.clone())
}

#[tokio::test]
async fn app_settings_persist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("p");
    let core = Core::open(&path, &DbKey::from_bytes([7; 32])).expect("open");
    assert_eq!(core.setting("spellcheck.language").expect("read"), None);
    core.set_setting("spellcheck.language", Some("en_GB")).await.expect("set");
    core.set_setting("spellcheck.words", Some("post\nopenmls")).await.expect("set");
    core.set_setting("spellcheck.words", None).await.expect("clear");
    assert!(core.set_setting("Not Valid", Some("x")).await.is_err(), "names are checked");
    assert!(core.setting("../meta").is_err());
    drop(core);
    let core = Core::open(&path, &DbKey::from_bytes([7; 32])).expect("reopen");
    assert_eq!(core.setting("spellcheck.language").expect("read").as_deref(), Some("en_GB"));
    assert_eq!(core.setting("spellcheck.words").expect("read"), None);
}

#[cfg(unix)]
#[test]
fn profiles_are_readable_by_their_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().expect("tempdir");
    let mode = |p: &Path| std::fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
    let profile = dir.path().join("fresh");
    drop(Core::open(&profile, &DbKey::from_bytes([1; 32])).expect("open"));
    assert_eq!(mode(&profile), 0o700);
    assert_eq!(mode(&profile.join("profile.db")), 0o600);

    // A profile made before this rule is tightened when next opened.
    std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    std::fs::set_permissions(profile.join("profile.db"), std::fs::Permissions::from_mode(0o644)).expect("chmod");
    drop(Core::open(&profile, &DbKey::from_bytes([1; 32])).expect("reopen"));
    assert_eq!(mode(&profile), 0o700);
    assert_eq!(mode(&profile.join("profile.db")), 0o600);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registration_rules() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let invite = server.invite(false);
    let a = Core::open(&dir.path().join("a"), &DbKey::from_bytes([1; 32])).expect("open");
    a.register(&server.url, &invite, "alice", false).await.expect("register");

    // An invite works once.
    let b = Core::open(&dir.path().join("b"), &DbKey::from_bytes([1; 32])).expect("open");
    assert!(b.register(&server.url, &invite, "bob", false).await.is_err());
    // A username is taken once.
    assert!(b.register(&server.url, &server.invite(false), "alice", false).await.is_err());
    // A bot invite only registers a bot account, and the reverse.
    assert!(b.register(&server.url, &server.invite(true), "bob", false).await.is_err());
    b.register(&server.url, &server.invite(true), "weatherbot", true).await.expect("bot registers");
    assert!(b.identity().await.expect("identity").bot);
}
