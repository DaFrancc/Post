//! Device chain rules (design section 5). Each test builds a real chain and
//! checks one rule, from the point of view of a client verifying links it got
//! from an untrusted server.

use postern_identity::chain::{LINK_VERSION, MAX_ACTIVE_DEVICES, seal, validate_domain, validate_username};
use postern_identity::{Chain, DeviceKeys, Error, Link, Op};

const DOMAIN: &str = "chat.example.org";

fn keys() -> DeviceKeys {
    DeviceKeys::generate().expect("rng")
}

fn new_account(username: &str) -> (Chain, DeviceKeys) {
    let k = keys();
    let (chain, _) = Chain::create(&k, DOMAIN, username, false, 1).expect("create");
    (chain, k)
}

/// Add `new` to `chain`, signed by `by`. `label` names the device in failures.
/// Returns the signed link.
fn add(chain: &mut Chain, by: &DeviceKeys, new: &DeviceKeys, label: &str) -> Vec<u8> {
    let op = Op::AddDevice { device: new.entry(chain.is_bot()) };
    let bytes = chain.next_link(by.device_id(), op, 2).unwrap_or_else(|e| panic!("adding {label}: {e}"));
    let raw = seal(&bytes, &by.sign_link(&bytes).expect("sign"), Some(&new.cosign_link(&bytes).expect("cosign")));
    chain.append(&raw).unwrap_or_else(|e| panic!("adding {label}: {e}"));
    raw
}

/// A link with every field chosen by the test, bypassing `next_link`'s checks,
/// so tests can present what a malicious server or device might send.
fn raw_link(chain: &Chain, signer: &DeviceKeys, op: Op, cosigner: Option<&DeviceKeys>) -> Vec<u8> {
    let link = Link {
        version: LINK_VERSION,
        account_id: Some(chain.account_id()),
        seq: chain.next_seq(),
        prev_hash: Some(chain.head()),
        timestamp_ms: 3,
        signer: signer.device_id(),
        op,
    };
    let bytes = link.encode();
    let cosig = cosigner.map(|c| c.cosign_link(&bytes).expect("cosign"));
    seal(&bytes, &signer.sign_link(&bytes).expect("sign"), cosig.as_ref())
}

/// Append must fail with `want` and leave the chain exactly as it was.
/// A macro so the link can be built from the chain before it is borrowed mutably.
macro_rules! assert_rejected {
    ($chain:ident, $raw:expr, $want:expr) => {{
        let raw: Vec<u8> = ($raw).to_vec();
        check_rejected(&mut $chain, &raw, $want)
    }};
}

fn check_rejected(chain: &mut Chain, raw: &[u8], want: Error) {
    let before = (chain.next_seq(), chain.head(), chain.devices().to_vec());
    assert_eq!(chain.append(raw), Err(want));
    assert_eq!((chain.next_seq(), chain.head(), chain.devices().to_vec()), before);
}

fn verify_fresh(chain: &Chain) -> Chain {
    Chain::from_links(chain.links().iter().map(Vec::as_slice)).expect("fresh verification")
}

#[test]
fn create_and_verify() {
    let (chain, k) = new_account("alice");
    assert_eq!(chain.account_id(), chain.head());
    assert_eq!(chain.username(), "alice");
    assert_eq!(chain.server_domain(), DOMAIN);
    assert_eq!(chain.next_seq(), 1);
    assert!(chain.authorizes_leaf(&k.device_id(), &k.mls.verifying_key().to_bytes()));

    let other = verify_fresh(&chain);
    assert_eq!(other.account_id(), chain.account_id());
    assert_eq!(other.devices(), chain.devices());
}

#[test]
fn account_ids_are_unique() {
    assert_ne!(new_account("alice").0.account_id(), new_account("alice").0.account_id());
}

#[test]
fn add_device_with_countersignature() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    add(&mut chain, &laptop, &phone, "phone");
    assert_eq!(chain.active_devices().count(), 2);
    assert!(chain.authorizes_leaf(&phone.device_id(), &phone.mls.verifying_key().to_bytes()));

    // The new device can now sign.
    let tablet = keys();
    add(&mut chain, &phone, &tablet, "tablet");
    assert_eq!(verify_fresh(&chain).active_devices().count(), 3);
}

