//! Spike 1: OpenMLS with the hybrid post-quantum ciphersuite.
//!
//! For each provider and group size it builds a group, then measures:
//! creating it, joining from a Welcome, a key-refresh commit, adding and
//! removing one member, and a short application message.
//!
//! Run with: cargo run --release -- [sizes...]          (default: 3 10 50 100 250)
//!       or: cargo run --release -- --warm [sizes...]   (default: 16 64 128)

use std::time::{Duration, Instant};

use openmls::prelude::{tls_codec::*, *};
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::OpenMlsProvider;

const SUITE: Ciphersuite = Ciphersuite::MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519;

struct Member<P> {
    provider: P,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
}

fn new_member<P: OpenMlsProvider + Default>(i: usize) -> Member<P> {
    let provider = P::default();
    let signer = SignatureKeyPair::new(SUITE.signature_algorithm()).unwrap();
    signer.store(provider.storage()).unwrap();
    let credential = CredentialWithKey {
        credential: BasicCredential::new(format!("device-{i}").into_bytes()).into(),
        signature_key: signer.to_public_vec().into(),
    };
    Member { provider, signer, credential }
}

fn key_package<P: OpenMlsProvider>(m: &Member<P>) -> KeyPackage {
    KeyPackage::builder()
        .build(SUITE, &m.provider, &m.signer, m.credential.clone())
        .unwrap()
        .key_package()
        .clone()
}

fn bytes(msg: &MlsMessageOut) -> Vec<u8> {
    msg.tls_serialize_detached().unwrap()
}

fn to_protocol(b: &[u8]) -> ProtocolMessage {
    MlsMessageIn::tls_deserialize_exact(b)
        .unwrap()
        .try_into_protocol_message()
        .unwrap()
}

fn join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(true)
        .padding_size(256)
        .max_past_epochs(0)
        .build()
}

fn join<P: OpenMlsProvider>(m: &Member<P>, welcome: &[u8]) -> MlsGroup {
    let welcome = match MlsMessageIn::tls_deserialize_exact(welcome).unwrap().extract() {
        MlsMessageBodyIn::Welcome(w) => w,
        _ => panic!("not a welcome"),
    };
    StagedWelcome::new_from_welcome(&m.provider, &join_config(), welcome, None)
        .unwrap()
        .into_group(&m.provider)
        .unwrap()
}

fn apply_commit<P: OpenMlsProvider>(g: &mut MlsGroup, m: &Member<P>, commit: &[u8]) {
    let processed = g.process_message(&m.provider, to_protocol(commit)).unwrap();
    match processed.into_content() {
        ProcessedMessageContent::StagedCommitMessage(staged) => {
            g.merge_staged_commit(&m.provider, *staged).unwrap()
        }
        _ => panic!("expected a commit"),
    }
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1e3)
}

fn kb(n: usize) -> String {
    format!("{:.1} KB", n as f64 / 1024.0)
}

fn run<P: OpenMlsProvider + Default>(provider_name: &str, n: usize) {
    assert!(n >= 3, "need at least 3 leaves");
    let t = Instant::now();
    let members: Vec<Member<P>> = (0..n + 1).map(new_member::<P>).collect();
    let kps: Vec<KeyPackage> = members[1..n].iter().map(key_package).collect();
    let setup = t.elapsed();
    let kp_size = kps[0].tls_serialize_detached().unwrap().len();

    // Device 0 creates the group and adds everyone else in one commit.
    let alice = &members[0];
    let mut a = MlsGroup::builder()
        .ciphersuite(SUITE)
        .with_wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(true)
        .padding_size(256)
        .max_past_epochs(0)
        .build(&alice.provider, &alice.signer, alice.credential.clone())
        .unwrap();
    let t = Instant::now();
    let (commit, welcome, _) = a.add_members(&alice.provider, &alice.signer, &kps).unwrap();
    a.merge_pending_commit(&alice.provider).unwrap();
    let create = t.elapsed();
    let (create_commit, welcome) = (bytes(&commit), bytes(&welcome));

    // Two real members join; the rest exist only as leaves.
    let t = Instant::now();
    let mut b = join(&members[1], &welcome);
    let join_time = t.elapsed();
    let mut c = join(&members[n - 1], &welcome);

    // Key refresh: B updates its own leaf; A and C process it.
    let t = Instant::now();
    let bundle = b.self_update(&members[1].provider, &members[1].signer, LeafNodeParameters::default()).unwrap();
    b.merge_pending_commit(&members[1].provider).unwrap();
    let update_create = t.elapsed();
    let update = bytes(bundle.commit());
    let t = Instant::now();
    apply_commit(&mut a, alice, &update);
    let update_process = t.elapsed();
    apply_commit(&mut c, &members[n - 1], &update);

    // Add one new device to the full group.
    let newcomer = &members[n];
    let kp = key_package(newcomer);
    let t = Instant::now();
    let (commit, welcome1, _) = a.add_members(&alice.provider, &alice.signer, &[kp]).unwrap();
    a.merge_pending_commit(&alice.provider).unwrap();
    let add_time = t.elapsed();
    let (add_commit, welcome1) = (bytes(&commit), bytes(&welcome1));
    apply_commit(&mut b, &members[1], &add_commit);
    apply_commit(&mut c, &members[n - 1], &add_commit);
    let mut d = join(newcomer, &welcome1);

    // Remove one device.
    let victim = c.own_leaf_index();
    let t = Instant::now();
    let (commit, _, _) = a.remove_members(&alice.provider, &alice.signer, &[victim]).unwrap();
    a.merge_pending_commit(&alice.provider).unwrap();
    let remove_time = t.elapsed();
    let remove_commit = bytes(&commit);
    apply_commit(&mut b, &members[1], &remove_commit);
    apply_commit(&mut d, newcomer, &remove_commit);

    // Application message, 100 bytes of text.
    let text = vec![b'x'; 100];
    let msg = bytes(&b.create_message(&members[1].provider, &members[1].signer, &text).unwrap());
    for (g, m) in [(&mut a, alice), (&mut d, newcomer)] {
        match g.process_message(&m.provider, to_protocol(&msg)).unwrap().into_content() {
            ProcessedMessageContent::ApplicationMessage(app) => assert_eq!(app.into_bytes(), text),
            _ => panic!("expected an application message"),
        }
    }

    // Everyone still in agrees on the epoch and the exporter secret.
    let ex = |g: &MlsGroup, p: &P| g.export_secret(p.crypto(), "postern/call/v1", b"call", 32).unwrap();
    let e = ex(&a, &alice.provider);
    assert_eq!(e, ex(&b, &members[1].provider));
    assert_eq!(e, ex(&d, &newcomer.provider));
    assert_eq!(a.epoch(), d.epoch());

    println!(
        "| {provider_name} | {n} | {} | {} / {} | {} / {} | {} | {} / {} | {} / {} | {} / {} | {} |",
        kb(kp_size),
        kb(create_commit.len()), ms(create),
        kb(welcome.len()), ms(join_time),
        ms(update_create).replace(" ms", "") + " / " + &ms(update_process),
        kb(update.len()), ms(update_create),
        kb(add_commit.len()), kb(welcome1.len()),
        kb(remove_commit.len()), ms(remove_time),
        msg.len(),
    );
    let _ = (setup, add_time);
}


