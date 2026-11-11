//! The server's metadata protections (design section 8), tested with a raw
//! protocol client playing a curious or misbehaving peer.

use futures_util::{SinkExt as _, StreamExt as _};
use postern_identity::{Chain, DeviceKeys, labels};
use postern_proto::{self as proto, ClientFrame, ErrorCode, Reply, Request, ServerFrame, ServerKind};
use prost::Message as _;
use tokio_tungstenite::tungstenite::Message;
use zeroize::Zeroizing;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Server {
    running: postern_server::Running,
    domain: String,
    db: std::path::PathBuf,
    key: [u8; 32],
    _dir: tempfile::TempDir,
}

impl Server {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let domain = format!("127.0.0.1:{}", listener.local_addr().expect("addr").port());
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("server.db");
        let key = [3u8; 32];
        let running = postern_server::spawn(
            listener,
            postern_server::Config { server_domain: domain.clone(), db_path: db.clone(), db_key: Zeroizing::new(key) },
        )
        .expect("start");
        Self { running, domain, db, key, _dir: dir }
    }

    fn invite(&self) -> String {
        let mut c = postern_server::db::open(&self.db, &self.key).expect("db");
        postern_server::db::create_invite(&mut c, false, 3_600_000).expect("invite")
    }
}

struct Client {
    ws: Ws,
    nonce: Vec<u8>,
    next: u64,
}

impl Client {
    async fn connect(url: &str) -> Self {
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("connect");
        let hello = match next(&mut ws).await.kind {
            Some(ServerKind::Hello(h)) => h,
            other => panic!("expected Hello, got {other:?}"),
        };
        Self { ws, nonce: hello.nonce, next: 0 }
    }

    async fn call(&mut self, req: Request) -> Reply {
        self.next += 1;
        let frame = ClientFrame { id: self.next, request: Some(req) }.encode_to_vec();
        self.ws.send(Message::Binary(frame.into())).await.expect("send");
        loop {
            if let Some(ServerKind::Response(r)) = next(&mut self.ws).await.kind
                && r.id == self.next
            {
                return r.reply.expect("reply");
            }
        }
    }

    /// A logged-in session for a newly registered account.
    async fn register(server: &Server, username: &str) -> (Self, DeviceKeys) {
        let mut c = Self::connect(&server.running.url()).await;
        let keys = DeviceKeys::generate().expect("keys");
        let (_, genesis) = Chain::create(&keys, &server.domain, username, false, 1).expect("chain");
        let sig = keys.chain.sign(labels::AUTH, &proto::auth_message(&server.domain, &c.nonce)).expect("sign");
        let reply = c
            .call(Request::Register(proto::Register {
                invite_code: server.invite(),
                genesis,
                auth_sig_ed25519: sig.ed25519_bytes().to_vec(),
                auth_sig_mldsa65: sig.mldsa65_bytes().to_vec(),
            }))
            .await;
        assert!(matches!(reply, Reply::Authenticated(_)), "{reply:?}");
        (c, keys)
    }
}

async fn next(ws: &mut Ws) -> ServerFrame {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await.expect("timely").expect("open").expect("ok") {
            Message::Binary(b) => return ServerFrame::decode(b.as_ref()).expect("frame"),
            _ => continue,
        }
    }
}

