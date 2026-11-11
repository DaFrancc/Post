//! Phase 2's second done-when: a test drives a client through the JSON-RPC
//! daemon. Alice is a bot speaking JSON-RPC over an in-memory pipe; Bob is
//! an ordinary client.

use std::time::Duration;

use postern_core::{Core, DbKey};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use zeroize::Zeroizing;

/// A JSON-RPC client over a byte stream, as a bot in another language would be.
struct Rpc {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    next_id: u64,
    events: Vec<Value>,
}

impl Rpc {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next_id += 1;
        let id = self.next_id;
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string() + "\n";
        self.write.write_all(line.as_bytes()).await.expect("write");
        loop {
            let v = self.read().await;
            if v.get("id") == Some(&json!(id)) {
                return match v.get("error") {
                    Some(e) => Err(e.clone()),
                    None => Ok(v["result"].clone()),
                };
            }
            self.events.push(v);
        }
    }

    async fn read(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(20), self.lines.next_line())
            .await
            .expect("daemon answered in time")
            .expect("read")
            .expect("daemon still running");
        serde_json::from_str(&line).expect("valid JSON")
    }

    /// Wait for an `event` notification matching `want`.
    async fn event(&mut self, want: impl Fn(&Value) -> bool) -> Value {
        if let Some(i) = self.events.iter().position(|e| want(&e["params"])) {
            return self.events.remove(i);
        }
        loop {
            let v = self.read().await;
            if v["method"] == "event" && want(&v["params"]) {
                return v;
            }
            self.events.push(v);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bot_drives_postern_through_json_rpc() {
    // Server.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("server.db");
    let key = [9u8; 32];
    let running = postern_server::spawn(
        listener,
        postern_server::Config { server_domain: format!("127.0.0.1:{port}"), db_path: db.clone(), db_key: Zeroizing::new(key) },
    )
    .expect("server");
    let invite = |bot| {
        let mut c = postern_server::db::open(&db, &key).expect("db");
        postern_server::db::create_invite(&mut c, bot, 3_600_000).expect("invite")
    };

    // Bob, an ordinary client.
    let bob = Core::open(&dir.path().join("bob"), &DbKey::from_bytes([1; 32])).expect("bob");
    bob.register(&running.url(), &invite(false), "bob", false).await.expect("bob registers");

    // The bot, behind the daemon.
    let bot = Core::open(&dir.path().join("bot"), &DbKey::from_bytes([2; 32])).expect("bot");
    let (client, server_side) = tokio::io::duplex(1 << 20);
    let (sr, sw) = tokio::io::split(server_side);
    tokio::spawn(postern_cli::daemon::serve(bot.clone(), sr, sw));
    tokio::spawn(postern_cli::daemon::supervise(bot));
    let (cr, cw) = tokio::io::split(client);
    let mut rpc = Rpc { write: cw, lines: BufReader::new(cr).lines(), next_id: 0, events: Vec::new() };

    // Protocol errors are JSON-RPC errors.
    assert_eq!(rpc.call("no_such_method", json!({})).await.expect_err("unknown")["code"], -32601);
    assert_eq!(rpc.call("whoami", json!({ "typo": 1 })).await.expect_err("unknown field")["code"], -32602);
    assert_eq!(rpc.call("whoami", json!({})).await.expect("whoami"), Value::Null);
    let schema = rpc.call("schema", json!({})).await.expect("schema");
    assert_eq!(schema["info"]["version"], postern_api::VERSION);

    // Register as a bot and subscribe to events.
    let me = rpc
        .call("register", json!({ "server": running.url(), "invite": invite(true), "username": "weatherbot", "bot": true }))
        .await
        .expect("register");
    assert_eq!(me["username"], "weatherbot");
    assert_eq!(me["bot"], true);
    rpc.call("subscribe", json!({})).await.expect("subscribe");
    rpc.call("set_profile", json!({ "display_name": "Weather", "typing_template": "$$NAME$$ is checking the sky" }))
        .await
        .expect("profile");

    // DM Bob by username; Bob sees it, with the bot's profile.
    let sent = rpc.call("send", json!({ "to": "bob", "text": "sunny today" })).await.expect("send");
    assert_eq!(sent["state"], "sent");
    let conversation = sent["conversation"].as_str().expect("conversation").to_string();
    bob.sync().await.expect("bob sync");
    let bob_view = bob.messages(conversation.parse().expect("id"), 10).expect("messages");
    assert_eq!(bob_view.last().and_then(|m| m.body.as_deref()), Some("sunny today"));
    assert_eq!(bob_view.last().map(|m| m.sender.as_str()), Some("Weather"));

    // Bob replies; the bot gets a live `message_added` event.
    bob.send_text(conversation.parse().expect("id"), "thanks!").await.expect("reply");
    let e = rpc.event(|p| p["type"] == "message_added" && p["message"]["body"] == "thanks!").await;
    assert_eq!(e["params"]["message"]["sender_username"], "bob");

    // Edit and read back every version over JSON-RPC.
    let id = sent["id"].as_str().expect("id");
    rpc.call("edit", json!({ "conversation": conversation, "message": id, "text": "sunny, 24°C" })).await.expect("edit");
    let versions = rpc.call("versions", json!({ "conversation": conversation, "message": id })).await.expect("versions");
    let bodies: Vec<&str> = versions.as_array().expect("array").iter().filter_map(|v| v["body"].as_str()).collect();
    assert_eq!(bodies, vec!["sunny today", "sunny, 24°C"]);

    // Local errors carry Postern codes.
    let err = rpc.call("start_dm", json!({ "username": "nobody_here" })).await.expect_err("unknown user");
    assert_eq!(err["code"], 1008);
}