#[test]
fn add_requires_valid_countersignature() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    let op = || Op::AddDevice { device: phone.entry(false) };
    assert_rejected!(chain, &raw_link(&chain, &laptop, op(), None), Error::MissingCosignature);
    assert_rejected!(chain, &raw_link(&chain, &laptop, op(), Some(&keys())), Error::BadCosignature);
    // A countersignature made with the link-signing label is not a countersignature.
    let bytes = chain.next_link(laptop.device_id(), op(), 2).expect("next");
    let wrong_label = seal(&bytes, &laptop.sign_link(&bytes).expect("s"), Some(&phone.sign_link(&bytes).expect("s")));
    assert_rejected!(chain, &wrong_label, Error::BadCosignature);
}

#[test]
fn countersignature_only_on_add() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    add(&mut chain, &laptop, &phone, "phone");
    let op = Op::RemoveDevice { device_id: phone.device_id() };
    assert_rejected!(chain, &raw_link(&chain, &laptop, op, Some(&phone)), Error::UnexpectedCosignature);
}

#[test]
fn outsiders_cannot_sign() {
    let (mut chain, _) = new_account("alice");
    let mallory = keys();
    let op = Op::AddDevice { device: mallory.entry(false) };
    assert_rejected!(chain, &raw_link(&chain, &mallory, op, Some(&mallory)), Error::UnknownSigner);
}

#[test]
fn signature_must_match_named_signer() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    add(&mut chain, &laptop, &phone, "phone");
    // Names the laptop as signer, but signed with an outsider's key.
    let mallory = keys();
    let link = Link {
        version: LINK_VERSION,
        account_id: Some(chain.account_id()),
        seq: chain.next_seq(),
        prev_hash: Some(chain.head()),
        timestamp_ms: 3,
        signer: laptop.device_id(),
        op: Op::RemoveDevice { device_id: phone.device_id() },
    };
    let bytes = link.encode();
    let raw = seal(&bytes, &mallory.sign_link(&bytes).expect("s"), None);
    assert_rejected!(chain, &raw, Error::BadSignature);
}

#[test]
fn removed_devices_cannot_sign() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    add(&mut chain, &laptop, &phone, "stolen phone");
    chain.sign_and_append(&laptop, Op::RemoveDevice { device_id: phone.device_id() }, 3).expect("remove");
    assert!(!chain.authorizes_leaf(&phone.device_id(), &phone.mls.verifying_key().to_bytes()));

    let op = Op::RemoveDevice { device_id: laptop.device_id() };
    assert_rejected!(chain, &raw_link(&chain, &phone, op, None), Error::SignerRemoved);
    let thief = keys();
    let op = Op::AddDevice { device: thief.entry(false) };
    assert_rejected!(chain, &raw_link(&chain, &phone, op, Some(&thief)), Error::SignerRemoved);
}

#[test]
fn removal_rules() {
    let (mut chain, laptop) = new_account("alice");
    let lone = raw_link(&chain, &laptop, Op::RemoveDevice { device_id: laptop.device_id() }, None);
    assert_rejected!(chain, &lone, Error::LastDevice);

    let phone = keys();
    add(&mut chain, &laptop, &phone, "phone");
    let unknown = raw_link(&chain, &laptop, Op::RemoveDevice { device_id: keys().device_id() }, None);
    assert_rejected!(chain, &unknown, Error::UnknownDevice);

    // A device may remove itself.
    chain.sign_and_append(&phone, Op::RemoveDevice { device_id: phone.device_id() }, 3).expect("self removal");
    assert_eq!(chain.active_devices().count(), 1);

    let twice = raw_link(&chain, &laptop, Op::RemoveDevice { device_id: phone.device_id() }, None);
    assert_rejected!(chain, &twice, Error::DeviceRemoved);
}

#[test]
fn removed_devices_cannot_return() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    add(&mut chain, &laptop, &phone, "phone");
    chain.sign_and_append(&laptop, Op::RemoveDevice { device_id: phone.device_id() }, 3).expect("remove");
    let op = Op::AddDevice { device: phone.entry(false) };
    assert_rejected!(chain, &raw_link(&chain, &laptop, op, Some(&phone)), Error::AlreadyPresent);
}