fn code(r: &Reply) -> ErrorCode {
    match r {
        Reply::Error(e) => e.code(),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test]
async fn connection_kinds_never_mix() {
    let server = Server::start().await;

    // A device request on a connection that has not logged in.
    let mut anon = Client::connect(&server.running.url()).await;
    assert_eq!(code(&anon.call(Request::Directory(proto::Directory::default())).await), ErrorCode::Unauthenticated);

    // A conversation request on a logged-in session.
    let (mut session, _) = Client::register(&server, "alice").await;
    let send = proto::Send { group_id: vec![1; 16], access_key: vec![2; 32], client_msg_id: vec![3; 16], message: vec![] };
    assert_eq!(code(&session.call(Request::Send(send.clone())).await), ErrorCode::WrongConnection);
    let claim = proto::ClaimKeyPackages { account_id: vec![0; 32], ..Default::default() };
    assert_eq!(code(&session.call(Request::ClaimKeyPackages(claim)).await), ErrorCode::WrongConnection);
    // Linking runs anonymously too: a new device has no account to log in with.
    let put = proto::LinkPut { rendezvous: vec![1; 16], slot: 0, payload: vec![1] };
    assert_eq!(code(&session.call(Request::LinkPut(put)).await), ErrorCode::WrongConnection);

    // An anonymous connection, once used for conversations, cannot log in or
    // make device requests.
    let mut anon = Client::connect(&server.running.url()).await;
    let fetch = proto::FetchGroups { groups: vec![], limit: 0 };
    assert!(matches!(anon.call(Request::FetchGroups(fetch)).await, Reply::GroupBatch(_)));
    assert_eq!(code(&anon.call(Request::Directory(proto::Directory::default())).await), ErrorCode::WrongConnection);
    let auth = proto::Auth { device_id: vec![0; 16], sig_ed25519: vec![], sig_mldsa65: vec![] };
    assert_eq!(code(&anon.call(Request::Auth(auth)).await), ErrorCode::BadRequest);
}

#[tokio::test]
async fn group_access_needs_the_epoch_key() {
    let server = Server::start().await;
    let mut anon = Client::connect(&server.running.url()).await;
    // Unknown group and wrong key look the same, so guessing reveals nothing.
    let cursor = proto::GroupCursor { group_id: vec![7; 16], epoch: 1, access_key: vec![0; 32], after_seq: 0 };
    let Reply::GroupBatch(b) = anon.call(Request::FetchGroups(proto::FetchGroups { groups: vec![cursor.clone()], limit: 0 })).await
    else {
        panic!("expected a batch")
    };
    assert_eq!(b.groups[0].error.as_ref().map(proto::Error::code), Some(ErrorCode::BadAccessKey));
    let ephemeral = proto::SendEphemeral { group_id: vec![7; 16], access_key: vec![0; 32], payload: vec![1] };
    assert_eq!(code(&anon.call(Request::SendEphemeral(ephemeral)).await), ErrorCode::BadAccessKey);
}

#[tokio::test]
async fn tokens_are_required_unforgeable_and_single_use() {
    use blind_rsa_signatures::{BlindSignature, DefaultRng, PublicKeySha384PSSRandomized as Pk};

    let server = Server::start().await;
    let (mut session, _) = Client::register(&server, "alice").await;
    let Reply::TokenKey(key) = session.call(Request::GetTokenKey(proto::GetTokenKey {})).await else { panic!("token key") };
    let pk = Pk::from_spki(&key.public_key).expect("pk");
    let msg = [5u8; 32];
    let blinded = pk.blind(&mut DefaultRng, msg).expect("blind");
    let Reply::IssuedTokens(issued) = session
        .call(Request::IssueTokens(proto::IssueTokens { key_id: key.key_id.clone(), blinded: vec![blinded.blind_message.0.clone()] }))
        .await
    else {
        panic!("tokens")
    };
    let sig = pk.finalize(&BlindSignature(issued.blind_signatures[0].clone()), &blinded, msg).expect("finalize");
    let token = proto::Token {
        version: proto::TOKEN_VERSION_RSABSSA_SHA384,
        key_id: key.key_id.clone(),
        message: msg.to_vec(),
        randomizer: blinded.msg_randomizer.expect("randomizer").0.to_vec(),
        signature: sig.0,
    };
    let account = {
        let Reply::DirectoryUpdate(d) = session.call(Request::Directory(proto::Directory::default())).await else { panic!("dir") };
        d.accounts[0].account_id.clone()
    };

    let mut anon = Client::connect(&server.running.url()).await;
    let claim = |t: Option<proto::Token>| {
        Request::ClaimKeyPackages(proto::ClaimKeyPackages { token: t, account_id: account.clone(), ..Default::default() })
    };
    // No token.
    assert_eq!(code(&anon.call(claim(None)).await), ErrorCode::BadToken);
    // A forged token: right shape, signature over a different message.
    let mut forged = token.clone();
    forged.message = vec![6; 32];
    assert_eq!(code(&anon.call(claim(Some(forged))).await), ErrorCode::BadToken);
    // A real token works once.
    assert!(matches!(anon.call(claim(Some(token.clone()))).await, Reply::ClaimedKeyPackages(_)));
    assert_eq!(code(&anon.call(claim(Some(token))).await), ErrorCode::BadToken);
}

#[test]
fn no_table_links_identities_to_groups() {
    let dir = tempfile::tempdir().expect("tempdir");
    let conn = postern_server::db::open(&dir.path().join("s.db"), &[1; 32]).expect("open");
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .expect("prepare")
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    for t in &tables {
        let cols: Vec<String> = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{t}')"))
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        let has_group = cols.iter().any(|c| c.contains("group"));
        let has_identity = cols.iter().any(|c| c.contains("device") || c.contains("account") || c.contains("sender"));
        assert!(!(has_group && has_identity), "table {t} links groups to identities: {cols:?}");
    }
}
