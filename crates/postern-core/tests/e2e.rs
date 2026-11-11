//! End to end: a real server and real clients in one process (design section
//! 15). Phase 2's done-when: two devices exchange, edit and delete DMs and see
//! each other's profiles.

use std::path::Path;
use std::time::Duration;

use postern_core::{Core, DbKey, Event, MessageState, ProfileUpdate};
use tempfile::TempDir;
use zeroize::Zeroizing;

struct Server {
    _running: postern_server::Running,
    url: String,
    db: std::path::PathBuf,
    key: [u8; 32],
    _dir: TempDir,
}

impl Server {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("server.db");
        let key = [42u8; 32];
        let config = postern_server::Config {
            server_domain: format!("127.0.0.1:{}", addr.port()),
            db_path: db.clone(),
            db_key: Zeroizing::new(key),
        };
        let running = postern_server::spawn(listener, config).expect("server starts");
        Self { url: running.url(), _running: running, db, key, _dir: dir }
    }

    fn invite(&self, bot: bool) -> String {
        let mut conn = postern_server::db::open(&self.db, &self.key).expect("open server db");
        postern_server::db::create_invite(&mut conn, bot, 3_600_000).expect("invite")
    }

    /// The sum of every group's epoch: each commit anywhere adds one.
    fn total_epochs(&self) -> i64 {
        let conn = postern_server::db::open(&self.db, &self.key).expect("open server db");
        conn.query_row("SELECT COALESCE(SUM(epoch), 0) FROM groups", [], |r| r.get(0)).expect("epochs")
    }

    /// Entries in every group log on the server.
    fn log_entries(&self) -> i64 {
        let conn = postern_server::db::open(&self.db, &self.key).expect("open server db");
        conn.query_row("SELECT COUNT(*) FROM group_log", [], |r| r.get(0)).expect("count")
    }

    /// Make a device look unseen for `days`, and run the server's hourly
    /// stale-device check.
    fn age_device(&self, device_hex: &str, days: i64) {
        let mut conn = postern_server::db::open(&self.db, &self.key).expect("open server db");
        let id = hex::decode(device_hex).expect("hex");
        conn.execute("UPDATE devices SET last_seen_day = last_seen_day - ?1 WHERE device_id = ?2", (days, id)).expect("age");
        postern_server::db::mark_stale_devices(&mut conn).expect("mark stale");
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
        })
        .await
        .expect("profile");

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
    let admins = |ms: Vec<postern_core::Member>| ms.into_iter().filter(|m| m.admin).map(|m| m.username).collect::<Vec<_>>();

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
    let usernames = |ms: Vec<postern_core::Member>| ms.into_iter().map(|m| m.username).collect::<Vec<_>>();

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
    use postern_core::{Presence, PresenceSetting};
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
    let bob_devices = |c: &Core, conv: postern_core::ConversationId| {
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

fn last_in(c: &Core, conv: postern_core::ConversationId) -> Option<String> {
    c.messages(conv, 10).expect("messages").last().and_then(|m| m.body.clone())
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