#[test]
fn duplicate_devices_and_mls_keys_rejected() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    add(&mut chain, &laptop, &phone, "phone");
    let op = Op::AddDevice { device: phone.entry(false) };
    assert_rejected!(chain, &raw_link(&chain, &laptop, op, Some(&phone)), Error::AlreadyPresent);

    // A new device reusing an existing MLS key.
    let copycat = keys();
    let mut entry = copycat.entry(false);
    entry.mls_sig_pub = phone.mls.verifying_key().to_bytes();
    let op = Op::AddDevice { device: entry };
    assert_rejected!(chain, &raw_link(&chain, &laptop, op, Some(&copycat)), Error::AlreadyPresent);
}

#[test]
fn rotate_mls_key() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    add(&mut chain, &laptop, &phone, "phone");
    let old = phone.mls.verifying_key().to_bytes();
    let new = keys().mls.verifying_key().to_bytes();

    // Only the device itself may rotate its key.
    let op = Op::RotateMlsKey { device_id: phone.device_id(), mls_sig_pub: new };
    assert_rejected!(chain, &raw_link(&chain, &laptop, op.clone(), None), Error::SignerNotAllowed);

    chain.sign_and_append(&phone, op, 3).expect("rotate");
    assert!(chain.authorizes_leaf(&phone.device_id(), &new));
    assert!(!chain.authorizes_leaf(&phone.device_id(), &old));

    // Rotating back to a key that was ever used is refused.
    let back = Op::RotateMlsKey { device_id: phone.device_id(), mls_sig_pub: old };
    assert_rejected!(chain, &raw_link(&chain, &phone, back, None), Error::AlreadyPresent);
    assert_eq!(verify_fresh(&chain).devices(), chain.devices());
}

#[test]
fn device_limit() {
    let (mut chain, laptop) = new_account("alice");
    for i in 1..MAX_ACTIVE_DEVICES {
        add(&mut chain, &laptop, &keys(), &format!("device {i}"));
    }
    assert_eq!(chain.active_devices().count(), MAX_ACTIVE_DEVICES);
    let extra = keys();
    let op = Op::AddDevice { device: extra.entry(false) };
    assert_rejected!(chain, &raw_link(&chain, &laptop, op, Some(&extra)), Error::TooManyDevices);
}

#[test]
fn links_must_extend_the_head() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    let first = add(&mut chain, &laptop, &phone, "phone");

    // Replaying an old link.
    assert_rejected!(chain, &first, Error::WrongSequence { expected: 2, got: 1 });

    let op = || Op::RemoveDevice { device_id: phone.device_id() };
    let mut link = Link {
        version: LINK_VERSION,
        account_id: Some(chain.account_id()),
        seq: chain.next_seq(),
        prev_hash: Some(chain.account_id()), // points at genesis, not the head
        timestamp_ms: 3,
        signer: laptop.device_id(),
        op: op(),
    };
    let sealed = |l: &Link| {
        let b = l.encode();
        seal(&b, &laptop.sign_link(&b).expect("s"), None)
    };
    assert_rejected!(chain, &sealed(&link), Error::WrongPrevHash);

    link.prev_hash = Some(chain.head());
    link.seq = 7;
    assert_rejected!(chain, &sealed(&link), Error::WrongSequence { expected: 2, got: 7 });
}

#[test]
fn links_from_another_account_rejected() {
    let (mut alice, alice_laptop) = new_account("alice");
    let (bob, bob_laptop) = new_account("bob");
    // Bob's laptop signs a well-formed link for Bob's chain; offered to Alice's.
    let op = Op::AddDevice { device: keys().entry(false) };
    let _ = alice_laptop;
    let link = raw_link(&bob, &bob_laptop, op, None);
    assert!(matches!(alice.append(&link), Err(Error::WrongAccount | Error::WrongPrevHash)));
}