/// Warm tree: every member joins and commits one key refresh, which fills the
/// tree's internal nodes the way weekly refreshes will in real use. Then
/// measure a refresh commit, and a Welcome for one more device.
fn run_warm<P: OpenMlsProvider + Default>(provider_name: &str, n: usize) {
    let members: Vec<Member<P>> = (0..n + 1).map(new_member::<P>).collect();
    let kps: Vec<KeyPackage> = members[1..n].iter().map(key_package).collect();
    let m0 = &members[0];
    let mut groups = vec![MlsGroup::builder()
        .ciphersuite(SUITE)
        .with_wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(true)
        .padding_size(256)
        .max_past_epochs(0)
        .build(&m0.provider, &m0.signer, m0.credential.clone())
        .unwrap()];
    let (_, welcome, _) = groups[0].add_members(&m0.provider, &m0.signer, &kps).unwrap();
    groups[0].merge_pending_commit(&m0.provider).unwrap();
    let welcome = bytes(&welcome);
    for m in &members[1..n] {
        groups.push(join(m, &welcome));
    }

    let mut cold = 0;
    for i in 0..n {
        let bundle = groups[i].self_update(&members[i].provider, &members[i].signer, LeafNodeParameters::default()).unwrap();
        groups[i].merge_pending_commit(&members[i].provider).unwrap();
        let c = bytes(bundle.commit());
        if i == 1 { cold = c.len(); }
        for j in 0..n {
            if j != i { apply_commit(&mut groups[j], &members[j], &c); }
        }
    }

    // Measure on the warm tree.
    let i = n / 2;
    let t = Instant::now();
    let bundle = groups[i].self_update(&members[i].provider, &members[i].signer, LeafNodeParameters::default()).unwrap();
    groups[i].merge_pending_commit(&members[i].provider).unwrap();
    let refresh_create = t.elapsed();
    let refresh = bytes(bundle.commit());
    let t = Instant::now();
    apply_commit(&mut groups[0], m0, &refresh);
    let refresh_process = t.elapsed();

    let kp = key_package(&members[n]);
    let (add, w, _) = groups[0].add_members(&m0.provider, &m0.signer, &[kp]).unwrap();
    groups[0].merge_pending_commit(&m0.provider).unwrap();
    let (add, w) = (bytes(&add), bytes(&w));
    let t = Instant::now();
    let joined = join(&members[n], &w);
    let join_time = t.elapsed();
    assert_eq!(joined.epoch(), groups[0].epoch());

    println!(
        "| {provider_name} | {n} | {} | {} | {} / {} | {} | {} / {} |",
        kb(cold), kb(refresh.len()), ms(refresh_create), ms(refresh_process),
        kb(add.len()), kb(w.len()), ms(join_time),
    );
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--warm") {
        args.remove(0);
        let sizes: Vec<usize> = args.iter().map(|s| s.parse().unwrap()).collect();
        let sizes = if sizes.is_empty() { vec![16, 64, 128] } else { sizes };
        println!("suite: {SUITE:?} (0x{:04X}), warm tree\n", SUITE as u16);
        println!("| provider | leaves | refresh, cold tree | refresh, warm tree | create / process | add-1 commit | Welcome / join time |");
        println!("|---|---|---|---|---|---|---|");
        for &n in &sizes {
            run_warm::<openmls_rust_crypto::OpenMlsRustCrypto>("rustcrypto", n);
        }
        return;
    }
    let sizes: Vec<usize> = args.iter().map(|s| s.parse().unwrap()).collect();
    let sizes = if sizes.is_empty() { vec![3, 10, 50, 100, 250] } else { sizes };
    println!("suite: {SUITE:?} (0x{:04X})\n", SUITE as u16);
    println!("| provider | leaves | KeyPackage | create: commit / time | Welcome / join time | refresh: create / process | refresh commit / create | add-1: commit / Welcome | remove-1: commit / time | 100 B msg |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for &n in &sizes {
        run::<openmls_libcrux_crypto::Provider>("libcrux", n);
        run::<openmls_rust_crypto::OpenMlsRustCrypto>("rustcrypto", n);
    }
}
