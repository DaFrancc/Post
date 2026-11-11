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
