//! Spike 1's interop check, kept in CI (design section 6, Ciphersuite).
//!
//! Post uses the libcrux provider, whose ML-KEM is formally verified. That
//! provider accepts the hybrid suite but does not advertise it, so upstream
//! does not test the combination. An independent implementation (RustCrypto's
//! `ml-kem` and `x-wing`) must agree with it, in both directions, using
//! Post's own group settings. An OpenMLS upgrade that breaks agreement
//! fails here.

use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::OpenMlsProvider;
use post_identity::{DeviceId, LinkHash};
use post_mls::{GroupMeta, SUITE, capabilities, credential, group_builder, join_config};

fn member<P: OpenMlsProvider + Default>(n: u8) -> (P, SignatureKeyPair, CredentialWithKey, LinkHash) {
    let provider = P::default();
    let signer = SignatureKeyPair::new(SUITE.signature_algorithm()).expect("keys");
    let account = LinkHash([n; 32]);
    let cred = CredentialWithKey { credential: credential(&account, &DeviceId([n; 16])), signature_key: signer.to_public_vec().into() };
    (provider, signer, cred, account)
}

fn wire(m: &MlsMessageOut) -> Vec<u8> {
    m.tls_serialize_detached().expect("serialize")
}

fn process<P: OpenMlsProvider>(g: &mut MlsGroup, p: &P, b: &[u8]) -> Option<Vec<u8>> {
    let msg = MlsMessageIn::tls_deserialize_exact(b).expect("decode").try_into_protocol_message().expect("protocol");
    match g.process_message(p, msg).expect("process").into_content() {
        ProcessedMessageContent::StagedCommitMessage(c) => {
            g.merge_staged_commit(p, *c).expect("merge");
            None
        }
        ProcessedMessageContent::ApplicationMessage(m) => Some(m.into_bytes()),
        _ => panic!("unexpected content"),
    }
}

/// `creator` builds a DM and adds `joiner`; they exchange refreshes and
/// messages, and must agree on the exporter secret.
fn round_trip<C: OpenMlsProvider + Default, J: OpenMlsProvider + Default>() {
    let (cp, cs, cc, ca) = member::<C>(1);
    let (jp, js, jc, ja) = member::<J>(2);
    let kp = KeyPackage::builder().leaf_node_capabilities(capabilities()).build(SUITE, &jp, &js, jc).expect("kp");
    let mut c = group_builder(&GroupMeta::dm(&ca, &ja)).expect("builder").build(&cp, &cs, cc).expect("group");
    let (_, welcome, _) = c.add_members(&cp, &cs, &[kp.key_package().clone()]).expect("add");
    c.merge_pending_commit(&cp).expect("merge");
    let welcome = match MlsMessageIn::tls_deserialize_exact(wire(&welcome)).expect("decode").extract() {
        MlsMessageBodyIn::Welcome(w) => w,
        _ => panic!("not a welcome"),
    };
    let mut j = StagedWelcome::new_from_welcome(&jp, &join_config(), welcome, None).expect("stage").into_group(&jp).expect("join");

    for round in 0..3u8 {
        let commit = wire(c.self_update(&cp, &cs, LeafNodeParameters::default()).expect("update").commit());
        c.merge_pending_commit(&cp).expect("merge");
        process(&mut j, &jp, &commit);
        let commit = wire(j.self_update(&jp, &js, LeafNodeParameters::default()).expect("update").commit());
        j.merge_pending_commit(&jp).expect("merge");
        process(&mut c, &cp, &commit);

        let m = wire(&c.create_message(&cp, &cs, &[round]).expect("message"));
        assert_eq!(process(&mut j, &jp, &m), Some(vec![round]));
        let m = wire(&j.create_message(&jp, &js, &[round, 1]).expect("message"));
        assert_eq!(process(&mut c, &cp, &m), Some(vec![round, 1]));
    }
    let ec = c.export_secret(cp.crypto(), "post/group-access/v1", &[], 32).expect("export");
    let ej = j.export_secret(jp.crypto(), "post/group-access/v1", &[], 32).expect("export");
    assert_eq!(ec, ej);
}

#[test]
fn libcrux_creates_rustcrypto_joins() {
    round_trip::<openmls_libcrux_crypto::Provider, openmls_rust_crypto::OpenMlsRustCrypto>();
}

#[test]
fn rustcrypto_creates_libcrux_joins() {
    round_trip::<openmls_rust_crypto::OpenMlsRustCrypto, openmls_libcrux_crypto::Provider>();
}
