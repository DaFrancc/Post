//! Spike 1, interop: a libcrux-backed device and a RustCrypto-backed device in
//! one group. The libcrux provider accepts the hybrid suite but does not list
//! it, so upstream does not test it there. Agreement with an independent
//! implementation of X-Wing is the check.

use openmls::prelude::{tls_codec::*, *};
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::OpenMlsProvider;

const SUITE: Ciphersuite = Ciphersuite::MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519;

fn setup<P: OpenMlsProvider + Default>(name: &str) -> (P, SignatureKeyPair, CredentialWithKey) {
    let p = P::default();
    let s = SignatureKeyPair::new(SUITE.signature_algorithm()).unwrap();
    s.store(p.storage()).unwrap();
    let c = CredentialWithKey {
        credential: BasicCredential::new(name.as_bytes().to_vec()).into(),
        signature_key: s.to_public_vec().into(),
    };
    (p, s, c)
}
fn wire(m: &MlsMessageOut) -> Vec<u8> { m.tls_serialize_detached().unwrap() }
fn proto(b: &[u8]) -> ProtocolMessage {
    MlsMessageIn::tls_deserialize_exact(b).unwrap().try_into_protocol_message().unwrap()
}
fn process<P: OpenMlsProvider>(g: &mut MlsGroup, p: &P, b: &[u8]) -> Option<Vec<u8>> {
    match g.process_message(p, proto(b)).unwrap().into_content() {
        ProcessedMessageContent::StagedCommitMessage(c) => { g.merge_staged_commit(p, *c).unwrap(); None }
        ProcessedMessageContent::ApplicationMessage(m) => Some(m.into_bytes()),
        _ => panic!("unexpected"),
    }
}

fn main() {
    let (lp, ls, lc) = setup::<openmls_libcrux_crypto::Provider>("libcrux-device");
    let (rp, rs, rc) = setup::<openmls_rust_crypto::OpenMlsRustCrypto>("rustcrypto-device");
    let cfg = MlsGroupJoinConfig::builder().wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY).use_ratchet_tree_extension(true).build();

    for libcrux_creates in [true, false] {
        // Creator adds the other; both then refresh keys and exchange messages.
        let (mut a, mut b);
        if libcrux_creates {
            let kp = KeyPackage::builder().build(SUITE, &rp, &rs, rc.clone()).unwrap().key_package().clone();
            a = MlsGroup::builder().ciphersuite(SUITE).with_wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY).use_ratchet_tree_extension(true).build(&lp, &ls, lc.clone()).unwrap();
            let (_, w, _) = a.add_members(&lp, &ls, &[kp]).unwrap();
            a.merge_pending_commit(&lp).unwrap();
            let w = match MlsMessageIn::tls_deserialize_exact(&wire(&w)).unwrap().extract() { MlsMessageBodyIn::Welcome(w) => w, _ => unreachable!() };
            b = StagedWelcome::new_from_welcome(&rp, &cfg, w, None).unwrap().into_group(&rp).unwrap();
        } else {
            let kp = KeyPackage::builder().build(SUITE, &lp, &ls, lc.clone()).unwrap().key_package().clone();
            b = MlsGroup::builder().ciphersuite(SUITE).with_wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY).use_ratchet_tree_extension(true).build(&rp, &rs, rc.clone()).unwrap();
            let (_, w, _) = b.add_members(&rp, &rs, &[kp]).unwrap();
            b.merge_pending_commit(&rp).unwrap();
            let w = match MlsMessageIn::tls_deserialize_exact(&wire(&w)).unwrap().extract() { MlsMessageBodyIn::Welcome(w) => w, _ => unreachable!() };
            a = StagedWelcome::new_from_welcome(&lp, &cfg, w, None).unwrap().into_group(&lp).unwrap();
        }
        // a = libcrux, b = rustcrypto
        for round in 0..3 {
            let c = wire(a.self_update(&lp, &ls, LeafNodeParameters::default()).unwrap().commit());
            a.merge_pending_commit(&lp).unwrap();
            process(&mut b, &rp, &c);
            let c = wire(b.self_update(&rp, &rs, LeafNodeParameters::default()).unwrap().commit());
            b.merge_pending_commit(&rp).unwrap();
            process(&mut a, &lp, &c);
            let m = wire(&a.create_message(&lp, &ls, format!("l{round}").as_bytes()).unwrap());
            assert_eq!(process(&mut b, &rp, &m).unwrap(), format!("l{round}").into_bytes());
            let m = wire(&b.create_message(&rp, &rs, format!("r{round}").as_bytes()).unwrap());
            assert_eq!(process(&mut a, &lp, &m).unwrap(), format!("r{round}").into_bytes());
        }
        let ea = a.export_secret(lp.crypto(), "x", b"", 32).unwrap();
        let eb = b.export_secret(rp.crypto(), "x", b"", 32).unwrap();
        assert_eq!(ea, eb);
        println!("creator={}: welcome, 6 key-refresh commits, 6 messages, exporter agreement: ok",
                 if libcrux_creates { "libcrux" } else { "rustcrypto" });
    }
}
