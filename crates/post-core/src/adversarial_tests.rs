//! A malicious member or server against the group policy every client
//! enforces (design section 6, "Group policy"; section 15, "Testing").
//!
//! MLS lets any member commit anything, so these build real, valid MLS
//! commits and proposals that break Post's rules, and check that an honest
//! member's client refuses them. A refusal is either `Reject` or
//! `NeedDirectory` (which, after one directory refresh, becomes a refusal).

use openmls_libcrux_crypto::Provider as Memory;
use post_mls::HistoryVisibility;

use super::*;

const DOMAIN: &str = "chat.example.org";

/// One device of one account, with an in-memory MLS provider.
struct Dev {
    keys: DeviceKeys,
    signer: SignatureKeyPair,
    account: AccountId,
    provider: Memory,
}

impl Dev {
    fn new(chains: &mut HashMap<AccountId, Chain>, username: &str) -> Self {
        let keys = DeviceKeys::generate().expect("keys");
        let (chain, _) = Chain::create(&keys, DOMAIN, username, false, 1).expect("chain");
        let account = chain.account_id();
        chains.insert(account, chain);
        Self::with_keys(keys, account)
    }

    fn with_keys(keys: DeviceKeys, account: AccountId) -> Self {
        let signer = SignatureKeyPair::from_raw(SignatureScheme::ED25519, keys.mls.to_bytes().to_vec(), keys.mls.verifying_key().to_bytes().to_vec());
        Self { keys, signer, account, provider: Memory::default() }
    }

    fn credential(&self) -> CredentialWithKey {
        CredentialWithKey { credential: post_mls::credential(&self.account, &self.keys.device_id()), signature_key: self.signer.to_public_vec().into() }
    }

    fn key_package(&self) -> KeyPackage {
        KeyPackage::builder()
            .leaf_node_capabilities(post_mls::capabilities())
            .build(SUITE, &self.provider, &self.signer, self.credential())
            .expect("key package")
            .key_package()
            .clone()
    }

    /// Create a group with `meta` holding `others`, and have them join.
    fn create(&self, meta: &GroupMeta, others: &[&Dev]) -> (MlsGroup, Vec<MlsGroup>) {
        let mut group = post_mls::group_builder(meta).expect("builder").build(&self.provider, &self.signer, self.credential()).expect("group");
        let kps: Vec<KeyPackage> = others.iter().map(|d| d.key_package()).collect();
        let (_, welcome, _) = group.add_members(&self.provider, &self.signer, &kps).expect("add");
        group.merge_pending_commit(&self.provider).expect("merge");
        let bytes = welcome.tls_serialize_detached().expect("welcome");
        let joined = others.iter().map(|d| d.join(&bytes)).collect();
        (group, joined)
    }

    fn join(&self, welcome: &[u8]) -> MlsGroup {
        let MlsMessageBodyIn::Welcome(w) = MlsMessageIn::tls_deserialize_exact(welcome).expect("decode").extract() else { panic!("not a welcome") };
        StagedWelcome::new_from_welcome(&self.provider, &post_mls::join_config(), w, None).expect("stage").into_group(&self.provider).expect("join")
    }
}

/// Add a second device to an account's chain.
fn second_device(chains: &mut HashMap<AccountId, Chain>, first: &Dev) -> Dev {
    let keys = DeviceKeys::generate().expect("keys");
    let chain = chains.get_mut(&first.account).expect("chain");
    let bytes = chain.next_link(first.keys.device_id(), Op::AddDevice { device: keys.entry(false) }, 2).expect("link");
    let raw = post_identity::chain::seal(&bytes, &first.keys.sign_link(&bytes).expect("sign"), Some(&keys.cosign_link(&bytes).expect("cosign")));
    chain.append(&raw).expect("append");
    Dev::with_keys(keys, first.account)
}

fn wire(m: &MlsMessageOut) -> Vec<u8> {
    m.tls_serialize_detached().expect("serialize")
}

/// What an honest member decides about a commit from `by`.
fn judge(honest: &mut MlsGroup, provider: &Memory, by: &AccountId, commit: &[u8], chains: &HashMap<AccountId, Chain>) -> std::result::Result<(), Fail> {
    let msg = MlsMessageIn::tls_deserialize_exact(commit).expect("decode").try_into_protocol_message().expect("protocol");
    let ProcessedMessageContent::StagedCommitMessage(staged) = honest.process_message(provider, msg).expect("valid MLS").into_content() else {
        panic!("not a commit")
    };
    check_commit(honest, &staged, by, chains)
}

fn refused(r: std::result::Result<(), Fail>) -> bool {
    matches!(r, Err(Fail::Reject(_) | Fail::NeedDirectory))
}

