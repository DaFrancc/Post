//! Messages survive what goes wrong around them: the server killed or
//! stopped mid-write (a crash, a restart, an update, a machine shutting
//! down), the network cutting out, and apps killed mid-send and mid-receive.
//! Every message an app has written must arrive exactly once, in order, and
//! group changes made meanwhile must leave every member in the same group.
//! Databases must come through intact.
//!
//! Power cuts are the server killed without warning plus the disk losing
//! what was not flushed. Both databases run with `synchronous=FULL`, so a
//! write is flushed before anyone is told it happened; the kills below test
//! everything above that.
//!
//! Calls are left out: they are live streams, with nothing to keep.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use post_core::{ConversationId, Core, DbKey};
use tokio::net::{TcpListener, TcpStream};
use tracing::Instrument;
use zeroize::Zeroizing;

const CLIENT_KEY: [u8; 32] = [7; 32];

// ------------------------------------------------------------ helpers

/// A small random source, seeded from the clock and printed, so a failing
/// run can be repeated with POST_DURABILITY_SEED.
struct Rng(AtomicU64);

impl Rng {
    fn new(test: &str) -> Arc<Self> {
        let seed = std::env::var("POST_DURABILITY_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos() as u64 | 1);
        eprintln!("{test}: seed {seed} (POST_DURABILITY_SEED={seed} repeats it)");
        Arc::new(Self(AtomicU64::new(seed)))
    }

    /// A number in `lo..hi`.
    fn range(&self, lo: u64, hi: u64) -> u64 {
        let mut x = self.0.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0.store(x, Ordering::Relaxed);
        lo + x % (hi - lo)
    }

    fn millis(&self, lo: u64, hi: u64) -> Duration {
        Duration::from_millis(self.range(lo, hi))
    }

    fn chance(&self, percent: u64) -> bool {
        self.range(0, 100) < percent
    }
}

/// One of the workspace's programs, built if this test run has not yet.
/// Test binaries live in target/<profile>/deps, the programs a level up.
fn program(package: &str, name: &str) -> PathBuf {
    static BUILD: Mutex<()> = Mutex::new(());
    let dir = std::env::current_exe().expect("test binary").parent().and_then(Path::parent).expect("target dir").to_path_buf();
    let path = dir.join(name);
    let _once = BUILD.lock().unwrap_or_else(|p| p.into_inner());
    if !path.exists() {
        let mut cargo = Command::new(env!("CARGO"));
        cargo.args(["build", "-q", "-p", package, "--bin", name]);
        if dir.file_name().is_some_and(|d| d == "release") {
            cargo.arg("--release");
        }
        assert!(cargo.status().expect("cargo").success(), "building {name}");
    }
    path
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port()
}

/// Wait for `cond`, up to `limit`.
async fn until(limit: Duration, mut cond: impl AsyncFnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if cond().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// The bodies in a conversation starting with `prefix`, oldest first.
fn bodies(c: &Core, conv: ConversationId, prefix: &str) -> Vec<String> {
    c.messages(conv, 10_000).expect("messages").into_iter().filter_map(|m| m.body).filter(|b| b.starts_with(prefix)).collect()
}

/// Every message in `want` arrives exactly once and in order. On failure,
/// how everyone stands, to find where it stuck.
async fn arrives_exactly_once(reader: &Core, conv: ConversationId, prefix: &str, want: &[String], who: &str, everyone: &[(&Core, &str)]) {
    let arrived = until(Duration::from_secs(90), async || {
        let _ = reader.sync().instrument(tracing::info_span!("app", name = who)).await;
        bodies(reader, conv, prefix).len() >= want.len()
    })
    .await;
    let got = bodies(reader, conv, prefix);
    if !arrived || got != want {
        for (c, name) in everyone {
            let msgs = c.messages(conv, 10_000).expect("messages");
            let states = msgs.iter().filter(|m| m.outgoing).fold(std::collections::BTreeMap::<String, usize>::new(), |mut m, x| {
                *m.entry(format!("{:?}", x.state)).or_default() += 1;
                m
            });
            let last = msgs.iter().rev().find_map(|m| m.body.clone()).unwrap_or_default();
            eprintln!("DIAG {name}: connected={} own messages by state {states:?}; last body {last:?}", c.is_connected().await);
            for n in c.notices(None, 50).unwrap_or_default() {
                eprintln!("DIAG {name} notice: {}", n.text);
            }
        }
        // The server's log of the conversation, each entry named by whose
        // message it is.
        let server = SERVER_DB.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some((db, key)) = server {
            let mut names = std::collections::HashMap::new();
            for (c, name) in everyone {
                for m in c.messages(conv, 10_000).expect("messages") {
                    if m.outgoing {
                        names.insert(m.id.to_string(), format!("{name}: {}", m.body.unwrap_or_default()));
                    }
                }
            }
            let conn = post_server::db::open(&db, &key).expect("server db");
            let gid = hex::decode(conv.to_string()).expect("hex");
            let mut stmt = conn.prepare("SELECT seq, epoch, kind, client_msg_id FROM group_log WHERE group_id = ?1 ORDER BY seq").expect("prepare");
            let rows = stmt
                .query_map([gid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, Vec<u8>>(3)?)))
                .expect("query");
            for row in rows {
                let (seq, epoch, kind, id) = row.expect("row");
                let id = hex::encode(id);
                eprintln!("LOG seq {seq:4} epoch {epoch:3} kind {kind} {}", names.get(&id).cloned().unwrap_or_else(|| format!("? {id}")));
            }
        }
    }
    assert!(arrived, "{who}: only {} of {} of {prefix}'s messages arrived", got.len(), want.len());
    assert_eq!(got, want, "{who}: {prefix}'s messages, each once and in order");
}

/// The server database a failing check dumps the log of.
static SERVER_DB: Mutex<Option<(PathBuf, [u8; 32])>> = Mutex::new(None);

/// The core's own log, with RUST_LOG (post_core=info, say).
fn log() {
    let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_test_writer().try_init();
}

/// SQLite's own check of a database: every page and index consistent.
fn intact(path: &Path, key: &[u8; 32]) {
    let conn = rusqlite::Connection::open(path).expect("open");
    conn.pragma_update(None, "key", format!("x'{}'", hex::encode(key))).expect("key");
    let result: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).expect("check");
    assert_eq!(result, "ok", "{} is damaged", path.display());
}

