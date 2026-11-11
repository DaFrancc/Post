//! Spike 1, storage half: can OpenMLS state live in the app's own SQLCipher
//! database and share a transaction with app tables?
//!
//! Bob's device is backed by an encrypted SQLite file. Alice commits a key
//! refresh and sends a message. Bob processes the commit inside a transaction
//! that also writes an app row, then rolls back as if the app had crashed.
//! The group must come back at the old epoch, with no app row, and processing
//! the same commit again must succeed.

use openmls::prelude::{tls_codec::*, *};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::{OpenMlsRustCrypto, RustCrypto};
use openmls_sqlite_storage::{Codec, Connection, SqliteStorageProvider};
use openmls_traits::OpenMlsProvider;

const SUITE: Ciphersuite = Ciphersuite::MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519;

#[derive(Default)]
struct Json;
impl Codec for Json {
    type Error = serde_json::Error;
    fn to_vec<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(v)
    }
    fn from_slice<T: serde::de::DeserializeOwned>(b: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(b)
    }
}

/// A provider whose storage is whatever connection (or transaction) it is given.
struct Db<'c> {
    crypto: RustCrypto,
    storage: SqliteStorageProvider<Json, &'c Connection>,
}
impl<'c> Db<'c> {
    fn on(conn: &'c Connection) -> Self {
        Db { crypto: RustCrypto::default(), storage: SqliteStorageProvider::new(conn) }
    }
}
impl<'c> OpenMlsProvider for Db<'c> {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = SqliteStorageProvider<Json, &'c Connection>;
    fn storage(&self) -> &Self::StorageProvider { &self.storage }
    fn crypto(&self) -> &Self::CryptoProvider { &self.crypto }
    fn rand(&self) -> &Self::RandProvider { &self.crypto }
}

fn credential(name: &str, signer: &SignatureKeyPair) -> CredentialWithKey {
    CredentialWithKey {
        credential: BasicCredential::new(name.as_bytes().to_vec()).into(),
        signature_key: signer.to_public_vec().into(),
    }
}

fn wire(m: &MlsMessageOut) -> Vec<u8> { m.tls_serialize_detached().unwrap() }
fn proto(b: &[u8]) -> ProtocolMessage {
    MlsMessageIn::tls_deserialize_exact(b).unwrap().try_into_protocol_message().unwrap()
}

fn main() {
    let dir = std::env::temp_dir().join(format!("postern-spike-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bob.db");

    // Open an encrypted database.
    let mut conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "key", "x'2DD29CA851E7B56E4697B0E1F08507293D761A05CE4D1B628663F411A8086D99'").unwrap();
    let cipher: String = conn.query_row("PRAGMA cipher_version", [], |r| r.get(0)).unwrap();
    SqliteStorageProvider::<Json, &mut Connection>::new(&mut conn).run_migrations().unwrap();
    conn.execute("CREATE TABLE messages (id INTEGER PRIMARY KEY, body BLOB NOT NULL)", []).unwrap();

    // Alice: in-memory. Bob: database-backed.
    let alice_p = OpenMlsRustCrypto::default();
    let alice_s = SignatureKeyPair::new(SUITE.signature_algorithm()).unwrap();
    alice_s.store(alice_p.storage()).unwrap();
    let bob_s = SignatureKeyPair::new(SUITE.signature_algorithm()).unwrap();
    let bob_kp = {
        let db = Db::on(&conn);
        bob_s.store(db.storage()).unwrap();
        KeyPackage::builder().build(SUITE, &db, &bob_s, credential("bob", &bob_s)).unwrap().key_package().clone()
    };

    let mut alice = MlsGroup::builder()
        .ciphersuite(SUITE)
        .with_wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(true)
        .build(&alice_p, &alice_s, credential("alice", &alice_s))
        .unwrap();
    let (_, welcome, _) = alice.add_members(&alice_p, &alice_s, &[bob_kp]).unwrap();
    alice.merge_pending_commit(&alice_p).unwrap();
    let group_id = alice.group_id().clone();

    let join_cfg = MlsGroupJoinConfig::builder()
        .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(true)
        .max_past_epochs(0)
        .build();
    {
        let db = Db::on(&conn);
        let w = match MlsMessageIn::tls_deserialize_exact(&wire(&welcome)).unwrap().extract() {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => unreachable!(),
        };
        StagedWelcome::new_from_welcome(&db, &join_cfg, w, None).unwrap().into_group(&db).unwrap();
    }

    // Alice refreshes her key and sends a message in the new epoch.
    let commit = wire(alice.self_update(&alice_p, &alice_s, LeafNodeParameters::default()).unwrap().commit());
    alice.merge_pending_commit(&alice_p).unwrap();
    let msg = wire(&alice.create_message(&alice_p, &alice_s, b"hello bob").unwrap());

    let epoch_before = MlsGroup::load(Db::on(&conn).storage(), &group_id).unwrap().unwrap().epoch();

    // Attempt 1: process the commit and write an app row, then "crash".
    {
        let tx = conn.transaction().unwrap();
        let db = Db::on(&tx);
        let mut bob = MlsGroup::load(db.storage(), &group_id).unwrap().unwrap();
        match bob.process_message(&db, proto(&commit)).unwrap().into_content() {
            ProcessedMessageContent::StagedCommitMessage(c) => bob.merge_staged_commit(&db, *c).unwrap(),
            _ => unreachable!(),
        }
        tx.execute("INSERT INTO messages (body) VALUES (?1)", [b"from attempt 1".as_slice()]).unwrap();
        assert_eq!(bob.epoch(), alice.epoch());
        drop(db);
        tx.rollback().unwrap();
    }
    let reloaded = MlsGroup::load(Db::on(&conn).storage(), &group_id).unwrap().unwrap();
    let rows: i64 = conn.query_row("SELECT count(*) FROM messages", [], |r| r.get(0)).unwrap();
    assert_eq!(reloaded.epoch(), epoch_before, "MLS state must roll back with the transaction");
    assert_eq!(rows, 0, "app row must roll back with the transaction");

    // Attempt 2: same commit again, then the message, each in its own transaction.
    for bytes in [&commit, &msg] {
        let tx = conn.transaction().unwrap();
        let db = Db::on(&tx);
        let mut bob = MlsGroup::load(db.storage(), &group_id).unwrap().unwrap();
        match bob.process_message(&db, proto(bytes)).unwrap().into_content() {
            ProcessedMessageContent::StagedCommitMessage(c) => bob.merge_staged_commit(&db, *c).unwrap(),
            ProcessedMessageContent::ApplicationMessage(m) => {
                tx.execute("INSERT INTO messages (body) VALUES (?1)", [m.into_bytes()]).unwrap();
            }
            _ => unreachable!(),
        }
        drop(db);
        tx.commit().unwrap();
    }
    let bob = MlsGroup::load(Db::on(&conn).storage(), &group_id).unwrap().unwrap();
    let body: Vec<u8> = conn.query_row("SELECT body FROM messages", [], |r| r.get(0)).unwrap();
    assert_eq!(bob.epoch(), alice.epoch());
    assert_eq!(body, b"hello bob");
    drop(conn);

    // The file on disk must not be a plain SQLite database.
    let header = std::fs::read(&path).unwrap();
    assert_ne!(&header[..15], b"SQLite format 3", "database is not encrypted");
    std::fs::remove_dir_all(&dir).unwrap();

    println!("sqlcipher {cipher}");
    println!("rollback after processing a commit: MLS state and app row both reverted");
    println!("reprocessing the same commit after rollback: ok (epoch {:?})", bob.epoch());
    println!("database file is encrypted: ok");
}