/// Build a commit in `group` as `dev`, then forget it, so the next attempt
/// starts from the same epoch.
fn craft(group: &mut MlsGroup, dev: &Dev, build: impl FnOnce(CommitBuilder<'_, openmls::group::Initial>) -> CommitBuilder<'_, openmls::group::Initial>) -> Vec<u8> {
    let bundle = build(group.commit_builder())
        .load_psks(dev.provider.storage())
        .expect("psks")
        .build(dev.provider.rand(), dev.provider.crypto(), &dev.signer, |_| true)
        .expect("build")
        .stage_commit(&dev.provider)
        .expect("stage");
    let bytes = wire(bundle.commit());
    group.clear_pending_commit(dev.provider.storage()).expect("clear");
    bytes
}

fn meta_ext(meta: &GroupMeta) -> Extensions<GroupContext> {
    post_mls::group_context_extensions(meta).expect("extensions")
}

#[test]
fn members_cannot_overstep_in_a_group() {
    let mut chains = HashMap::new();
    let alice = Dev::new(&mut chains, "alice");
    let bob = Dev::new(&mut chains, "bob");
    let carol = Dev::new(&mut chains, "carol");
    let dave = Dev::new(&mut chains, "dave");
    let meta = GroupMeta::group("club", &alice.account, HistoryVisibility::Shared, 0);
    let (mut a, mut joined) = alice.create(&meta, &[&bob, &carol]);
    let mut b = joined.remove(0);
    let carol_leaf = a.members().find(|m| leaf_of_member(m).is_ok_and(|l| l.account == carol.account)).expect("carol").index;

    // Bob, not an admin, tries to add someone, rename the group, make
    // himself admin, and remove Carol.
    let add_dave = craft(&mut b, &bob, |c| c.propose_adds([dave.key_package()]));
    assert!(refused(judge(&mut a, &alice.provider, &bob.account, &add_dave, &chains)), "non-admin added a person");
    let renamed = GroupMeta { name: "bob's club".into(), ..meta.clone() };
    let rename = craft(&mut b, &bob, |c| c.propose_group_context_extensions(meta_ext(&renamed)).expect("gce"));
    assert!(refused(judge(&mut a, &alice.provider, &bob.account, &rename, &chains)), "non-admin renamed");
    let mut coup = meta.clone();
    coup.admins.push(bob.account.0.to_vec());
    let coup = craft(&mut b, &bob, |c| c.propose_group_context_extensions(meta_ext(&coup)).expect("gce"));
    assert!(refused(judge(&mut a, &alice.provider, &bob.account, &coup, &chains)), "non-admin made himself admin");
    let kick = craft(&mut b, &bob, |c| c.propose_removals([carol_leaf]));
    assert!(refused(judge(&mut a, &alice.provider, &bob.account, &kick, &chains)), "non-admin removed someone");

    // What Bob may do: refresh his own leaf.
    let refresh = craft(&mut b, &bob, |c| c.force_self_update(true));
    assert!(judge(&mut a, &alice.provider, &bob.account, &refresh, &chains).is_ok());

    // Even an admin may not turn the group into a DM, or leave it with no
    // admin among its members.
    let as_dm = GroupMeta { kind: MlsKind::Dm as i32, name: String::new(), history: 0, history_days: 0, admins: vec![alice.account.0.to_vec(), bob.account.0.to_vec()] };
    let mut a2 = carol_view(&alice, &bob, &carol, &meta);
    let to_dm = craft(&mut a2.0, &alice, |c| c.propose_group_context_extensions(meta_ext(&as_dm)).expect("gce"));
    assert!(refused(judge(&mut a2.1, &carol.provider, &alice.account, &to_dm, &chains)), "admin turned a group into a DM");
    let orphaned = GroupMeta { admins: vec![dave.account.0.to_vec()], ..meta.clone() };
    let orphan = craft(&mut a2.0, &alice, |c| c.propose_group_context_extensions(meta_ext(&orphaned)).expect("gce"));
    assert!(refused(judge(&mut a2.1, &carol.provider, &alice.account, &orphan, &chains)), "admin left the group with no admin");

    // The admin's legitimate changes pass.
    let renamed_by_admin = craft(&mut a2.0, &alice, |c| c.propose_group_context_extensions(meta_ext(&renamed)).expect("gce"));
    assert!(judge(&mut a2.1, &carol.provider, &alice.account, &renamed_by_admin, &chains).is_ok());
    let admin_adds = craft(&mut a2.0, &alice, |c| c.propose_adds([dave.key_package()]));
    assert!(judge(&mut a2.1, &carol.provider, &alice.account, &admin_adds, &chains).is_ok());
}

/// A fresh group where Alice is admin, as Alice and Carol see it.
fn carol_view(alice: &Dev, bob: &Dev, carol: &Dev, meta: &GroupMeta) -> (MlsGroup, MlsGroup) {
    let (a, mut joined) = alice.create(meta, &[bob, carol]);
    let c = joined.remove(1);
    (a, c)
}

#[test]
fn devices_must_be_in_their_accounts_chain() {
    let mut chains = HashMap::new();
    let alice = Dev::new(&mut chains, "alice");
    let bob = Dev::new(&mut chains, "bob");
    let meta = GroupMeta::dm(&alice.account, &bob.account);
    let (mut a, mut joined) = alice.create(&meta, &[&bob]);
    let mut b = joined.remove(0);

    // Bob slips in a second "device" of his that his chain does not list.
    let rogue = Dev::with_keys(DeviceKeys::generate().expect("keys"), bob.account);
    let sneak = craft(&mut b, &bob, |c| c.propose_adds([rogue.key_package()]));
    assert!(refused(judge(&mut a, &alice.provider, &bob.account, &sneak, &chains)), "unlisted device added");

    // Once the chain lists it, the same kind of add is fine.
    let phone = second_device(&mut chains, &bob);
    let add_phone = craft(&mut b, &bob, |c| c.propose_adds([phone.key_package()]));
    assert!(judge(&mut a, &alice.provider, &bob.account, &add_phone, &chains).is_ok());

    // A DM never gains a third account, whoever proposes it.
    let carol = Dev::new(&mut chains, "carol");
    let third = craft(&mut b, &bob, |c| c.propose_adds([carol.key_package()]));
    assert!(refused(judge(&mut a, &alice.provider, &bob.account, &third, &chains)), "third person in a DM");
}

#[test]
fn self_groups_hold_only_their_account() {
    let mut chains = HashMap::new();
    let alice = Dev::new(&mut chains, "alice");
    let mallory = Dev::new(&mut chains, "mallory");
    let phone = second_device(&mut chains, &alice);
    let meta = GroupMeta::self_group(&alice.account);
    let (mut laptop, mut joined) = alice.create(&meta, &[&phone]);
    let mut p = joined.remove(0);

    // Even the account's own device cannot add another account, or rename it.
    let add = craft(&mut p, &phone, |c| c.propose_adds([mallory.key_package()]));
    assert!(refused(judge(&mut laptop, &alice.provider, &alice.account, &add, &chains)));
    let named = GroupMeta { name: "mine".into(), ..meta.clone() };
    let rename = craft(&mut p, &phone, |c| c.propose_group_context_extensions(meta_ext(&named)).expect("gce"));
    assert!(refused(judge(&mut laptop, &alice.provider, &alice.account, &rename, &chains)));
}

#[test]
fn members_may_propose_removing_only_themselves() {
    let mut chains = HashMap::new();
    let alice = Dev::new(&mut chains, "alice");
    let bob = Dev::new(&mut chains, "bob");
    let carol = Dev::new(&mut chains, "carol");
    let meta = GroupMeta::group("club", &alice.account, HistoryVisibility::Shared, 0);
    let (mut a, mut joined) = alice.create(&meta, &[&bob, &carol]);
    let mut b = joined.remove(0);
    let leaf_of = |g: &MlsGroup, who: &AccountId| g.members().find(|m| leaf_of_member(m).is_ok_and(|l| l.account == *who)).expect("member").index;
    let carol_leaf = leaf_of(&a, &carol.account);
    let bob_leaf = leaf_of(&a, &bob.account);

    let mut judge_proposal = |target: LeafNodeIndex| {
        let (msg, _) = b.propose_remove_member(&bob.provider, &bob.signer, target).expect("propose");
        b.clear_pending_proposals(bob.provider.storage()).expect("clear");
        let msg = MlsMessageIn::tls_deserialize_exact(wire(&msg)).expect("decode").try_into_protocol_message().expect("protocol");
        let ProcessedMessageContent::ProposalMessage(p) = a.process_message(&alice.provider, msg).expect("valid MLS").into_content() else {
            panic!("not a proposal")
        };
        check_proposal(&a, &bob.account, &p)
    };
    assert!(refused(judge_proposal(carol_leaf)), "bob proposed removing carol");
    assert!(judge_proposal(bob_leaf).is_ok(), "bob may leave");
}

/// Carol's device, keeping its state in a real (in-memory) profile database.
fn carol_with_database(chains: &mut HashMap<AccountId, Chain>) -> (Me, Connection) {
    let carol_keys = DeviceKeys::generate().expect("keys");
    let (carol_chain, _) = Chain::create(&carol_keys, DOMAIN, "carol", false, 1).expect("chain");
    let carol_account = carol_chain.account_id();
    chains.insert(carol_account, carol_chain);
    let me = Me {
        signer: SignatureKeyPair::from_raw(SignatureScheme::ED25519, carol_keys.mls.to_bytes().to_vec(), carol_keys.mls.verifying_key().to_bytes().to_vec()),
        device: carol_keys.device_id(),
        keys: carol_keys,
        account: carol_account,
        username: "carol".into(),
        domain: DOMAIN.into(),
        url: format!("wss://{DOMAIN}/v1/ws"),
        bot: false,
    };
    let mut conn = Connection::open_in_memory().expect("db");
    store::migrate(&mut conn).expect("migrate");
    (me, conn)
}

fn database_key_package(me: &Me, conn: &mut Connection) -> KeyPackage {
    let tx = conn.transaction().expect("tx");
    let kp = {
        let provider = Provider::on(&tx);
        KeyPackage::builder().leaf_node_capabilities(post_mls::capabilities()).build(SUITE, &provider, &me.signer, me.credential()).expect("kp").key_package().clone()
    };
    tx.commit().expect("commit");
    kp
}

#[test]
fn a_welcome_listing_an_unlisted_device_is_refused() {
    let mut chains = HashMap::new();
    let alice = Dev::new(&mut chains, "alice");
    let (me, mut conn) = carol_with_database(&mut chains);
    // Alice's group includes a "device" of hers that her chain does not list,
    // as a server or a member slipping in an eavesdropper would.
    let rogue = Dev::with_keys(DeviceKeys::generate().expect("keys"), alice.account);
    let meta = GroupMeta::group("bugged", &alice.account, HistoryVisibility::Shared, 0);
    let mut g = post_mls::group_builder(&meta).expect("b").build(&alice.provider, &alice.signer, alice.credential()).expect("g");
    let kp = database_key_package(&me, &mut conn);
    let (_, welcome, _) = g.add_members(&alice.provider, &alice.signer, &[kp, rogue.key_package()]).expect("add");
    let tx = conn.transaction().expect("tx");
    assert!(matches!(join_from_welcome(&tx, &me, &chains, &wire(&welcome)), Err(Fail::NeedDirectory)));
}

#[test]
fn a_forged_welcome_cannot_replace_a_group_in_use() {
    let mut chains = HashMap::new();
    let alice = Dev::new(&mut chains, "alice");
    let mallory = Dev::new(&mut chains, "mallory");
    let (me, mut conn) = carol_with_database(&mut chains);
    let carol_kp = |conn: &mut Connection| database_key_package(&me, conn);

    // Alice adds Carol to a real group, and Carol joins it.
    let gid = [42u8; 16];
    let meta = GroupMeta::group("real", &alice.account, HistoryVisibility::Shared, 0);
    let mut real = post_mls::group_builder(&meta).expect("b").with_group_id(GroupId::from_slice(&gid)).build(&alice.provider, &alice.signer, alice.credential()).expect("g");
    let (_, welcome, _) = real.add_members(&alice.provider, &alice.signer, &[carol_kp(&mut conn)]).expect("add");
    let join = |conn: &mut Connection, bytes: &[u8]| {
        let tx = conn.transaction().expect("tx");
        let r = join_from_welcome(&tx, &me, &chains, bytes);
        if r.is_ok() {
            tx.commit().expect("commit");
        }
        r.ok().flatten()
    };
    assert_eq!(join(&mut conn, &wire(&welcome)), Some(gid));
    let epoch_of = |conn: &Connection| load_group(&Provider::on(conn), &gid).expect("group").epoch().as_u64();
    let real_epoch = epoch_of(&conn);

    // Mallory claims another of Carol's KeyPackages and builds a group with
    // the same ID, at a later epoch, inviting Carol.
    let mut fake = post_mls::group_builder(&GroupMeta::group("fake", &mallory.account, HistoryVisibility::Shared, 0))
        .expect("b")
        .with_group_id(GroupId::from_slice(&gid))
        .build(&mallory.provider, &mallory.signer, mallory.credential())
        .expect("g");
    for _ in 0..3 {
        fake.self_update(&mallory.provider, &mallory.signer, LeafNodeParameters::default()).expect("update");
        fake.merge_pending_commit(&mallory.provider).expect("merge");
    }
    let (_, forged, _) = fake.add_members(&mallory.provider, &mallory.signer, &[carol_kp(&mut conn)]).expect("add");
    assert_eq!(join(&mut conn, &wire(&forged)), None, "a Welcome for a group in use is ignored");
    assert_eq!(epoch_of(&conn), real_epoch, "the real group's state is untouched");
    let name: String = conn.query_row("SELECT name FROM conversations WHERE group_id = ?1", [&gid[..]], |r| r.get(0)).expect("name");
    assert_eq!(name, "real");
}