#[test]
fn genesis_rules() {
    let k = keys();
    let (_, genesis) = Chain::create(&k, DOMAIN, "alice", false, 1).expect("create");

    // Not a genesis link first.
    let (mut chain, laptop) = new_account("carol");
    let phone = keys();
    let second = add(&mut chain, &laptop, &phone, "phone");
    assert_eq!(Chain::from_genesis(&second).err(), Some(Error::NotGenesis));

    // Another account's genesis link offered later in a chain.
    assert_rejected!(chain, &genesis, Error::WrongSequence { expected: 2, got: 0 });

    // A genesis op claiming a later position is refused when decoded.
    let g = Link {
        version: LINK_VERSION,
        account_id: Some(chain.account_id()),
        seq: chain.next_seq(),
        prev_hash: Some(chain.head()),
        timestamp_ms: 1,
        signer: laptop.device_id(),
        op: Op::Genesis { server_domain: DOMAIN.into(), username: "carol".into(), device: keys().entry(false) },
    };
    let gb = g.encode();
    assert_rejected!(chain, seal(&gb, &laptop.sign_link(&gb).expect("s"), None), Error::Malformed("account id"));

    // Genesis signed by a key other than its device's.
    let entry = k.entry(false);
    let link = Link {
        version: LINK_VERSION,
        account_id: None,
        seq: 0,
        prev_hash: None,
        timestamp_ms: 1,
        signer: entry.device_id(),
        op: Op::Genesis { server_domain: DOMAIN.into(), username: "alice".into(), device: entry.clone() },
    };
    let bytes = link.encode();
    let forged = seal(&bytes, &keys().sign_link(&bytes).expect("s"), None);
    assert_eq!(Chain::from_genesis(&forged).err(), Some(Error::BadSignature));

    // Genesis naming a signer other than its device.
    let mut l2 = link.clone();
    l2.signer = keys().device_id();
    let b2 = l2.encode();
    assert_eq!(Chain::from_genesis(&seal(&b2, &k.sign_link(&b2).expect("s"), None)).err(), Some(Error::SignerNotAllowed));

    // Genesis with a countersignature.
    let cosigned = seal(&bytes, &k.sign_link(&bytes).expect("s"), Some(&k.cosign_link(&bytes).expect("s")));
    assert_eq!(Chain::from_genesis(&cosigned).err(), Some(Error::UnexpectedCosignature));

    // Bad username or domain, even when correctly signed.
    assert_eq!(Chain::create(&k, DOMAIN, "Alice", false, 1).err(), Some(Error::InvalidUsername));
    assert_eq!(Chain::create(&k, "Chat.Example.org", "alice", false, 1).err(), Some(Error::InvalidDomain));
}

#[test]
fn non_canonical_encodings_rejected() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    let op = Op::AddDevice { device: phone.entry(false) };
    let mut bytes = chain.next_link(laptop.device_id(), op, 2).expect("next");
    // Append an unknown field (tag 99, varint 1), then sign correctly.
    bytes.extend_from_slice(&[0x98, 0x06, 0x01]);
    let raw = seal(&bytes, &laptop.sign_link(&bytes).expect("s"), Some(&phone.cosign_link(&bytes).expect("s")));
    assert_rejected!(chain, &raw, Error::NonCanonical);

    // An unknown field in the envelope around an otherwise valid link.
    let mut good = {
        let op = Op::AddDevice { device: phone.entry(false) };
        let b = chain.next_link(laptop.device_id(), op, 2).expect("next");
        seal(&b, &laptop.sign_link(&b).expect("s"), Some(&phone.cosign_link(&b).expect("s")))
    };
    good.extend_from_slice(&[0x98, 0x06, 0x01]);
    assert_rejected!(chain, &good, Error::NonCanonical);
}

#[test]
fn unsupported_version_rejected() {
    let (mut chain, laptop) = new_account("alice");
    let phone = keys();
    add(&mut chain, &laptop, &phone, "phone");
    let link = Link {
        version: 2,
        account_id: Some(chain.account_id()),
        seq: chain.next_seq(),
        prev_hash: Some(chain.head()),
        timestamp_ms: 3,
        signer: laptop.device_id(),
        op: Op::RemoveDevice { device_id: phone.device_id() },
    };
    let b = link.encode();
    assert_rejected!(chain, &seal(&b, &laptop.sign_link(&b).expect("s"), None), Error::UnsupportedVersion(2));
}