// ------------------------------------------------------------ a real postd

/// postd as its own process, so it can be killed like a crash or stopped
/// like a restart, on the same database each time.
struct Postd {
    child: Option<Child>,
    port: u16,
    db: PathBuf,
    key: PathBuf,
    releases: PathBuf,
}

impl Postd {
    fn new(dir: &Path) -> Self {
        let key = dir.join("server.key");
        let ok = Command::new(program("post-server", "postd")).arg("keygen").arg(&key).status().expect("keygen");
        assert!(ok.success());
        let mut p = Self { child: None, port: free_port(), db: dir.join("server.db"), key, releases: dir.join("releases") };
        p.start();
        p
    }

    fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/v1/ws", self.port)
    }

    fn command(&self) -> Command {
        let mut c = Command::new(program("post-server", "postd"));
        c.env("POST_DB", &self.db).env("POST_DB_KEY_FILE", &self.key).env("RUST_LOG", "warn");
        c
    }

    fn start(&mut self) {
        let addr = format!("127.0.0.1:{}", self.port);
        let child = self
            .command()
            .arg("serve")
            .env("POST_DOMAIN", &addr)
            .env("POST_LISTEN", &addr)
            .env("POST_RELEASES_DIR", &self.releases)
            .env("POST_RELEASES_FROM", "")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("postd");
        self.child = Some(child);
        let start = Instant::now();
        while std::net::TcpStream::connect(&addr).is_err() {
            assert!(start.elapsed() < Duration::from_secs(10), "postd did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Stopped like a restart (SIGTERM), or killed like a crash (SIGKILL).
    fn stop(&mut self, crash: bool) {
        let Some(mut child) = self.child.take() else { return };
        if crash {
            let _ = child.kill();
        } else {
            let _ = Command::new("kill").arg("-TERM").arg(child.id().to_string()).status();
        }
        let _ = child.wait();
    }

    fn invite(&self) -> String {
        let out = self.command().arg("invite").output().expect("invite");
        assert!(out.status.success(), "postd invite: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("utf8").trim().to_owned()
    }

    fn key_bytes(&self) -> [u8; 32] {
        let hex = std::fs::read_to_string(&self.key).expect("key");
        let mut k = [0u8; 32];
        hex::decode_to_slice(hex.trim(), &mut k).expect("hex key");
        k
    }
}

impl Drop for Postd {
    fn drop(&mut self) {
        self.stop(true);
    }
}

async fn account(dir: &Path, url: &str, invite: &str, name: &str) -> Core {
    let core = Core::open(&dir.join(name), &DbKey::from_bytes(CLIENT_KEY)).expect("open profile");
    core.register(url, invite, name, false).await.expect("register");
    core
}

/// Send `count` messages, a little apart, each written to the outbox before
/// it returns: what the app shows as sent or sending.
fn sender(core: Core, conv: ConversationId, prefix: &'static str, count: usize, rng: Arc<Rng>) -> tokio::task::JoinHandle<Vec<String>> {
    tokio::spawn(async move {
        let mut sent = Vec::new();
        for i in 0..count {
            let text = format!("{prefix} {i:03}");
            let name = if prefix == "dm" { "alice" } else { prefix };
            core.send_text(conv, &text).instrument(tracing::info_span!("app", name)).await.unwrap_or_else(|e| panic!("{text}: {e}"));
            sent.push(text);
            tokio::time::sleep(rng.millis(20, 100)).await;
        }
        sent
    })
}

// ------------------------------------------------------------ the server stops

/// postd killed (a crash, or a power cut above the disk) and stopped (a
/// restart, an update, the machine shutting down) again and again, while
/// three people write in a group and a DM, and the group is renamed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_survive_the_server_stopping() {
    log();
    let rng = Rng::new("messages_survive_the_server_stopping");
    let dir = tempfile::tempdir().expect("tempdir");
    let server = Arc::new(Mutex::new(Postd::new(dir.path())));
    let (url, invites) = {
        let s = server.lock().expect("lock");
        *SERVER_DB.lock().expect("lock") = Some((s.db.clone(), s.key_bytes()));
        (s.url(), [s.invite(), s.invite(), s.invite()])
    };
    let alice = account(dir.path(), &url, &invites[0], "alice").await;
    let bob = account(dir.path(), &url, &invites[1], "bob").await;
    let carol = account(dir.path(), &url, &invites[2], "carol").await;
    let group = alice.create_group("climbing", &["bob".into(), "carol".into()]).await.expect("group").id;
    let dm = alice.start_dm("bob").await.expect("dm").id;
    for c in [&bob, &carol] {
        c.sync().await.expect("sync");
    }
    // Staying connected as the app does: reconnecting when the server is back.
    let running: Vec<_> = [(&alice, "alice"), (&bob, "bob"), (&carol, "carol")]
        .into_iter()
        .map(|(c, name)| {
            let c = c.clone();
            tokio::spawn(async move { c.run().await }.instrument(tracing::info_span!("app", name)))
        })
        .collect();

    let done = Arc::new(AtomicBool::new(false));
    let stops = {
        let (server, done, rng) = (server.clone(), done.clone(), rng.clone());
        tokio::spawn(async move {
            let mut n = 0;
            while !done.load(Ordering::Relaxed) {
                tokio::time::sleep(rng.millis(150, 700)).await;
                let crash = rng.chance(70);
                server.lock().expect("lock").stop(crash);
                tokio::time::sleep(rng.millis(30, 400)).await;
                server.lock().expect("lock").start();
                n += 1;
            }
            n
        })
    };
    let senders =
        [sender(alice.clone(), group, "alice", 100, rng.clone()), sender(bob.clone(), group, "bob", 100, rng.clone()), sender(alice.clone(), dm, "dm", 50, rng.clone())];
    // Group changes through the stops too: each rename is a commit.
    for title in ["climbing (sat)", "climbing (sun)"] {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = alice.rename_group(group, title).instrument(tracing::info_span!("app", name = "alice")).await;
    }
    let mut sent = Vec::new();
    for s in senders {
        sent.push(s.await.expect("sender"));
    }
    done.store(true, Ordering::Relaxed);
    let stopped = stops.await.expect("stops");
    eprintln!("the server stopped {stopped} times");
    assert!(stopped >= 5, "the server should have stopped a few times");

    let everyone = [(&alice, "alice"), (&bob, "bob"), (&carol, "carol")];
    arrives_exactly_once(&bob, group, "alice", &sent[0], "bob", &everyone).await;
    arrives_exactly_once(&carol, group, "alice", &sent[0], "carol", &everyone).await;
    arrives_exactly_once(&alice, group, "bob", &sent[1], "alice", &everyone).await;
    arrives_exactly_once(&carol, group, "bob", &sent[1], "carol", &everyone).await;
    arrives_exactly_once(&bob, dm, "dm", &sent[2], "bob", &everyone).await;
    // Everyone is in the same group: the same name, and it still works.
    let renamed = alice.rename_group(group, "climbing").await;
    assert!(renamed.is_ok(), "a commit after the stops: {renamed:?}");
    for (c, who) in [(&bob, "bob"), (&carol, "carol")] {
        assert!(until(Duration::from_secs(30), async || { c.sync().await.is_ok() && c.conversation(group).expect("group").title == "climbing" }).await, "{who} has the same group");
    }
    let last = carol.send_text(group, "carol after").await.expect("send");
    assert!(last.body.is_some());
    arrives_exactly_once(&alice, group, "carol after", &["carol after".to_owned()], "alice", &everyone).await;

    for r in running {
        r.abort();
    }
    let s = server.lock().expect("lock");
    intact(&s.db, &s.key_bytes());
    for name in ["alice", "bob", "carol"] {
        intact(&dir.path().join(name).join("profile.db"), &CLIENT_KEY);
    }
}

// ------------------------------------------------------------ the network cuts out

/// Between the apps and the server: once `flaky`, connections cut at random
/// moments, mid-message included, and outages when no connection gets
/// through.
async fn flaky_network(listener: TcpListener, upstream: std::net::SocketAddr, flaky: Arc<AtomicBool>, outage: Arc<AtomicBool>, rng: Arc<Rng>) {
    loop {
        let Ok((client, _)) = listener.accept().await else { return };
        if outage.load(Ordering::Relaxed) {
            continue;
        }
        let (rng, outage, flaky) = (rng.clone(), outage.clone(), flaky.clone());
        tokio::spawn(async move {
            let Ok(server) = TcpStream::connect(upstream).await else { return };
            let (mut c, mut s) = (client, server);
            let lifetime = if flaky.load(Ordering::Relaxed) { rng.millis(100, 2500) } else { Duration::from_secs(3600) };
            let cut = async {
                let start = Instant::now();
                while start.elapsed() < lifetime && !outage.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            // Whichever comes first; dropping both ends cuts the connection
            // wherever the bytes had got to.
            tokio::select! {
                _ = tokio::io::copy_bidirectional(&mut c, &mut s) => {}
                () = cut => {}
            }
        });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_survive_a_flaky_network() {
    let rng = Rng::new("messages_survive_a_flaky_network");
    let dir = tempfile::tempdir().expect("tempdir");
    let proxy = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let proxy_addr = proxy.local_addr().expect("addr");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let upstream = listener.local_addr().expect("addr");
    let key = [42u8; 32];
    let db = dir.path().join("server.db");
    // Apps know the server by the address they reach, the proxy's.
    let config = post_server::Config {
        server_domain: proxy_addr.to_string(),
        db_path: db.clone(),
        db_key: Zeroizing::new(key),
        releases: post_server::ReleaseConfig::none(),
        calls: None,
    };
    let _server = post_server::spawn(listener, config).expect("server");
    let invite = || {
        let mut conn = post_server::db::open(&db, &key).expect("db");
        post_server::db::create_invite(&mut conn, false, 3_600_000).expect("invite")
    };
    let url = format!("ws://{proxy_addr}/v1/ws");
    // Steady while the accounts and the group are made, then flaky.
    let (flaky, outage) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    tokio::spawn(flaky_network(proxy, upstream, flaky.clone(), outage.clone(), rng.clone()));
    let alice = account(dir.path(), &url, &invite(), "alice").await;
    let bob = account(dir.path(), &url, &invite(), "bob").await;
    let group = alice.create_group("climbing", &["bob".into()]).await.expect("group").id;
    bob.sync().await.expect("sync");
    flaky.store(true, Ordering::Relaxed);
    let running: Vec<_> = [&alice, &bob]
        .into_iter()
        .map(|c| {
            let c = c.clone();
            tokio::spawn(async move { c.run().await })
        })
        .collect();

    let done = Arc::new(AtomicBool::new(false));
    let outages = {
        let (outage, done, rng) = (outage.clone(), done.clone(), rng.clone());
        tokio::spawn(async move {
            while !done.load(Ordering::Relaxed) {
                tokio::time::sleep(rng.millis(500, 2000)).await;
                outage.store(true, Ordering::Relaxed);
                tokio::time::sleep(rng.millis(200, 1500)).await;
                outage.store(false, Ordering::Relaxed);
            }
        })
    };
    let senders = [sender(alice.clone(), group, "alice", 100, rng.clone()), sender(bob.clone(), group, "bob", 100, rng.clone())];
    let mut sent = Vec::new();
    for s in senders {
        sent.push(s.await.expect("sender"));
    }
    done.store(true, Ordering::Relaxed);
    outages.await.expect("outages");
    outage.store(false, Ordering::Relaxed);

    let everyone = [(&alice, "alice"), (&bob, "bob")];
    arrives_exactly_once(&bob, group, "alice", &sent[0], "bob", &everyone).await;
    arrives_exactly_once(&alice, group, "bob", &sent[1], "alice", &everyone).await;
    for r in running {
        r.abort();
    }
    intact(&db, &key);
    for name in ["alice", "bob"] {
        intact(&dir.path().join(name).join("profile.db"), &CLIENT_KEY);
    }
}

// ------------------------------------------------------------ apps are killed

/// The command-line app as a process, so it can be killed mid-write.
struct Cli {
    profile: PathBuf,
    key: PathBuf,
}

impl Cli {
    fn new(dir: &Path, name: &str) -> Self {
        let cli = Self { profile: dir.join(name), key: dir.join(format!("{name}.key")) };
        let ok = Command::new(program("post-cli", "post")).arg("keygen").arg(&cli.key).status().expect("keygen");
        assert!(ok.success());
        cli
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(program("post-cli", "post"));
        c.arg("--profile").arg(&self.profile).arg("--db-key-file").arg(&self.key).args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        c
    }

    /// Run to the end.
    fn run(&self, args: &[&str]) -> String {
        let out = self.command(args).output().expect("post");
        assert!(out.status.success(), "post {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("utf8")
    }

    /// Run, but kill it after `after` if it is still going. True if it
    /// finished by itself and succeeded.
    fn run_or_kill(&self, args: &[&str], after: Duration) -> bool {
        let mut child = self.command(args).spawn().expect("post");
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().expect("wait") {
                return status.success();
            }
            if start.elapsed() >= after {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn key_bytes(&self) -> [u8; 32] {
        let hex = std::fs::read_to_string(&self.key).expect("key");
        let mut k = [0u8; 32];
        hex::decode_to_slice(hex.trim(), &mut k).expect("hex key");
        k
    }
}

/// Alice's app killed at random moments while sending, Bob's while
/// receiving. A message whose send finished must arrive; one killed midway
/// may or may not have been written, and arrives if it was; none arrives
/// twice, and they keep their order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_survive_apps_being_killed() {
    let rng = Rng::new("messages_survive_apps_being_killed");
    let dir = tempfile::tempdir().expect("tempdir");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let key = [42u8; 32];
    let db = dir.path().join("server.db");
    let config = post_server::Config {
        server_domain: addr.to_string(),
        db_path: db.clone(),
        db_key: Zeroizing::new(key),
        releases: post_server::ReleaseConfig::none(),
        calls: None,
    };
    let server = post_server::spawn(listener, config).expect("server");
    let invite = || {
        let mut conn = post_server::db::open(&db, &key).expect("db");
        post_server::db::create_invite(&mut conn, false, 3_600_000).expect("invite")
    };
    let url = server.url();
    let alice = Cli::new(dir.path(), "alice");
    let bob = Cli::new(dir.path(), "bob");
    alice.run(&["register", "--server", &url, "--invite", &invite(), "--username", "alice"]);
    bob.run(&["register", "--server", &url, "--invite", &invite(), "--username", "bob"]);
    let dm: serde_json::Value = serde_json::from_str(&alice.run(&["--json", "dm", "bob"])).expect("json");
    let dm = dm["id"].as_str().expect("id").to_owned();
    bob.run(&["sync"]);

    // Bob's app receiving, killed now and then, the whole time.
    let receiving = Arc::new(AtomicBool::new(true));
    let bob_side = {
        let (receiving, rng) = (receiving.clone(), rng.clone());
        let bob = Cli { profile: bob.profile.clone(), key: bob.key.clone() };
        std::thread::spawn(move || {
            let mut kills = 0;
            while receiving.load(Ordering::Relaxed) {
                if !bob.run_or_kill(&["sync"], rng.millis(5, 400)) {
                    kills += 1;
                }
            }
            kills
        })
    };
    let mut finished = Vec::new();
    let mut killed = HashSet::new();
    let mut order = Vec::new();
    for i in 0..40 {
        let text = format!("alice {i:03}");
        order.push(text.clone());
        // Half are killed somewhere between starting and finishing.
        let limit = if rng.chance(50) { rng.millis(1, 300) } else { Duration::from_secs(30) };
        if alice.run_or_kill(&["send", "--conversation", &dm, &text], limit) {
            finished.push(text);
        } else {
            killed.insert(text);
        }
    }
    receiving.store(false, Ordering::Relaxed);
    let bob_kills = bob_side.join().expect("bob");
    eprintln!("alice's app killed {} times while sending, bob's {bob_kills} times while receiving", killed.len());

    // Both apps open again and finish: what was written goes out, and
    // comes in.
    alice.run(&["sync"]);
    bob.run(&["sync"]);
    let got: Vec<serde_json::Value> = serde_json::from_str(&bob.run(&["--json", "messages", &dm, "--limit", "500"])).expect("json");
    let got: Vec<String> = got.iter().filter_map(|m| m["body"].as_str()).filter(|b| b.starts_with("alice ")).map(str::to_owned).collect();
    let unique: HashSet<&String> = got.iter().collect();
    assert_eq!(unique.len(), got.len(), "no message arrives twice: {got:?}");
    for f in &finished {
        assert!(got.contains(f), "{f} finished sending but never arrived");
    }
    for g in &got {
        assert!(finished.contains(g) || killed.contains(g), "{g} was never sent");
    }
    let sent_order: Vec<&String> = order.iter().filter(|t| got.contains(t)).collect();
    assert_eq!(got.iter().collect::<Vec<_>>(), sent_order, "in the order they were sent");
    intact(&db, &key);
    intact(&alice.profile.join("profile.db"), &alice.key_bytes());
    intact(&bob.profile.join("profile.db"), &bob.key_bytes());
}
