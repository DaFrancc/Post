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