#[test]
fn extend_from_merges_and_detects_forks() {
    let (mut alice_view, laptop) = new_account("alice");
    let genesis = alice_view.links()[0].clone();
    let phone = keys();
    add(&mut alice_view, &laptop, &phone, "phone");
    let tablet = keys();
    add(&mut alice_view, &laptop, &tablet, "tablet");

    // A contact who only has the genesis link catches up from the server.
    let mut contact = Chain::from_genesis(&genesis).expect("genesis");
    assert_eq!(contact.extend_from(0, alice_view.links()).expect("merge"), 2);
    assert_eq!(contact.head(), alice_view.head());
    // Offering the same links again changes nothing.
    assert_eq!(contact.extend_from(1, &alice_view.links()[1..]).expect("merge"), 0);

    // The same link re-signed (ML-DSA signatures are randomized) is not a fork.
    let mut resigned_view = Chain::from_genesis(&genesis).expect("genesis");
    let b1 = {
        let op = Op::AddDevice { device: phone.entry(false) };
        let link = Link {
            version: LINK_VERSION,
            account_id: Some(resigned_view.account_id()),
            seq: 1,
            prev_hash: Some(resigned_view.head()),
            timestamp_ms: 2,
            signer: laptop.device_id(),
            op,
        };
        link.encode()
    };
    let resigned = seal(&b1, &laptop.sign_link(&b1).expect("s"), Some(&phone.cosign_link(&b1).expect("s")));
    resigned_view.append(&resigned).expect("append");
    assert_ne!(resigned, alice_view.links()[1]);
    assert_eq!(resigned_view.extend_from(1, &alice_view.links()[1..]).expect("merge"), 1);

    // A different link at a sequence number we already hold is a fork. Here
    // the server shows the contact a branch where the phone was never added.
    let mut forked = Chain::from_genesis(&genesis).expect("genesis");
    let other = keys();
    add(&mut forked, &laptop, &other, "something else");
    assert_eq!(contact.extend_from(1, &forked.links()[1..]), Err(Error::Fork { seq: 1 }));

    // Links that start past our end leave a gap.
    let mut fresh = Chain::from_genesis(&genesis).expect("genesis");
    assert_eq!(fresh.extend_from(2, &alice_view.links()[2..]), Err(Error::Gap { have: 1, from: 2 }));
}

#[test]
fn username_and_domain_rules() {
    for ok in ["abc", "alice", "a_1", "z23456789012345678901234567890ab"] {
        validate_username(ok).unwrap_or_else(|_| panic!("{ok} should be valid"));
    }
    for bad in ["ab", "Alice", "1abc", "_abc", "al ice", "alice!", "ålice", "a23456789012345678901234567890abc"] {
        assert_eq!(validate_username(bad), Err(Error::InvalidUsername), "{bad}");
    }
    for ok in ["chat.example.org", "localhost", "localhost:8443", "a-b.example", "10.0.0.1:443"] {
        validate_domain(ok).unwrap_or_else(|_| panic!("{ok} should be valid"));
    }
    for bad in ["", "Chat.example.org", "-a.example", "a..b", "a.example.", "host:0", "host:080", "host:99999", "host:", "a b"] {
        assert_eq!(validate_domain(bad), Err(Error::InvalidDomain), "{bad}");
    }
}

#[test]
fn bot_status_is_fixed_by_genesis() {
    // A bot account adds only bot devices.
    let bot_k = keys();
    let (mut bot, _) = Chain::create(&bot_k, DOMAIN, "weatherbot", true, 1).expect("create");
    assert!(bot.is_bot());
    let second = keys();
    let op = Op::AddDevice { device: second.entry(true) };
    let bytes = bot.next_link(bot_k.device_id(), op, 2).expect("next");
    bot.append(&seal(&bytes, &bot_k.sign_link(&bytes).expect("s"), Some(&second.cosign_link(&bytes).expect("s"))))
        .expect("bot adds bot device");
    let human_device = keys();
    let op = Op::AddDevice { device: human_device.entry(false) };
    assert_rejected!(bot, raw_link(&bot, &bot_k, op, Some(&human_device)), Error::BotMismatch);

    // A person's account never adds a bot device, so it cannot gain or lose a badge.
    let (mut alice, laptop) = new_account("alice");
    assert!(!alice.is_bot());
    let sneaky = keys();
    let op = Op::AddDevice { device: sneaky.entry(true) };
    assert_rejected!(alice, raw_link(&alice, &laptop, op, Some(&sneaky)), Error::BotMismatch);
    assert!(verify_fresh(&bot).is_bot());
}
