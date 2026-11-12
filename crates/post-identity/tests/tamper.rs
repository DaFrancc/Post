//! Property: changing any byte of any link in a valid chain makes the whole
//! chain fail verification. Covers link fields, signatures, countersignatures
//! and the envelope around them.

use std::sync::OnceLock;

use post_identity::chain::seal;
use post_identity::{Chain, DeviceKeys, Op};
use proptest::prelude::*;

/// Genesis, an added device, and a removal: every op type that carries
/// signatures, a countersignature, and a reference to another device.
fn chain() -> &'static Vec<Vec<u8>> {
    static LINKS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    LINKS.get_or_init(|| {
        let laptop = DeviceKeys::generate().expect("rng");
        let phone = DeviceKeys::generate().expect("rng");
        let (mut c, _) = Chain::create(&laptop, "chat.example.org", "alice", false, 1).expect("create");
        let op = Op::AddDevice { device: phone.entry(false) };
        let b = c.next_link(laptop.device_id(), op, 2).expect("next");
        c.append(&seal(&b, &laptop.sign_link(&b).expect("s"), Some(&phone.cosign_link(&b).expect("s")))).expect("add");
        let op = Op::RotateMlsKey { device_id: phone.device_id(), mls_sig_pub: DeviceKeys::generate().expect("rng").mls.verifying_key().to_bytes() };
        c.sign_and_append(&phone, op, 3).expect("rotate");
        c.sign_and_append(&phone, Op::RemoveDevice { device_id: laptop.device_id() }, 4).expect("remove");
        c.links().to_vec()
    })
}

fn verifies(links: &[Vec<u8>]) -> bool {
    Chain::from_links(links.iter().map(Vec::as_slice)).is_ok()
}

#[test]
fn untampered_chain_verifies() {
    assert!(verifies(chain()));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    #[test]
    fn any_byte_change_is_rejected(which in 0usize..4, pos in any::<prop::sample::Index>(), xor in 1u8..=255) {
        let mut links = chain().clone();
        let i = pos.index(links[which].len());
        links[which][i] ^= xor;
        prop_assert!(!verifies(&links), "link {which}, byte {i}, xor {xor:#04x} still verified");
    }

    #[test]
    fn truncation_is_rejected(which in 0usize..4, keep in any::<prop::sample::Index>()) {
        let mut links = chain().clone();
        let n = keep.index(links[which].len());
        links[which].truncate(n);
        prop_assert!(!verifies(&links));
    }

    #[test]
    fn dropping_or_reordering_links_is_rejected(a in 1usize..4, b in 1usize..4) {
        let mut links = chain().clone();
        if a == b {
            links.remove(a);
            // Dropping the last link leaves a valid shorter chain; that is the
            // withholding case, caught by chain-head gossip instead.
            prop_assume!(a != 3);
        } else {
            links.swap(a, b);
        }
        prop_assert!(!verifies(&links));
    }
}
