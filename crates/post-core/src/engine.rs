//! The client's protocol logic. One `Engine` owns the write connection to the
//! local database and both server connections; `Core` serializes access to
//! it, so MLS state and network ordering never race.
//!
//! Rules this file follows (design sections 6, 8, 11 and 14):
//! - Never hold a database transaction across a network call.
//! - Persist state before transmitting; advance a cursor only after the
//!   resulting state is committed.
//! - Conversation traffic goes on the anonymous connection, device traffic on
//!   the session. Never the other way round.
//! - Everything from the server is untrusted: chains, KeyPackages, Welcomes
//!   and commits are checked against the verified directory before use.

use std::collections::{HashMap, HashSet};

use blind_rsa_signatures::{BlindSignature, DefaultRng, PublicKeySha384PSSRandomized as TokenPublicKey};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::OpenMlsProvider;
use openmls_traits::types::SignatureScheme;
use post_identity::chain::validate_username;
use post_identity::{AccountId, Chain, DeviceEntry, DeviceId, DeviceKeys, HybridSignature, HybridSigningKey, Link, Op, labels};
use post_mls::{ConversationKind as MlsKind, GroupMeta, LeafIdentity, SUITE};
use post_proto::{self as proto, ErrorCode, Reply, Request};
use prost::Message as _;
use rusqlite::{Connection, Transaction};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use zeroize::Zeroizing;

use crate::content::{self, Body, Content};
use crate::{calls, ephemeral};
use crate::link::{self as linking, Channel, LinkCode};
use crate::net::{ServerConn, expect_reply};
use crate::provider::Provider;
use crate::store::{self, KIND_DM, KIND_GROUP, KIND_SELF, NewMessage};
use crate::{ConversationId, Error, Event, MessageId, Presence, PresenceSetting, Result};

/// Exporter label for group access keys (design section 8).
pub const ACCESS_LABEL: &str = "post/group-access/v1";
const KP_LOW: u32 = 10;
const KP_TARGET: u32 = 30;
const TOKEN_LOW: u32 = 20;
const TOKEN_BATCH: usize = 50;
/// Upper bound on fetch rounds in one sync, so a misbehaving server cannot
/// keep a client looping forever.
const MAX_FETCH_ROUNDS: usize = 10_000;
/// After a claim finds no KeyPackages for a device, wait this long before
/// spending another token on it.
const KP_BACKOFF: std::time::Duration = std::time::Duration::from_secs(600);
/// Whether a leaf belongs to a stale device whose account keeps another,
/// non-stale device in the same group. Its account's other devices, or a
/// group admin, remove such leaves (design section 6, "Stale devices"); an
/// account's last leaf never is, so an account stays a member while all its
/// devices are away.
fn stale_and_replaceable(stale: &HashSet<DeviceId>, leaves: &[(LeafNodeIndex, LeafIdentity)], leaf: &LeafIdentity) -> bool {
    stale.contains(&leaf.device) && leaves.iter().any(|(_, l)| l.account == leaf.account && l.device != leaf.device && !stale.contains(&l.device))
}

/// How often a device replaces its own leaf in each group (design section 6,
/// "Key refresh"). Each group is due somewhat earlier, by a spread derived
/// from its ID, so a device's refreshes do not all land at once.
const REFRESH_INTERVAL_MS: u64 = 7 * 24 * 3600 * 1000;
const REFRESH_SPREAD_MS: u64 = 24 * 3600 * 1000;

/// Whether this device's leaf in group `gid`, last replaced at
/// `refreshed_at`, should be replaced now. Leaves older than `force_before`
/// are always due.
fn refresh_due(gid: &[u8; 16], refreshed_at: u64, now: u64, force_before: u64) -> bool {
    let spread = u64::from_be_bytes([gid[0], gid[1], gid[2], gid[3], gid[4], gid[5], gid[6], gid[7]]) % REFRESH_SPREAD_MS;
    refreshed_at < force_before || now.saturating_sub(refreshed_at) >= REFRESH_INTERVAL_MS - spread
}

/// Typing signals go out at most this often per conversation.
const TYPING_EVERY: std::time::Duration = std::time::Duration::from_secs(3);
/// Someone silent this long is shown offline.
const PRESENCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25 * 60);
/// Groups quiet this long are not sent repeated presence.
const QUIET_GROUP_MS: u64 = 30 * 24 * 3600 * 1000;
/// Signals sent before pausing a second, under the server's 20 a second.
const SIGNALS_PER_BURST: usize = 15;

/// Most accounts in one group. Post serves a community of about 100.
const MAX_GROUP_ACCOUNTS: usize = 150;
/// Most KeyPackages the server holds for a device (its `MAX_KEY_PACKAGES`).
const KP_MAX: u32 = 100;
/// How long a new device waits for the user to enter its code on another
/// device: as long as the server keeps the rendezvous.
const LINK_WAIT_FOR_USER: std::time::Duration = std::time::Duration::from_secs(600);
/// How long either device waits for the other's next automatic step.
const LINK_WAIT_FOR_DEVICE: std::time::Duration = std::time::Duration::from_secs(180);

/// A link this unregistered device has offered, waiting for another device
/// of the account to accept it (design section 5, "Linking a device").
pub(crate) struct PendingLink {
    channel: Channel,
    conn: ServerConn,
    keys: DeviceKeys,
    url: String,
    domain: String,
    bot: bool,
}

/// A device asking to be linked, as the accepting device sees it.
pub(crate) struct LinkOffer {
    pub entry: DeviceEntry,
    pub name: String,
}

/// Membership changes one group needs.
struct Plan {
    gid: [u8; 16],
    /// This account's devices missing from the group.
    add: Vec<DeviceId>,
    /// Leaves of devices their accounts have removed.
    remove: Vec<LeafNodeIndex>,
    /// Members' proposals (people leaving) wait to be committed.
    proposals: bool,
    /// This device's leaf is due for fresh keys.
    refresh: bool,
}

/// KeyPackages claimed for one device. One-time packages are used first; the
/// last-resort package may then be used for every remaining group.
#[derive(Default)]
struct KpPool {
    one_time: Vec<KeyPackage>,
    last_resort: Option<KeyPackage>,
}

impl KpPool {
    fn take(&mut self) -> Option<KeyPackage> {
        self.one_time.pop().or_else(|| self.last_resort.clone())
    }

    fn is_empty(&self) -> bool {
        self.one_time.is_empty() && self.last_resort.is_none()
    }
}

/// What the listen loop waits on.
pub(crate) struct ListenHandles {
    pub wakes: [std::sync::Arc<tokio::sync::Notify>; 2],
    pub closed: [crate::net::Closed; 2],
    pub signals: std::sync::Arc<crate::net::Signals>,
}

/// This device's identity and keys.
pub(crate) struct Me {
    pub keys: DeviceKeys,
    pub signer: SignatureKeyPair,
    pub account: AccountId,
    pub device: DeviceId,
    pub username: String,
    pub domain: String,
    pub url: String,
    pub bot: bool,
}

impl Me {
    fn credential(&self) -> CredentialWithKey {
        CredentialWithKey {
            credential: post_mls::credential(&self.account, &self.device),
            signature_key: self.signer.to_public_vec().into(),
        }
    }
}

/// Why processing one input failed.
enum Fail {
    /// A member or signer is not in the verified directory, or is newer than
    /// it: refresh the directory and try once more.
    NeedDirectory,
    /// The input is invalid or breaks policy: record a notice and move on.
    Reject(String),
    /// Something is wrong locally: stop and report.
    Fatal(Error),
}

impl From<Error> for Fail {
    fn from(e: Error) -> Self {
        Fail::Fatal(e)
    }
}

impl From<rusqlite::Error> for Fail {
    fn from(e: rusqlite::Error) -> Self {
        Fail::Fatal(Error::Db(e))
    }
}

fn reject(msg: impl Into<String>) -> Fail {
    Fail::Reject(msg.into())
}

fn mls<E: std::fmt::Display>(e: E) -> Error {
    Error::Mls(e.to_string())
}

fn fatal<E: std::fmt::Display>(e: E) -> Fail {
    Fail::Fatal(mls(e))
}

/// The result of one attempt at processing an input, decided with the
/// transaction open and acted on after it is closed.
enum Step<T> {
    Done(T),
    /// Refresh the directory, then try again.
    Retry,
    /// Skip the input and tell the user why.
    Refused(String),
}

/// What a processed entry changed, for events after commit.
enum Outcome {
    Nothing,
    Added([u8; 16]),
    Changed([u8; 16]),
    Deleted([u8; 16]),
    Profile(AccountId),
    /// Values synced between this account's devices changed.
    SelfSync(SyncChanges),
    Conversation,
    /// A commit added accounts that were not members: they need our profile.
    NewMembers,
    /// A call event, sent or received.
    Call(calls::Seen),
}

fn random16() -> Result<[u8; 16]> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|_| Error::Rng)?;
    Ok(b)
}

fn group_id_of(b: &[u8]) -> Result<[u8; 16]> {
    b.try_into().map_err(|_| Error::Protocol("group id must be 16 bytes".into()))
}

fn load_group(provider: &Provider<'_>, gid: &[u8]) -> Result<MlsGroup> {
    MlsGroup::load(provider.storage(), &GroupId::from_slice(gid)).map_err(mls)?.ok_or(Error::Corrupt("missing MLS group"))
}

fn access_key_of(group: &MlsGroup, provider: &Provider<'_>) -> Result<Vec<u8>> {
    group.export_secret(provider.crypto(), ACCESS_LABEL, &[], 32).map_err(mls)
}

fn leaf_of_member(m: &Member) -> std::result::Result<LeafIdentity, Fail> {
    let (account, device) = post_mls::parse_credential(&m.credential).map_err(|_| reject("a member has an invalid credential"))?;
    Ok(LeafIdentity { account, device, signature_key: m.signature_key.clone() })
}

/// Every leaf must be an active device of its account, using the key its
/// chain lists (design section 6, "Credentials and the membership check").
fn authorize(chains: &HashMap<AccountId, Chain>, leaves: &[LeafIdentity]) -> std::result::Result<(), Fail> {
    for l in leaves {
        match chains.get(&l.account) {
            Some(c) if c.authorizes_leaf(&l.device, &l.signature_key) => {}
            _ => return Err(Fail::NeedDirectory),
        }
    }
    Ok(())
}

/// The membership check for a Welcome. A Welcome shows the group as it was
/// when this device was added, which can be before one of its members'
/// devices was removed from that account: a device that links, and whose
/// approving device logs out before the new one first fetches its
/// invitations, is invited by a device that has since left. A removal does
/// not make earlier states wrong (design section 6), so a leaf may also be a
/// removed device with the key its chain last listed for it. This device
/// then removes that leaf like any removed device's (`plan_membership`), and
/// meanwhile the server refuses a removed device's login, so it reads
/// nothing. A device its account never listed is still refused.
fn authorize_welcome(chains: &HashMap<AccountId, Chain>, leaves: &[LeafIdentity]) -> std::result::Result<(), Fail> {
    for l in leaves {
        let listed = chains.get(&l.account).and_then(|c| c.device(&l.device)).is_some_and(|d| d.entry.mls_sig_pub[..] == *l.signature_key);
        if !listed {
            return Err(Fail::NeedDirectory);
        }
    }
    Ok(())
}

/// Domain for a server URL: host, plus port if not the scheme's default.
pub(crate) fn domain_of(url: &str) -> Result<String> {
    let u = url::Url::parse(url).map_err(|_| Error::InvalidInput("server URL is not a valid URL".into()))?;
    if !matches!(u.scheme(), "ws" | "wss") {
        return Err(Error::InvalidInput("server URL must start with wss:// (or ws:// for local testing)".into()));
    }
    let host = u.host_str().ok_or_else(|| Error::InvalidInput("server URL has no host".into()))?.to_ascii_lowercase();
    let domain = match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    };
    post_identity::chain::validate_domain(&domain).map_err(|_| Error::InvalidInput(format!("{domain} is not a valid server domain")))?;
    Ok(domain)
}

pub(crate) struct Engine {
    pub conn: Connection,
    pub me: Option<Me>,
    pub registered: bool,
    /// Closed for good (logging out): never connects again.
    closed: bool,
    chains: HashMap<AccountId, Chain>,
    session: Option<ServerConn>,
    anon: Option<ServerConn>,
    subscribed: Vec<(Vec<u8>, u64)>,
    /// Devices whose last KeyPackage claim came back empty, and when.
    no_key_packages: HashMap<DeviceId, std::time::Instant>,
    /// Chain heads already warned about, so one problem is reported once.
    gossip_warned: HashSet<(AccountId, u64)>,
    /// Devices the directory lists as stale.
    stale: HashSet<DeviceId>,
    /// Leaves last refreshed before this time (Unix ms) are due now; set by
    /// an explicit refresh.
    refresh_before: u64,
    /// When this device last sent a typing signal to each group.
    typing_sent: HashMap<[u8; 16], std::time::Instant>,
    /// Others' presence and when it was last heard. Memory only, by design.
    presence: HashMap<AccountId, (ephemeral::State, std::time::Instant)>,
    link: Option<PendingLink>,
    /// Live calls. Memory only, by design.
    calls: std::sync::Mutex<calls::Calls>,
    /// Announcements are to be fetched at the next sync: set at login, and
    /// again when fetching them failed.
    announcements_due: bool,
    pub events: broadcast::Sender<Event>,
}

impl Engine {
    pub fn new(conn: Connection, events: broadcast::Sender<Event>) -> Result<Self> {
        let chains = store::load_chains(&conn)?;
        let stale = store::stale_devices(&conn)?;
        let mut e = Self {
            conn,
            me: None,
            registered: false,
            closed: false,
            chains,
            session: None,
            anon: None,
            subscribed: Vec::new(),
            no_key_packages: HashMap::new(),
            gossip_warned: HashSet::new(),
            stale,
            refresh_before: 0,
            typing_sent: HashMap::new(),
            presence: HashMap::new(),
            link: None,
            calls: std::sync::Mutex::default(),
            announcements_due: false,
            events,
        };
        e.load_identity()?;
        Ok(e)
    }

    fn load_identity(&mut self) -> Result<()> {
        let c = &self.conn;
        let Some(seeds) = store::meta_get(c, "chain_seeds")? else { return Ok(()) };
        let seeds: [u8; 64] = seeds.try_into().map_err(|_| Error::Corrupt("chain seeds"))?;
        let mls_seed: [u8; 32] =
            store::meta_get(c, "mls_seed")?.ok_or(Error::Corrupt("mls seed"))?.try_into().map_err(|_| Error::Corrupt("mls seed"))?;
        let keys = DeviceKeys { chain: HybridSigningKey::from_seeds(&seeds), mls: ed25519_dalek::SigningKey::from_bytes(&mls_seed) };
        let account = store::account_id(&store::meta_get(c, "account_id")?.ok_or(Error::Corrupt("account id"))?)?;
        let me = Me {
            signer: SignatureKeyPair::from_raw(
                SignatureScheme::ED25519,
                keys.mls.to_bytes().to_vec(),
                keys.mls.verifying_key().to_bytes().to_vec(),
            ),
            device: keys.device_id(),
            keys,
            account,
            username: store::meta_str(c, "username")?.ok_or(Error::Corrupt("username"))?,
            domain: store::meta_str(c, "domain")?.ok_or(Error::Corrupt("domain"))?,
            url: store::meta_str(c, "url")?.ok_or(Error::Corrupt("url"))?,
            bot: store::meta_str(c, "bot")?.as_deref() == Some("1"),
        };
        self.registered = store::meta_str(c, "registered")?.as_deref() == Some("1");
        self.me = Some(me);
        Ok(())
    }

    fn me(&self) -> Result<&Me> {
        self.me.as_ref().ok_or(Error::NotRegistered)
    }

    fn session(&self) -> Result<&ServerConn> {
        self.session.as_ref().filter(|s| !s.is_closed()).ok_or(Error::NotConnected)
    }

    fn anon(&self) -> Result<&ServerConn> {
        self.anon.as_ref().filter(|s| !s.is_closed()).ok_or(Error::NotConnected)
    }

    pub fn is_connected(&self) -> bool {
        self.session().is_ok() && self.anon().is_ok()
    }

    /// Both connections' wake and close signals, and the relayed signals,
    /// for the listen loop.
    pub fn listen_handles(&self) -> Result<ListenHandles> {
        let (s, a) = (self.session()?, self.anon()?);
        Ok(ListenHandles {
            wakes: [s.wake.clone(), a.wake.clone()],
            closed: [s.closed_signal(), a.closed_signal()],
            signals: a.signals.clone(),
        })
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn notice(&self, group: Option<&[u8]>, text: &str) -> Result<()> {
        tracing::warn!(notice = text);
        store::add_notice(&self.conn, group, text)?;
        self.emit(Event::Notice { conversation: group.and_then(|g| group_id_of(g).ok()).map(ConversationId), text: text.into() });
        Ok(())
    }

    fn check_hello(&self, hello: &proto::Hello, domain: &str) -> Result<()> {
        if hello.server_domain != domain {
            return Err(Error::Protocol(format!("server says it is {}, expected {domain}", hello.server_domain)));
        }
        Ok(())
    }

    // ------------------------------------------------------------ registration and login

    /// Create an account on `url` with an invite.
    pub async fn register(&mut self, url: &str, invite: &str, username: &str, bot: bool) -> Result<()> {
        if self.registered {
            return Err(Error::AlreadyRegistered);
        }
        validate_username(username)
            .map_err(|_| Error::InvalidInput("usernames are 3 to 32 of a-z, 0-9 and _, starting with a letter".into()))?;
        let domain = domain_of(url)?;
        self.forget_identity()?;

        let keys = DeviceKeys::generate()?;
        let (chain, genesis) = Chain::create(&keys, &domain, username, bot, store::now_ms())?;
        let account = chain.account_id();
        // Keys are saved before the server sees them: if the reply is lost,
        // the next connect logs in with them instead of burning the invite.
        {
            let tx = self.conn.transaction()?;
            store::meta_put(&tx, "chain_seeds", &keys.chain.to_seeds()[..])?;
            store::meta_put(&tx, "mls_seed", &keys.mls.to_bytes())?;
            store::meta_put(&tx, "account_id", &account.0)?;
            store::meta_put(&tx, "username", username.as_bytes())?;
            store::meta_put(&tx, "domain", domain.as_bytes())?;
            store::meta_put(&tx, "url", url.as_bytes())?;
            store::meta_put(&tx, "bot", if bot { b"1" } else { b"0" })?;
            store::meta_put(&tx, "registered", b"0")?;
            store::save_chain(&tx, &chain)?;
            tx.commit()?;
        }
        self.chains.insert(account, chain);
        self.load_identity()?;

        let (session, hello) = ServerConn::connect(url).await?;
        self.check_hello(&hello, &domain)?;
        let sig = keys.chain.sign(labels::AUTH, &proto::auth_message(&domain, &hello.nonce))?;
        let req = Request::Register(proto::Register {
            invite_code: invite.trim().into(),
            genesis,
            auth_sig_ed25519: sig.ed25519_bytes().to_vec(),
            auth_sig_mldsa65: sig.mldsa65_bytes().to_vec(),
        });
        let auth = match session.request(req).await {
            Ok(r) => expect_reply!(r, Reply::Authenticated)?,
            Err(e @ Error::Server(_)) => {
                // A definite refusal: these keys were never accepted.
                self.forget_identity()?;
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        self.mark_registered()?;
        self.session = Some(session);
        self.after_login(auth).await
    }

    fn mark_registered(&mut self) -> Result<()> {
        let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
        let tx = self.conn.transaction()?;
        store::meta_put(&tx, "registered", b"1")?;
        store::upsert_account(&tx, &me.account, &me.username, false)?;
        tx.commit()?;
        self.registered = true;
        Ok(())
    }

    fn forget_identity(&mut self) -> Result<()> {
        if let Some(me) = self.me.take() {
            self.chains.remove(&me.account);
            self.conn.execute("DELETE FROM chain_links WHERE account_id = ?1", [&me.account.0[..]])?;
        }
        self.conn.execute(
            "DELETE FROM meta WHERE k IN ('chain_seeds', 'mls_seed', 'account_id', 'username', 'domain', 'url', 'bot', 'registered', 'kp_target')",
            [],
        )?;
        store::forget_announcements(&self.conn)?;
        self.registered = false;
        Ok(())
    }

    /// Log in, then bring everything up to date.
    pub async fn connect(&mut self) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.is_connected() {
            return Ok(());
        }
        let (url, domain) = {
            let me = self.me()?;
            (me.url.clone(), me.domain.clone())
        };
        let (session, hello) = ServerConn::connect(&url).await?;
        self.check_hello(&hello, &domain)?;
        let me = self.me()?;
        let sig = me.keys.chain.sign(labels::AUTH, &proto::auth_message(&domain, &hello.nonce))?;
        let req = Request::Auth(proto::Auth {
            device_id: me.device.0.to_vec(),
            sig_ed25519: sig.ed25519_bytes().to_vec(),
            sig_mldsa65: sig.mldsa65_bytes().to_vec(),
        });
        let auth = match session.request(req).await {
            Ok(r) => expect_reply!(r, Reply::Authenticated)?,
            Err(Error::Server(e)) if !self.registered && e.code() == ErrorCode::Unauthenticated => {
                // Registration never reached the server.
                self.forget_identity()?;
                return Err(Error::NotRegistered);
            }
            Err(e) => return Err(e),
        };
        if !self.registered {
            self.mark_registered()?;
        }
        self.session = Some(session);
        self.after_login(auth).await
    }

    async fn after_login(&mut self, auth: proto::Authenticated) -> Result<()> {
        self.sync_directory().await?;
        self.adopt_own_username()?;
        self.settle_pending_link().await?;
        self.top_up_key_packages(Some((auth.key_packages_remaining, auth.has_last_resort))).await?;
        self.top_up_tokens().await?;
        let (anon, hello) = ServerConn::connect(&self.me()?.url).await?;
        self.check_hello(&hello, &self.me()?.domain)?;
        self.anon = Some(anon);
        self.subscribed.clear();
        self.announcements_due = true;
        self.retry_pending_groups().await?;
        self.retry_pending_commits().await?;
        self.ensure_self_group().await?;
        self.sync().await
    }

    /// Create this account's self group if this device is in none and is the
    /// account's oldest active device. Every other device waits to be added,
    /// so an account does not end up with one self group per device.
    async fn ensure_self_group(&mut self) -> Result<()> {
        if store::self_group(&self.conn)?.is_some() {
            return Ok(());
        }
        let me = self.me()?;
        let oldest = self.chains.get(&me.account).and_then(|c| c.active_devices().next()).map(|d| d.id);
        if oldest != Some(me.device) {
            return Ok(());
        }
        let gid = random16()?;
        let request = {
            let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
            let tx = self.conn.transaction()?;
            let provider = Provider::on(&tx);
            let mut group = post_mls::group_builder(&GroupMeta::self_group(&me.account))
                .map_err(mls)?
                .with_group_id(GroupId::from_slice(&gid))
                .build(&provider, &me.signer, me.credential())
                .map_err(mls)?;
            // The server registers a group at epoch 1, so commit once to get there.
            group.self_update(&provider, &me.signer, LeafNodeParameters::default()).map_err(mls)?;
            let staged = group.pending_commit().ok_or(Error::Corrupt("pending commit"))?;
            let access = staged.export_secret(provider.crypto(), ACCESS_LABEL, &[], 32).map_err(mls)?;
            let req = proto::CreateGroup {
                token: None,
                group_id: gid.to_vec(),
                access_hash: Sha256::digest(&access).to_vec(),
                welcome: Vec::new(),
                welcome_to: Vec::new(),
            };
            store::pending_group_put(&tx, &gid, &req.encode_to_vec())?;
            drop(provider);
            tx.commit()?;
            req
        };
        self.finish_create(gid, request).await
    }

    /// The verified chain names the account's username; a linked device was
    /// only told it by the device that linked it.
    fn adopt_own_username(&mut self) -> Result<()> {
        let Some(me) = self.me.as_mut() else { return Ok(()) };
        if let Some(chain) = self.chains.get(&me.account)
            && chain.username() != me.username
        {
            me.username = chain.username().to_string();
            store::meta_put(&self.conn, "username", me.username.as_bytes())?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ linking a device

    /// On a new device: make keys and a code, and offer this device for
    /// linking. Returns the code for the user to enter on a device that is
    /// already part of their account.
    pub async fn begin_link(&mut self, url: &str, name: &str, bot: bool) -> Result<LinkCode> {
        if self.registered {
            return Err(Error::AlreadyRegistered);
        }
        check_device_name(name)?;
        let domain = domain_of(url)?;
        let keys = DeviceKeys::generate()?;
        let code = LinkCode::generate()?;
        let channel = Channel::new(&code);
        let (conn, hello) = ServerConn::connect(url).await?;
        self.check_hello(&hello, &domain)?;
        let offer = linking::Offer { entry: keys.entry(bot).encode(), name: name.into() };
        channel.put(&conn, linking::SLOT_OFFER, &offer).await?;
        self.link = Some(PendingLink { channel, conn, keys, url: url.into(), domain, bot });
        Ok(code)
    }

    /// On the new device: wait for the other device to accept, countersign,
    /// log in, and wait to be added to the account's conversations.
    pub async fn finish_link(&mut self) -> Result<()> {
        let link = self.link.take().ok_or_else(|| Error::InvalidInput("no link in progress; start one first".into()))?;
        let waited_too_long = || Error::InvalidInput("the code was not entered in time; start again".into());
        let answer: linking::Answer =
            link.channel.get(&link.conn, linking::SLOT_ANSWER, LINK_WAIT_FOR_USER).await?.ok_or_else(waited_too_long)?;

        // Countersign only a link that adds exactly this device.
        let parsed = Link::decode(&answer.link)?;
        let adds_me = matches!(&parsed.op, Op::AddDevice { device } if *device == link.keys.entry(link.bot));
        let account = parsed.account_id.filter(|_| adds_me).ok_or_else(|| Error::Protocol("the other device sent a link that does not add this one".into()))?;
        validate_username(&answer.username).map_err(|_| Error::Protocol("the other device sent an invalid username".into()))?;

        // Keys are saved before the countersignature leaves: once it does,
        // the link may land on the server whatever happens here.
        self.forget_identity()?;
        {
            let tx = self.conn.transaction()?;
            store::meta_put(&tx, "chain_seeds", &link.keys.chain.to_seeds()[..])?;
            store::meta_put(&tx, "mls_seed", &link.keys.mls.to_bytes())?;
            store::meta_put(&tx, "account_id", &account.0)?;
            store::meta_put(&tx, "username", answer.username.as_bytes())?;
            store::meta_put(&tx, "domain", link.domain.as_bytes())?;
            store::meta_put(&tx, "url", link.url.as_bytes())?;
            store::meta_put(&tx, "bot", if link.bot { b"1" } else { b"0" })?;
            store::meta_put(&tx, "registered", b"0")?;
            // Enough KeyPackages to be added to every conversation at once.
            store::meta_put_u64(&tx, "kp_target", u64::from(answer.conversations.saturating_add(KP_LOW)))?;
            tx.commit()?;
        }
        self.load_identity()?;

        let sig = link.keys.cosign_link(&answer.link)?;
        let cosign = linking::Cosign { ed25519: sig.ed25519_bytes().to_vec(), mldsa65: sig.mldsa65_bytes().to_vec() };
        link.channel.put(&link.conn, linking::SLOT_COSIGN, &cosign).await?;
        let done: Option<linking::Step> = link.channel.get(&link.conn, linking::SLOT_DONE, LINK_WAIT_FOR_DEVICE).await?;
        if done.is_none() {
            // The other device may still have uploaded the link; logging in
            // tells. If it did not, connect forgets these keys.
            tracing::warn!("the other device did not confirm the link");
        }
        self.connect().await?;
        link.channel.put(&link.conn, linking::SLOT_READY, &linking::Step::default()).await?;
        if link.channel.get::<linking::Step>(&link.conn, linking::SLOT_ADDED, LINK_WAIT_FOR_DEVICE).await?.is_some() {
            self.sync().await?;
        }
        Ok(())
    }

    /// On an existing device: read the offer behind a code. Takes `&mut self`
    /// although it changes nothing: a shared borrow of the engine held across
    /// an await would make the future non-`Send` (the connection is not `Sync`).
    async fn fetch_offer(&mut self, code: &str) -> Result<(Channel, ServerConn, LinkOffer)> {
        let channel = Channel::new(&LinkCode::parse(code)?);
        let (conn, hello) = ServerConn::connect(&self.me()?.url).await?;
        self.check_hello(&hello, &self.me()?.domain)?;
        let offer: linking::Offer = channel
            .get(&conn, linking::SLOT_OFFER, std::time::Duration::from_secs(5))
            .await?
            .ok_or_else(|| Error::InvalidInput("no device is waiting with that code".into()))?;
        let entry = DeviceEntry::decode(&offer.entry).map_err(|_| Error::Protocol("the new device sent an invalid key".into()))?;
        let name = if check_device_name(&offer.name).is_ok() { offer.name } else { String::new() };
        let me = self.me()?;
        let chain = self.chains.get(&me.account).ok_or(Error::Corrupt("own chain"))?;
        if chain.device(&entry.device_id()).is_some() {
            return Err(Error::InvalidInput("that device is already linked".into()));
        }
        if entry.bot != chain.is_bot() {
            return Err(Error::InvalidInput(if chain.is_bot() {
                "this is a bot account; link the new device as a bot".into()
            } else {
                "the new device was set up as a bot, but this is not a bot account".into()
            }));
        }
        Ok((channel, conn, LinkOffer { entry, name }))
    }

    /// On an existing device: what a code would link, for the user to confirm.
    pub async fn link_inspect(&mut self, code: &str) -> Result<LinkOffer> {
        Ok(self.fetch_offer(code).await?.2)
    }

    /// On an existing device: add the device behind `code` to this account,
    /// then to every conversation.
    pub async fn link_approve(&mut self, code: &str) -> Result<LinkOffer> {
        let (channel, conn, offer) = self.fetch_offer(code).await?;
        let (bytes, answer) = {
            let me = self.me()?;
            let chain = self.chains.get(&me.account).ok_or(Error::Corrupt("own chain"))?;
            let bytes = chain.next_link(me.device, Op::AddDevice { device: offer.entry.clone() }, store::now_ms())?;
            let conversations = u32::try_from(store::broadcast_groups(&self.conn)?.len()).unwrap_or(u32::MAX);
            let answer = linking::Answer { link: bytes.clone(), username: me.username.clone(), conversations };
            (bytes, answer)
        };
        channel.put(&conn, linking::SLOT_ANSWER, &answer).await?;
        let cosign: linking::Cosign = channel
            .get(&conn, linking::SLOT_COSIGN, LINK_WAIT_FOR_DEVICE)
            .await?
            .ok_or_else(|| Error::InvalidInput("the new device did not answer; start again".into()))?;
        let cosig = HybridSignature::from_parts(&cosign.ed25519, &cosign.mldsa65)?;
        let raw = post_identity::chain::seal(&bytes, &self.me()?.keys.sign_link(&bytes)?, Some(&cosig));
        self.append_own_link(raw).await?;
        channel.put(&conn, linking::SLOT_DONE, &linking::Step::default()).await?;
        if !offer.name.is_empty() {
            let entry = content::DeviceNameEntry { device: offer.entry.device_id().0.to_vec(), name: offer.name.clone() };
            self.sync_to_own_devices(content::SelfSync { device_names: vec![entry], ..Default::default() }).await?;
        }

        // Add it everywhere once it has KeyPackages on the server. If it is
        // slow, membership reconciliation adds it on a later sync instead.
        if channel.get::<linking::Step>(&conn, linking::SLOT_READY, LINK_WAIT_FOR_DEVICE).await?.is_some() {
            self.reconcile().await?;
            // Sends what the self group's new member should hear first.
            self.flush_outbox().await?;
        }
        let count = u32::try_from(store::broadcast_groups(&self.conn)?.len()).unwrap_or(u32::MAX);
        channel.put(&conn, linking::SLOT_ADDED, &linking::Step { count }).await?;
        Ok(offer)
    }

    // ------------------------------------------------------------ devices

    /// This account's active devices, oldest first, with their names.
    pub fn devices(&self) -> Result<Vec<(DeviceId, String)>> {
        let me = self.me()?;
        let chain = self.chains.get(&me.account).ok_or(Error::Corrupt("own chain"))?;
        chain
            .active_devices()
            .map(|d| Ok((d.id, store::synced_get(&self.conn, store::Synced::DeviceName, &d.id.0)?.unwrap_or_default())))
            .collect()
    }

    /// An active device of this account from its ID or an unambiguous prefix
    /// of at least four hex characters.
    pub fn resolve_device(&self, id: &str) -> Result<DeviceId> {
        let id = id.trim().to_ascii_lowercase();
        if id.len() < 4 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::InvalidInput("give a device ID, or at least its first four characters".into()));
        }
        let matches: Vec<DeviceId> = self.devices()?.into_iter().map(|(d, _)| d).filter(|d| d.to_string().starts_with(&id)).collect();
        match matches.as_slice() {
            [d] => Ok(*d),
            [] => Err(Error::NotFound("device")),
            _ => Err(Error::InvalidInput("more than one device starts with that; give more of its ID".into())),
        }
    }

    /// Name one of this account's devices, on all of them.
    pub async fn rename_device(&mut self, device: DeviceId, name: &str) -> Result<()> {
        check_device_name(name)?;
        let entry = content::DeviceNameEntry { device: device.0.to_vec(), name: name.into() };
        self.sync_to_own_devices(content::SelfSync { device_names: vec![entry], ..Default::default() }).await
    }

    /// Remove another of this account's devices: publish its removal in the
    /// chain, which makes the server cut it off at once, then remove it from
    /// every group so it cannot follow them past the current epoch.
    pub async fn remove_device(&mut self, device: DeviceId) -> Result<()> {
        let raw = {
            let me = self.me()?;
            if device == me.device {
                return Err(Error::InvalidInput("remove this device from another of your devices".into()));
            }
            let chain = self.chains.get(&me.account).ok_or(Error::Corrupt("own chain"))?;
            let bytes = chain.next_link(me.device, Op::RemoveDevice { device_id: device }, store::now_ms())?;
            post_identity::chain::seal(&bytes, &me.keys.sign_link(&bytes)?, None)
        };
        self.append_own_link(raw).await?;
        self.reconcile().await?;
        self.flush_outbox().await
    }

    /// Log out: this device signs its own removal from the account (design
    /// section 4 allows any device to remove itself, but never the last
    /// one). Members' devices then remove its leaves, as for any removed
    /// device; it cannot commit its own removal. Sends what is queued first,
    /// so nothing written here is lost.
    pub async fn log_out(&mut self) -> Result<()> {
        self.connect().await?;
        self.flush_outbox().await?;
        let raw = {
            let me = self.me()?;
            let chain = self.chains.get(&me.account).ok_or(Error::Corrupt("own chain"))?;
            if chain.active_devices().count() <= 1 {
                return Err(Error::InvalidInput(
                    "this is the account's only device: link another device before logging out, or the account is lost".into(),
                ));
            }
            let bytes = chain.next_link(me.device, Op::RemoveDevice { device_id: me.device }, store::now_ms())?;
            post_identity::chain::seal(&bytes, &me.keys.sign_link(&bytes)?, None)
        };
        self.append_own_link(raw).await?;
        self.close();
        Ok(())
    }

    /// Stop for good: drop both connections, and refuse to connect again.
    pub fn close(&mut self) {
        self.closed = true;
        self.session = None;
        self.anon = None;
    }

    /// Verify, record and upload a link to this account's own chain. The link
    /// is kept until the server answers, so a lost reply is resolved at the
    /// next login rather than leaving this device unsure of its own chain.
    async fn append_own_link(&mut self, raw: Vec<u8>) -> Result<()> {
        let account = self.me()?.account;
        let mut chain = self.chains.get(&account).ok_or(Error::Corrupt("own chain"))?.clone();
        let seq = chain.next_seq();
        chain.append(&raw)?;
        store::meta_put(&self.conn, "pending_link", &raw)?;
        store::meta_put_u64(&self.conn, "pending_link_seq", seq)?;
        match self.session()?.request(Request::AppendChain(proto::AppendChain { link: raw })).await {
            Ok(_) => {
                let tx = self.conn.transaction()?;
                store::save_chain(&tx, &chain)?;
                store::meta_delete(&tx, "pending_link")?;
                tx.commit()?;
                self.chains.insert(account, chain);
                Ok(())
            }
            Err(Error::Server(e)) => {
                store::meta_delete(&self.conn, "pending_link")?;
                self.sync_directory().await?;
                Err(Error::Server(e))
            }
            Err(e) => Err(e),
        }
    }

    /// Settle a chain link whose upload had no answer: done if the server's
    /// chain has it, uploaded again if the chain can still take it, dropped
    /// if another link took its place.
    async fn settle_pending_link(&mut self) -> Result<()> {
        let Some(raw) = store::meta_get(&self.conn, "pending_link")? else { return Ok(()) };
        let seq = store::meta_u64(&self.conn, "pending_link_seq")?;
        let account = self.me()?.account;
        let chain = self.chains.get(&account).ok_or(Error::Corrupt("own chain"))?;
        match chain.clone().extend_from(seq, std::slice::from_ref(&raw)) {
            Ok(0) => store::meta_delete(&self.conn, "pending_link"),
            Ok(_) => self.append_own_link(raw).await,
            Err(_) => {
                store::meta_delete(&self.conn, "pending_link")?;
                self.notice(None, "a device change made on this device was not accepted; check your devices")
            }
        }
    }

    // ------------------------------------------------------------ directory, key packages, tokens

    /// Download what changed in the directory and verify it.
    pub async fn sync_directory(&mut self) -> Result<()> {
        let since = store::meta_u64(&self.conn, "dir_version")?;
        let upd = expect_reply!(self.session()?.request(Request::Directory(proto::Directory { since_version: since })).await?, Reply::DirectoryUpdate)?;
        let domain = self.me()?.domain.clone();
        for a in upd.accounts {
            let Ok(account) = store::account_id(&a.account_id) else { continue };
            let mut before = None;
            let verified = match self.chains.get(&account) {
                Some(c) => {
                    before = Some(c.active_devices().map(|d| d.id).collect::<HashSet<_>>());
                    let mut c = c.clone();
                    match c.extend_from(0, &a.chain) {
                        Ok(_) => c,
                        Err(e) => {
                            self.notice(None, &format!("the server offered a conflicting device list for {}: {e}", a.username))?;
                            continue;
                        }
                    }
                }
                None => match Chain::from_links(a.chain.iter().map(Vec::as_slice)) {
                    Ok(c) => c,
                    Err(e) => {
                        self.notice(None, &format!("the server offered an invalid device list for {}: {e}", a.username))?;
                        continue;
                    }
                },
            };
            if verified.account_id() != account || verified.username() != a.username || verified.server_domain() != domain {
                self.notice(None, &format!("the server's directory entry for {} does not match its device list", a.username))?;
                continue;
            }
            let tx = self.conn.transaction()?;
            match store::upsert_account(&tx, &account, &a.username, a.disabled) {
                Ok(()) => {}
                Err(Error::IdentityChanged(u)) => {
                    drop(tx);
                    self.notice(None, &format!("the account for {u} has changed; it may have been reset or impersonated"))?;
                    continue;
                }
                Err(e) => return Err(e),
            }
            store::save_chain(&tx, &verified)?;
            let stale: Vec<DeviceId> = a.stale_devices.iter().filter_map(|d| DeviceId::from_slice(d).ok()).collect();
            store::set_stale_devices(&tx, &account, &stale)?;
            tx.commit()?;
            for d in verified.devices() {
                self.stale.remove(&d.id);
            }
            self.stale.extend(stale);
            if let Some(before) = before {
                let after: HashSet<DeviceId> = verified.active_devices().map(|d| d.id).collect();
                let (added, removed) = (after.difference(&before).count() as u32, before.difference(&after).count() as u32);
                if added + removed > 0 {
                    self.emit(Event::DevicesChanged { username: a.username.clone(), added, removed });
                }
            }
            self.chains.insert(account, verified);
        }
        store::meta_put_u64(&self.conn, "dir_version", upd.version)?;
        Ok(())
    }

    /// Keep enough KeyPackages on the server. `known` is (remaining, has
    /// last resort) if the caller already knows; otherwise the server is asked.
    async fn top_up_key_packages(&mut self, known: Option<(u32, bool)>) -> Result<()> {
        let (remaining, has_last_resort) = match known {
            Some(k) => k,
            None => {
                let empty = Request::UploadKeyPackages(proto::UploadKeyPackages::default());
                (expect_reply!(self.session()?.request(empty).await?, Reply::KeyPackageCount)?.remaining, true)
            }
        };
        // A newly linked device needs one per conversation it is about to join.
        let target = u32::try_from(store::meta_u64(&self.conn, "kp_target")?).unwrap_or(KP_MAX).clamp(KP_TARGET, KP_MAX);
        if remaining >= KP_LOW && has_last_resort && target == KP_TARGET {
            return Ok(());
        }
        let n = target.saturating_sub(remaining) as usize;
        let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
        let (one_time, last_resort) = {
            let tx = self.conn.transaction()?;
            let provider = Provider::on(&tx);
            let build = |last: bool| -> Result<Vec<u8>> {
                let mut b = KeyPackage::builder().leaf_node_capabilities(post_mls::capabilities());
                if last {
                    b = b.mark_as_last_resort();
                }
                let kp = b.build(SUITE, &provider, &me.signer, me.credential()).map_err(mls)?;
                kp.key_package().tls_serialize_detached().map_err(mls)
            };
            let one_time = (0..n).map(|_| build(false)).collect::<Result<Vec<_>>>()?;
            let last_resort = if has_last_resort { Vec::new() } else { build(true)? };
            drop(provider);
            tx.commit()?;
            (one_time, last_resort)
        };
        self.session()?
            .request(Request::UploadKeyPackages(proto::UploadKeyPackages { key_packages: one_time, last_resort }))
            .await?;
        store::meta_delete(&self.conn, "kp_target")?;
        Ok(())
    }

    /// Keep a pool of anonymous tokens, collected well before they are spent
    /// so spending one cannot be timed against collecting it.
    async fn top_up_tokens(&mut self) -> Result<()> {
        if store::tokens_available(&self.conn)? >= TOKEN_LOW {
            return Ok(());
        }
        let key = expect_reply!(self.session()?.request(Request::GetTokenKey(proto::GetTokenKey {})).await?, Reply::TokenKey)?;
        if key.version != proto::TOKEN_VERSION_RSABSSA_SHA384 {
            return Err(Error::Protocol("unsupported token version".into()));
        }
        let pk = TokenPublicKey::from_spki(&key.public_key).map_err(|_| Error::Protocol("invalid token key".into()))?;
        let mut pending = Vec::with_capacity(TOKEN_BATCH);
        for _ in 0..TOKEN_BATCH {
            let mut msg = [0u8; 32];
            getrandom::fill(&mut msg).map_err(|_| Error::Rng)?;
            let blinded = pk.blind(&mut DefaultRng, msg).map_err(|_| Error::Protocol("blinding failed".into()))?;
            pending.push((msg, blinded));
        }
        let req = Request::IssueTokens(proto::IssueTokens {
            key_id: key.key_id.clone(),
            blinded: pending.iter().map(|(_, b)| b.blind_message.0.clone()).collect(),
        });
        let issued = match self.session()?.request(req).await {
            Ok(r) => expect_reply!(r, Reply::IssuedTokens)?,
            Err(Error::Server(e)) if e.code() == ErrorCode::RateLimited => {
                tracing::warn!("token limit reached for today");
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if issued.blind_signatures.len() != pending.len() {
            return Err(Error::Protocol("server returned the wrong number of tokens".into()));
        }
        let tx = self.conn.transaction()?;
        for ((msg, blinded), sig) in pending.iter().zip(issued.blind_signatures) {
            let sig = pk.finalize(&BlindSignature(sig), blinded, msg).map_err(|_| Error::Protocol("server returned an invalid token".into()))?;
            let randomizer = blinded.msg_randomizer.ok_or_else(|| Error::Protocol("token randomizer missing".into()))?;
            let token = proto::Token {
                version: proto::TOKEN_VERSION_RSABSSA_SHA384,
                key_id: key.key_id.clone(),
                message: msg.to_vec(),
                randomizer: randomizer.0.to_vec(),
                signature: sig.0,
            };
            store::store_token(&tx, &token.encode_to_vec(), key.valid_until_ms)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Take a token, deleting it first so it is never presented twice.
    async fn take_token(&mut self) -> Result<proto::Token> {
        if store::tokens_available(&self.conn)? == 0 {
            self.top_up_tokens().await?;
        }
        let raw = store::take_token(&self.conn)?.ok_or(Error::NoTokens)?;
        proto::Token::decode(raw.as_slice()).map_err(|_| Error::Corrupt("stored token"))
    }

    // ------------------------------------------------------------ people

    /// The account for a username, from the verified directory. Never asks
    /// the server about this particular name.
    pub async fn resolve(&mut self, username: &str) -> Result<AccountId> {
        if store::account_by_username(&self.conn, username)?.is_none() {
            self.sync_directory().await?;
        }
        match store::account_by_username(&self.conn, username)? {
            Some((_, true)) => Err(Error::InvalidInput(format!("{username}'s account is disabled"))),
            Some((a, false)) => Ok(a),
            None => Err(Error::UnknownUser(username.into())),
        }
    }

    /// Claim and verify KeyPackages for each of an account's devices, other
    /// than this one.
    async fn claim(&mut self, account: AccountId) -> Result<Vec<KeyPackage>> {
        let token = self.take_token().await?;
        let req = Request::ClaimKeyPackages(proto::ClaimKeyPackages {
            token: Some(token),
            account_id: account.0.to_vec(),
            ..Default::default()
        });
        let claimed = expect_reply!(self.anon()?.request(req).await?, Reply::ClaimedKeyPackages)?;
        let crypto = post_mls::crypto();
        let me = self.me()?.device;
        let mut out = Vec::new();
        for c in claimed.key_packages {
            let v = match post_mls::validate_key_package(&c.key_package, &crypto) {
                Ok(v) => v,
                Err(e) => {
                    self.notice(None, &format!("the server returned an invalid KeyPackage: {e}"))?;
                    continue;
                }
            };
            let authorized = self.chains.get(&account).is_some_and(|ch| ch.authorizes_leaf(&v.leaf.device, &v.leaf.signature_key));
            if v.leaf.account != account || c.device_id[..] != v.leaf.device.0 || !authorized {
                self.notice(None, "the server returned a KeyPackage for a device that is not authorized")?;
                continue;
            }
            if v.leaf.device != me {
                out.push(v.key_package);
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------ conversations

    /// Open a DM with `username`, creating it if needed.
    pub async fn start_dm(&mut self, username: &str) -> Result<ConversationId> {
        let peer = self.resolve(username).await?;
        let mine = self.me()?.account;
        if peer == mine {
            return Err(Error::InvalidInput("you cannot DM yourself".into()));
        }
        if let Some(c) = store::dm_with(&self.conn, &peer)? {
            return Ok(ConversationId(c.group_id));
        }
        let mut kps = self.claim(peer).await?;
        if kps.is_empty() {
            return Err(Error::PeerUnavailable(username.into()));
        }
        // The account's other devices join too. The server cannot know who is
        // asking, so this also consumes one of this device's own KeyPackages.
        kps.extend(self.claim(mine).await?);
        let gid = self.create_conversation(GroupMeta::dm(&mine, &peer), kps).await?;
        Ok(ConversationId(store::dm_with(&self.conn, &peer)?.map_or(gid, |c| c.group_id)))
    }

    /// Create a named group with this account as its admin and `usernames`
    /// as members, each with all of their devices.
    pub async fn create_group(&mut self, name: &str, usernames: &[String]) -> Result<ConversationId> {
        check_group_name(name)?;
        let mine = self.me()?.account;
        let mut accounts = Vec::new();
        for u in usernames {
            let a = self.resolve(u).await?;
            if a != mine && !accounts.iter().any(|(x, _)| *x == a) {
                accounts.push((a, u.clone()));
            }
        }
        if accounts.is_empty() {
            return Err(Error::InvalidInput("a group needs at least one other member".into()));
        }
        if accounts.len() >= MAX_GROUP_ACCOUNTS {
            return Err(Error::InvalidInput(format!("a group can have at most {MAX_GROUP_ACCOUNTS} members")));
        }
        let mut kps = Vec::new();
        for (account, username) in &accounts {
            let theirs = self.claim(*account).await?;
            if theirs.is_empty() {
                return Err(Error::PeerUnavailable(username.clone()));
            }
            kps.extend(theirs);
        }
        kps.extend(self.claim(mine).await?);
        // New members see past messages, shared a week at a time on request
        // (design section 6, "History for new members"); admins can change it.
        let meta = GroupMeta::group(name, &mine, post_mls::HistoryVisibility::Shared, 0);
        Ok(ConversationId(self.create_conversation(meta, kps).await?))
    }

    /// Change a group as its admin: add accounts (with all their devices),
    /// remove accounts (all their leaves, and their admin role), and edit the
    /// group's details. Decided against the group's current state, and
    /// decided again if another commit takes the epoch first.
    async fn change_group(
        &mut self,
        gid: [u8; 16],
        add: &[AccountId],
        remove: &[AccountId],
        mut edit: impl FnMut(&mut GroupMeta) -> Result<()> + Send,
    ) -> Result<()> {
        for _ in 0..3 {
            self.sync_groups().await?;
            if store::pending_commit(&self.conn, &gid)?.is_some() {
                return Err(Error::InvalidInput("another change to this group is still being sent; try again shortly".into()));
            }
            let conv = store::conversation(&self.conn, &gid)?.filter(|c| c.following).ok_or(Error::NotFound("conversation"))?;
            if conv.kind != KIND_GROUP {
                return Err(Error::NotAllowed("only group chats have members and details to change"));
            }
            let mine = self.me()?.account;
            let (meta, members) = group_members(&self.conn, &gid)?;
            if !meta.admins.iter().any(|a| a[..] == mine.0) {
                return Err(Error::NotAllowed("only the group's admins can do that"));
            }
            let mut new_meta = meta.clone();
            edit(&mut new_meta)?;
            new_meta.admins.retain(|a| !remove.iter().any(|r| a[..] == r.0));
            if new_meta.admins.is_empty() {
                return Err(Error::InvalidInput("a group needs at least one admin".into()));
            }
            let is_member = |a: &AccountId| members.iter().any(|(m, _)| m == a);
            let to_add: Vec<AccountId> = add.iter().filter(|a| !is_member(a)).copied().collect();
            if members.len() + to_add.len() > MAX_GROUP_ACCOUNTS {
                return Err(Error::InvalidInput(format!("a group can have at most {MAX_GROUP_ACCOUNTS} members")));
            }
            let leaves: Vec<LeafNodeIndex> = {
                let provider = Provider::on(&self.conn);
                let group = load_group(&provider, &gid)?;
                group.members().filter(|m| leaf_of_member(m).is_ok_and(|l| remove.contains(&l.account))).map(|m| m.index).collect()
            };
            let meta_change = (new_meta != meta).then_some(new_meta);
            if to_add.is_empty() && leaves.is_empty() && meta_change.is_none() {
                return Ok(());
            }
            let mut kps = Vec::new();
            for a in &to_add {
                let theirs = self.claim(*a).await?;
                if theirs.is_empty() {
                    let name = store::username_of(&self.conn, &a.0)?.unwrap_or_default();
                    return Err(Error::PeerUnavailable(name));
                }
                kps.extend(theirs);
            }
            if self.commit_changes(gid, kps, leaves, meta_change, false).await? {
                // Merged when it comes back through the log.
                self.sync_groups().await?;
                return self.flush_outbox().await;
            }
        }
        Err(Error::InvalidInput("the group kept changing while this was being sent; try again".into()))
    }

    async fn accounts_of(&mut self, usernames: &[String]) -> Result<Vec<AccountId>> {
        let mut out = Vec::new();
        for u in usernames {
            out.push(self.resolve(u).await?);
        }
        Ok(out)
    }

    /// Add people to a group, with all their devices.
    pub async fn add_members(&mut self, gid: [u8; 16], usernames: &[String]) -> Result<()> {
        let accounts = self.accounts_of(usernames).await?;
        self.change_group(gid, &accounts, &[], |_| Ok(())).await
    }

    /// Remove people from a group, with all their devices.
    pub async fn remove_members(&mut self, gid: [u8; 16], usernames: &[String]) -> Result<()> {
        let accounts = self.accounts_of(usernames).await?;
        if accounts.contains(&self.me()?.account) {
            return Err(Error::InvalidInput("to leave a group, leave it rather than removing yourself".into()));
        }
        self.change_group(gid, &[], &accounts, |_| Ok(())).await
    }

    /// Rename a group.
    pub async fn rename_group(&mut self, gid: [u8; 16], name: &str) -> Result<()> {
        check_group_name(name)?;
        self.change_group(gid, &[], &[], |m| {
            m.name = name.to_string();
            Ok(())
        })
        .await
    }

    /// Make a member an admin, or stop them being one.
    pub async fn set_admin(&mut self, gid: [u8; 16], username: &str, admin: bool) -> Result<()> {
        let account = self.resolve(username).await?;
        let (_, members) = group_members(&self.conn, &gid)?;
        if !members.iter().any(|(a, _)| *a == account) {
            return Err(Error::InvalidInput(format!("{username} is not in this group")));
        }
        self.change_group(gid, &[], &[], |m| {
            m.admins.retain(|a| a[..] != account.0);
            if admin {
                m.admins.push(account.0.to_vec());
            }
            Ok(())
        })
        .await
    }

    /// Whether new members see past messages, and how many days back.
    pub async fn set_history(&mut self, gid: [u8; 16], shared: bool, days: u32) -> Result<()> {
        self.change_group(gid, &[], &[], |m| {
            m.history = if shared { post_mls::HistoryVisibility::Shared } else { post_mls::HistoryVisibility::Hidden } as i32;
            m.history_days = if shared { days } else { 0 };
            Ok(())
        })
        .await
    }

    /// Leave a group, with all of this account's devices (design section 6,
    /// "Group policy"). A member cannot commit its own removal, so this
    /// proposes it and another member commits it; until then this device
    /// keeps proposing in each new epoch, and shows the group as left.
    pub async fn leave_group(&mut self, gid: [u8; 16]) -> Result<()> {
        let conv = store::conversation(&self.conn, &gid)?.filter(|c| c.following).ok_or(Error::NotFound("conversation"))?;
        if conv.kind != KIND_GROUP {
            return Err(Error::NotAllowed("only group chats can be left"));
        }
        if conv.leaving {
            return Ok(());
        }
        let mine = self.me()?.account;
        let (meta, members) = group_members(&self.conn, &gid)?;
        let others: Vec<AccountId> = members.iter().map(|(a, _)| *a).filter(|a| *a != mine).collect();
        if let Some(heir) = others.first().copied() {
            // Hand the group on first if we are its only admin among members,
            // or nobody could manage it (and other clients would refuse the
            // removal). The heir is the longest-standing other member.
            let another_admin = meta.admins.iter().any(|a| others.iter().any(|o| a[..] == o.0));
            if !another_admin {
                self.change_group(gid, &[], &[], |m| {
                    m.admins.push(heir.0.to_vec());
                    Ok(())
                })
                .await?;
            }
        }
        {
            let tx = self.conn.transaction()?;
            store::set_leaving(&tx, &gid)?;
            if others.is_empty() {
                // Nobody is left to commit a removal.
                store::stop_following(&tx, &gid)?;
            }
            tx.commit()?;
        }
        self.emit_outcome(gid, Outcome::Conversation)?;
        if others.is_empty() {
            return Ok(());
        }
        self.propose_leaving(gid)?;
        match self.flush_proposals().await {
            Ok(()) | Err(Error::NotConnected | Error::Network(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Propose removing each of this account's leaves that this device has
    /// not already proposed in the current epoch.
    fn propose_leaving(&mut self, gid: [u8; 16]) -> Result<()> {
        let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
        let tx = self.conn.transaction()?;
        {
            let provider = Provider::on(&tx);
            let mut group = load_group(&provider, &gid)?;
            if group.pending_commit().is_some() {
                return Ok(());
            }
            let own = group.own_leaf_index();
            let proposed: HashSet<LeafNodeIndex> = group
                .pending_proposals()
                .filter(|p| matches!(p.sender(), Sender::Member(i) if *i == own))
                .filter_map(|p| match p.proposal() {
                    Proposal::Remove(r) => Some(r.removed()),
                    _ => None,
                })
                .collect();
            let ours: Vec<LeafNodeIndex> = group
                .members()
                .filter(|m| leaf_of_member(m).is_ok_and(|l| l.account == me.account) && !proposed.contains(&m.index))
                .map(|m| m.index)
                .collect();
            let access_key = access_key_of(&group, &provider)?;
            let epoch = group.epoch().as_u64();
            for leaf in ours {
                let (msg, _) = group.propose_remove_member(&provider, &me.signer, leaf).map_err(mls)?;
                let id = random16()?;
                // Our own proposal comes back in the log, and this device
                // cannot decrypt its own messages: recognise it by id.
                store::record_own_send(&tx, &gid, &id)?;
                let req = proto::Send {
                    group_id: gid.to_vec(),
                    access_key: access_key.clone(),
                    client_msg_id: id.to_vec(),
                    message: msg.tls_serialize_detached().map_err(mls)?,
                };
                store::proposal_push(&tx, &id, &gid, epoch, &req.encode_to_vec())?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Send waiting proposals. One whose epoch has passed is dropped.
    async fn flush_proposals(&mut self) -> Result<()> {
        for w in store::proposals_waiting(&self.conn)? {
            let id = w.client_msg_id;
            let current = {
                let provider = Provider::on(&self.conn);
                load_group(&provider, &w.group_id).map(|g| g.epoch().as_u64()).ok()
            };
            if current != Some(w.epoch) {
                store::proposal_remove(&self.conn, &id)?;
                continue;
            }
            let req = proto::Send::decode(w.request.as_slice()).map_err(|_| Error::Corrupt("waiting proposal"))?;
            match self.anon()?.request(Request::Send(req)).await {
                Ok(_) => store::proposal_remove(&self.conn, &id)?,
                Err(Error::Server(e)) if matches!(e.code(), ErrorCode::WrongEpoch | ErrorCode::BadAccessKey) => {
                    store::proposal_remove(&self.conn, &id)?;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// For each group being left: propose again if a new epoch began, and
    /// send what is waiting.
    async fn continue_leaving(&mut self) -> Result<()> {
        for gid in store::leaving_groups(&self.conn)? {
            self.propose_leaving(gid)?;
        }
        self.flush_proposals().await
    }

    /// Build a group with `meta` and the devices behind `kps`, and register it.
    async fn create_conversation(&mut self, meta: GroupMeta, kps: Vec<KeyPackage>) -> Result<[u8; 16]> {
        let gid = random16()?;
        let request = {
            let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
            let tx = self.conn.transaction()?;
            let provider = Provider::on(&tx);
            let mut group = post_mls::group_builder(&meta)
                .map_err(mls)?
                .with_group_id(GroupId::from_slice(&gid))
                .build(&provider, &me.signer, me.credential())
                .map_err(mls)?;
            let (_, welcome, _) = group.add_members(&provider, &me.signer, &kps).map_err(mls)?;
            let staged = group.pending_commit().ok_or(Error::Corrupt("pending commit"))?;
            let access = staged.export_secret(provider.crypto(), ACCESS_LABEL, &[], 32).map_err(mls)?;
            let welcome_to = kps.iter().filter_map(|kp| post_mls::leaf_identity(kp.leaf_node()).ok()).map(|l| l.device.0.to_vec()).collect();
            let req = proto::CreateGroup {
                token: None,
                group_id: gid.to_vec(),
                access_hash: Sha256::digest(&access).to_vec(),
                welcome: welcome.tls_serialize_detached().map_err(mls)?,
                welcome_to,
            };
            store::pending_group_put(&tx, &gid, &req.encode_to_vec())?;
            drop(provider);
            tx.commit()?;
            req
        };
        self.finish_create(gid, request).await?;
        Ok(gid)
    }

    /// Register a created group with the server, then merge it locally.
    async fn finish_create(&mut self, gid: [u8; 16], mut req: proto::CreateGroup) -> Result<()> {
        req.token = Some(self.take_token().await?);
        match self.anon()?.request(Request::CreateGroup(req)).await {
            Ok(_) => {}
            // Group IDs are random, so this is our own earlier attempt whose
            // reply was lost.
            Err(Error::Server(e)) if e.code() == ErrorCode::GroupExists => {}
            Err(Error::Server(e)) => {
                let tx = self.conn.transaction()?;
                {
                    let provider = Provider::on(&tx);
                    if let Ok(mut g) = load_group(&provider, &gid) {
                        g.delete(provider.storage()).map_err(mls)?;
                    }
                }
                store::pending_group_remove(&tx, &gid)?;
                tx.commit()?;
                return Err(Error::Server(e));
            }
            Err(e) => return Err(e),
        }
        {
            let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
            let tx = self.conn.transaction()?;
            {
                let provider = Provider::on(&tx);
                let mut group = load_group(&provider, &gid)?;
                group.merge_pending_commit(&provider).map_err(mls)?;
                let meta = GroupMeta::from_extensions(group.extensions()).map_err(mls)?;
                install_conversation(&tx, &gid, &meta, &me.account)?;
            }
            store::pending_group_remove(&tx, &gid)?;
            tx.commit()?;
        }
        self.after_join(gid).await
    }

    // ------------------------------------------------------------ commits

    /// Stage a commit in `gid` adding `adds` and removing `removes`, record it,
    /// and send it. Returns false if another commit took the epoch first; the
    /// caller syncs and decides again. The commit is merged only when it shows
    /// up in the group's log, so messages logged before it in the same epoch
    /// are still read with the old keys.
    async fn commit_changes(
        &mut self,
        gid: [u8; 16],
        adds: Vec<KeyPackage>,
        removes: Vec<LeafNodeIndex>,
        meta: Option<GroupMeta>,
        refresh: bool,
    ) -> Result<bool> {
        let req = {
            let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
            let tx = self.conn.transaction()?;
            let req = {
                let provider = Provider::on(&tx);
                let mut group = load_group(&provider, &gid)?;
                tracing::info!(epoch = group.epoch().as_u64(), replacing = group.pending_commit().is_some(), "making a commit");
                let access_key = access_key_of(&group, &provider)?;
                let welcome_to = adds
                    .iter()
                    .filter_map(|kp| post_mls::leaf_identity(kp.leaf_node()).ok())
                    .map(|l| l.device.0.to_vec())
                    .collect();
                let mut builder = group.commit_builder().force_self_update(refresh).propose_adds(adds).propose_removals(removes);
                if let Some(meta) = &meta {
                    builder = builder.propose_group_context_extensions(post_mls::group_context_extensions(meta).map_err(mls)?).map_err(mls)?;
                }
                let bundle = builder
                    .load_psks(provider.storage())
                    .map_err(mls)?
                    .build(provider.rand(), provider.crypto(), &me.signer, |_| true)
                    .map_err(mls)?
                    .stage_commit(&provider)
                    .map_err(mls)?;
                let staged = group.pending_commit().ok_or(Error::Corrupt("pending commit"))?;
                let next = staged.export_secret(provider.crypto(), ACCESS_LABEL, &[], 32).map_err(mls)?;
                proto::Commit {
                    group_id: gid.to_vec(),
                    access_key,
                    next_access_hash: Sha256::digest(&next).to_vec(),
                    client_msg_id: random16()?.to_vec(),
                    commit: bundle.commit().tls_serialize_detached().map_err(mls)?,
                    welcome: bundle.to_welcome_msg().map(|w| w.tls_serialize_detached()).transpose().map_err(mls)?.unwrap_or_default(),
                    welcome_to,
                }
            };
            store::pending_commit_put(&tx, &gid, &req.client_msg_id, &req.encode_to_vec())?;
            tx.commit()?;
            req
        };
        self.send_commit(gid, req).await
    }

    /// Send a recorded commit. Retrying one the server already logged is
    /// harmless: it answers with the existing entry.
    async fn send_commit(&mut self, gid: [u8; 16], req: proto::Commit) -> Result<bool> {
        match self.anon()?.request(Request::Commit(req)).await {
            Ok(r) => {
                expect_reply!(r, Reply::Accepted)?;
                Ok(true)
            }
            Err(Error::Server(e)) if matches!(e.code(), ErrorCode::WrongEpoch | ErrorCode::BadAccessKey) => {
                self.abandon_commit(gid)?;
                Ok(false)
            }
            Err(Error::Server(e)) => {
                self.abandon_commit(gid)?;
                self.notice(Some(&gid), &format!("a change to the group's devices was refused: {}", e.message))?;
                Err(Error::Server(e))
            }
            // Unknown outcome: keep it, and retry at the next login.
            Err(e) => Err(e),
        }
    }

    /// Forget a pending commit that can no longer land.
    fn abandon_commit(&mut self, gid: [u8; 16]) -> Result<()> {
        tracing::info!("abandoning our pending commit");
        let tx = self.conn.transaction()?;
        {
            let provider = Provider::on(&tx);
            load_group(&provider, &gid)?.clear_pending_commit(provider.storage()).map_err(mls)?;
        }
        store::pending_commit_remove(&tx, &gid)?;
        tx.commit()?;
        Ok(())
    }

    async fn retry_pending_commits(&mut self) -> Result<()> {
        for (gid, req) in store::pending_commits(&self.conn)? {
            let req = proto::Commit::decode(req.as_slice()).map_err(|_| Error::Corrupt("pending commit"))?;
            self.send_commit(group_id_of(&gid)?, req).await?;
        }
        Ok(())
    }

    /// Our own commit reached the log: merge it.
    fn merge_own_commit(&mut self, gid: [u8; 16], seq: u64) -> Result<()> {
        tracing::info!(seq, "merging our own commit, seen in the log");
        let tx = self.conn.transaction()?;
        {
            let provider = Provider::on(&tx);
            let mut group = load_group(&provider, &gid)?;
            // A commit with a path gives this device's leaf fresh keys,
            // whatever else it did.
            if group.pending_commit().and_then(|s| s.update_path_leaf_node()).is_some() {
                store::set_refreshed(&tx, &gid)?;
            }
            group.merge_pending_commit(&provider).map_err(mls)?;
            let meta = GroupMeta::from_extensions(group.extensions()).map_err(mls)?;
            if meta.conversation_kind() == Some(MlsKind::Group) {
                store::set_name(&tx, &gid, &meta.name)?;
            }
        }
        store::pending_commit_remove(&tx, &gid)?;
        store::set_cursor(&tx, &gid, seq)?;
        tx.commit()?;
        Ok(())
    }

    // ------------------------------------------------------------ membership

    /// Bring every group in line with the verified device chains (design
    /// section 6): add this account's devices that are missing, and remove
    /// devices their accounts have removed. Any member may do either, so a
    /// change does not depend on one device; if two race, the server takes
    /// one commit per epoch and the other side finds nothing left to do.
    pub async fn reconcile(&mut self) -> Result<()> {
        for _ in 0..3 {
            let plans = self.plan_membership()?;
            if plans.is_empty() {
                return Ok(());
            }
            let mut pools = self.claim_for(&plans).await?;
            let mut lost_race = false;
            for p in plans {
                let adds: Vec<KeyPackage> = p.add.iter().filter_map(|d| pools.get_mut(d).and_then(KpPool::take)).collect();
                // A commit takes every pending proposal along with it.
                if adds.is_empty() && p.remove.is_empty() && !p.proposals && !p.refresh {
                    continue;
                }
                lost_race |= !self.commit_changes(p.gid, adds, p.remove, None, p.refresh).await?;
            }
            // Our commits are merged as they come back through the log.
            self.sync_groups().await?;
            if !lost_race {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Give this device's leaf fresh keys in every group now, instead of
    /// waiting for the weekly refresh: after a device may have been exposed.
    pub async fn refresh_keys(&mut self) -> Result<()> {
        self.refresh_before = store::now_ms();
        self.reconcile().await?;
        self.flush_outbox().await
    }

    fn plan_membership(&self) -> Result<Vec<Plan>> {
        let me = self.me()?;
        let Some(mine) = self.chains.get(&me.account) else { return Ok(Vec::new()) };
        if !mine.device(&me.device).is_some_and(|d| d.is_active()) {
            return Ok(Vec::new());
        }
        let provider = Provider::on(&self.conn);
        let mut out = Vec::new();
        for c in store::followed_groups(&self.conn)? {
            if store::pending_commit(&self.conn, &c.group_id)?.is_some() {
                continue;
            }
            let group = load_group(&provider, &c.group_id)?;
            // This account is leaving: another member must commit that, and
            // no device of ours may be added back meanwhile.
            let own = group.own_leaf_index();
            let ours_proposed = group.pending_proposals().any(|p| match p.proposal() {
                Proposal::Remove(r) => r.removed() == own,
                _ => false,
            });
            if ours_proposed || c.leaving {
                continue;
            }
            let proposals = group.pending_proposals().next().is_some();
            let leaves: Vec<(LeafNodeIndex, LeafIdentity)> =
                group.members().filter_map(|m| leaf_of_member(&m).ok().map(|l| (m.index, l))).collect();
            let present: HashSet<DeviceId> = leaves.iter().filter(|(_, l)| l.account == me.account).map(|(_, l)| l.device).collect();
            // Stale leaves are ours to remove if they are our own devices, or
            // anyone's if we administer this group.
            let meta = GroupMeta::from_extensions(group.extensions()).map_err(mls)?;
            let group_admin = c.kind == KIND_GROUP && meta.admins.iter().any(|a| a[..] == me.account.0);
            let mut remove = Vec::new();
            for (index, leaf) in &leaves {
                let removed = self.chains.get(&leaf.account).is_some_and(|ch| !ch.device(&leaf.device).is_some_and(|d| d.is_active()));
                let may_remove_stale = leaf.account == me.account || group_admin;
                if removed || (may_remove_stale && stale_and_replaceable(&self.stale, &leaves, leaf)) {
                    remove.push(*index);
                }
            }
            // A retired duplicate DM only needs stale leaves removed. Stale
            // devices of ours are added back once they return.
            let add: Vec<DeviceId> = if c.active {
                mine.active_devices()
                    .map(|d| d.id)
                    .filter(|d| !present.contains(d) && !self.backing_off(d) && !self.stale.contains(d))
                    .collect()
            } else {
                Vec::new()
            };
            let refresh = refresh_due(&c.group_id, c.refreshed_at, store::now_ms(), self.refresh_before);
            if !add.is_empty() || !remove.is_empty() || proposals || refresh {
                out.push(Plan { gid: c.group_id, add, remove, proposals, refresh });
            }
        }
        Ok(out)
    }

    fn backing_off(&self, device: &DeviceId) -> bool {
        self.no_key_packages.get(device).is_some_and(|t| t.elapsed() < KP_BACKOFF)
    }

    /// Claim, for each device the plans add, one KeyPackage per group it is
    /// missing from: one claim and one token per device.
    async fn claim_for(&mut self, plans: &[Plan]) -> Result<HashMap<DeviceId, KpPool>> {
        let mut wanted: HashMap<DeviceId, u32> = HashMap::new();
        for d in plans.iter().flat_map(|p| &p.add) {
            *wanted.entry(*d).or_default() += 1;
        }
        let account = self.me()?.account;
        let crypto = post_mls::crypto();
        let mut pools = HashMap::new();
        for (device, count) in wanted {
            let token = self.take_token().await?;
            let req = Request::ClaimKeyPackages(proto::ClaimKeyPackages {
                token: Some(token),
                account_id: account.0.to_vec(),
                device_id: device.0.to_vec(),
                count: count.min(proto::MAX_CLAIM),
            });
            let claimed = match self.anon()?.request(req).await {
                Ok(r) => expect_reply!(r, Reply::ClaimedKeyPackages)?.key_packages,
                Err(Error::Server(e)) if e.code() == ErrorCode::NotFound => Vec::new(),
                Err(e) => return Err(e),
            };
            let mut pool = KpPool::default();
            for c in claimed {
                let authorized = |v: &post_mls::ValidKeyPackage| {
                    v.leaf.account == account
                        && v.leaf.device == device
                        && self.chains.get(&account).is_some_and(|ch| ch.authorizes_leaf(&device, &v.leaf.signature_key))
                };
                match post_mls::validate_key_package(&c.key_package, &crypto) {
                    Ok(v) if authorized(&v) && v.last_resort => pool.last_resort = Some(v.key_package),
                    Ok(v) if authorized(&v) => pool.one_time.push(v.key_package),
                    _ => self.notice(None, "the server returned a KeyPackage that does not belong to the device asked for")?,
                }
            }
            if pool.is_empty() {
                // The device has not uploaded any yet. Try again later rather
                // than spend a token on every sync.
                self.no_key_packages.insert(device, std::time::Instant::now());
            } else {
                self.no_key_packages.remove(&device);
                pools.insert(device, pool);
            }
        }
        Ok(pools)
    }

    async fn retry_pending_groups(&mut self) -> Result<()> {
        for (gid, req) in store::pending_groups(&self.conn)? {
            let req = proto::CreateGroup::decode(req.as_slice()).map_err(|_| Error::Corrupt("pending group"))?;
            self.finish_create(group_id_of(&gid)?, req).await?;
        }
        Ok(())
    }

    /// After joining or creating a conversation: announce it, send our
    /// profile so the others see our name, and subscribe to the group so the
    /// server wakes us for it.
    async fn after_join(&mut self, gid: [u8; 16]) -> Result<()> {
        if let Some(c) = store::conversation(&self.conn, &gid)?
            && c.kind != KIND_SELF
        {
            self.emit(Event::ConversationAdded(crate::conversation_from_row(&self.conn, &c)?));
        }
        let mine = self.me()?.account;
        if let Some(p) = store::profile(&self.conn, &mine.0)? {
            self.queue_body(gid, Body::Profile(profile_body(&p)))?;
        }
        match self.flush_outbox().await {
            Ok(()) | Err(Error::NotConnected | Error::Network(_)) => {}
            Err(e) => return Err(e),
        }
        self.resubscribe().await
    }

    // ------------------------------------------------------------ sending

    /// Encrypt `body` for conversation `gid`, store it in the outbox, and try
    /// to send. Returns the new content's id.
    pub async fn send_body(&mut self, gid: [u8; 16], body: Body) -> Result<MessageId> {
        let id = self.queue_body(gid, body)?;
        match self.flush_outbox().await {
            Ok(()) | Err(Error::NotConnected | Error::Network(_)) => Ok(MessageId(id)),
            Err(e) => Err(e),
        }
    }

    /// The live group to send to for a conversation id: for a retired DM,
    /// the live DM with the same peer.
    fn live_group(&self, gid: [u8; 16]) -> Result<[u8; 16]> {
        let c = store::conversation(&self.conn, &gid)?.ok_or(Error::NotFound("conversation"))?;
        if c.active {
            return Ok(gid);
        }
        if c.kind == KIND_DM
            && let Some(peer) = c.peer_account.as_deref().map(store::account_id).transpose()?
            && let Some(live) = store::dm_with(&self.conn, &peer)?
        {
            return Ok(live.group_id);
        }
        Err(Error::NotFound("conversation"))
    }

    fn queue_body(&mut self, gid: [u8; 16], body: Body) -> Result<[u8; 16]> {
        let gid = self.live_group(gid)?;
        let conv = store::conversation(&self.conn, &gid)?.ok_or(Error::NotFound("conversation"))?;
        if !conv.following || conv.leaving {
            return Err(Error::InvalidInput("you are no longer in this conversation".into()));
        }
        if !body_allowed(&body, conv.kind == KIND_SELF) {
            return Err(Error::NotFound("conversation"));
        }
        let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
        // Validate before anything is written.
        match &body {
            Body::Text(t) if t.body.is_empty() || t.body.len() > content::MAX_TEXT => {
                return Err(Error::InvalidInput("message is empty or too long".into()));
            }
            Body::Edit(e) => {
                if e.body.is_empty() || e.body.len() > content::MAX_TEXT {
                    return Err(Error::InvalidInput("message is empty or too long".into()));
                }
                own_live_message(&self.conn, &gid, &e.target, &me.account)?;
            }
            Body::Delete(d) => own_live_message(&self.conn, &gid, &d.target, &me.account)?,
            Body::ConversationName(n) => content::check_display_name(&n.name).map_err(|e| Error::InvalidInput(e.to_string()))?,
            Body::SelfSync(s) => check_self_sync(s).map_err(Error::InvalidInput)?,
            Body::Call(c) => {
                c.check().map_err(|e| Error::InvalidInput(e.into()))?;
            }
            _ => {}
        }
        let id = random16()?;
        let now = store::now_ms();
        let chain = self.chains.get(&me.account).ok_or(Error::Corrupt("own chain"))?;
        let c = Content {
            id: id.to_vec(),
            sent_at_ms: now,
            sender_chain: Some(content::ChainHead { seq: chain.next_seq() - 1, head: chain.head().0.to_vec() }),
            body: Some(body.clone()),
        };
        let bytes = c.encode_to_vec();
        let tx = self.conn.transaction()?;
        // Queued unencrypted: it is encrypted when it reaches the front of
        // the outbox (`flush_outbox`), so generations follow send order.
        store::outbox_push(&tx, &store::OutboxRow { client_msg_id: id.to_vec(), group_id: gid.to_vec(), content: bytes, ciphertext: Vec::new() })?;
        store::record_own_send(&tx, &gid, &id)?;
        let outcome = match &body {
            Body::Text(t) => {
                store::insert_message(
                    &tx,
                    &NewMessage {
                        group: &gid,
                        id: &id,
                        sender_account: &me.account,
                        sender_device: &me.device,
                        outgoing: true,
                        sent_at: now,
                        seq: None,
                        state: store::STATE_PENDING,
                        body: Some(&t.body),
                    },
                )?;
                store::touch(&tx, &gid)?;
                Outcome::Added(id)
            }
            Body::Edit(e) => {
                store::add_version(&tx, &gid, &e.target, &id, None, &e.body, now)?;
                Outcome::Changed(group_id_of(&e.target)?)
            }
            Body::Delete(d) => {
                store::delete_message(&tx, &gid, &d.target)?;
                Outcome::Deleted(group_id_of(&d.target)?)
            }
            Body::ConversationName(n) => {
                store::set_conversation_name(&tx, &gid, &me.account.0, &n.name)?;
                Outcome::Conversation
            }
            Body::Profile(_) => Outcome::Nothing,
            // A snapshot only repeats what this device already holds.
            Body::SelfSync(s) if s.snapshot => Outcome::Nothing,
            Body::SelfSync(s) => Outcome::SelfSync(apply_self_sync(&tx, s, SyncSource::Local(Some(&id)))?),
            Body::Call(c) => Outcome::Call(calls::Seen { sender: me.account, device: me.device, sent_at: now, event: c.clone() }),
        };
        tx.commit()?;
        self.emit_outcome(gid, outcome)?;
        Ok(id)
    }

    /// Send whatever is in the outbox, oldest first.
    pub async fn flush_outbox(&mut self) -> Result<()> {
        let mut retries = 0;
        while let Some(row) = store::outbox_first(&self.conn)? {
            if self.anon().is_err() {
                return Ok(());
            }
            // Encrypted only at the front of the queue, just before it is
            // sent: each sender's generations must reach the server, and so
            // every member, in order. Encrypting when queued let messages
            // written after an epoch change take lower generations than the
            // older ones re-encrypted ahead of them, and members refused them
            // as too old. Retries resend these exact bytes.
            let row = if row.ciphertext.is_empty() { self.encrypt_queued(&row, false)? } else { row };
            let access = {
                let provider = Provider::on(&self.conn);
                access_key_of(&load_group(&provider, &row.group_id)?, &provider)?
            };
            let req = Request::Send(proto::Send {
                group_id: row.group_id.clone(),
                access_key: access,
                client_msg_id: row.client_msg_id.clone(),
                message: row.ciphertext.clone(),
            });
            let result = self.anon()?.request(req).await;
            match result {
                Ok(r) => {
                    let accepted = expect_reply!(r, Reply::Accepted)?;
                    let tx = self.conn.transaction()?;
                    store::mark_sent(&tx, &row.group_id, &row.client_msg_id, accepted.seq)?;
                    store::outbox_remove(&tx, &row.client_msg_id)?;
                    tx.commit()?;
                    retries = 0;
                    let gid = group_id_of(&row.group_id)?;
                    if let Some(m) = store::message(&self.conn, &gid, &row.client_msg_id)? {
                        self.emit(Event::MessageChanged(crate::message_from_row(&self.conn, &m)?));
                    }
                }
                // Our epoch is behind: catch up, re-encrypt in the new epoch,
                // and resend with the same id (design section 6, Ordering).
                Err(Error::Server(e)) if matches!(e.code(), ErrorCode::WrongEpoch | ErrorCode::BadAccessKey) && retries < 3 => {
                    tracing::info!(code = ?e.code(), message = %e.message, "the server refused a queued message; catching up");
                    retries += 1;
                    self.sync_groups().await?;
                    self.encrypt_queued(&row, true)?;
                }
                Err(Error::Server(e)) => {
                    let tx = self.conn.transaction()?;
                    store::mark_failed(&tx, &row.group_id, &row.client_msg_id)?;
                    store::outbox_remove(&tx, &row.client_msg_id)?;
                    tx.commit()?;
                    self.notice(Some(&row.group_id), &format!("a message could not be sent: {}", e.message))?;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Encrypt the message at the front of the outbox in the group's current
    /// epoch (`again`: its earlier ciphertext missed the epoch), saving the
    /// ciphertext with the advanced MLS state before anything is sent.
    fn encrypt_queued(&mut self, row: &store::OutboxRow, again: bool) -> Result<store::OutboxRow> {
        let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
        let tx = self.conn.transaction()?;
        let ciphertext = {
            let provider = Provider::on(&tx);
            let mut group = load_group(&provider, &row.group_id)?;
            if again {
                tracing::info!(epoch = group.epoch().as_u64(), "re-encrypting a queued message for a later epoch");
            }
            group.create_message(&provider, &me.signer, &row.content).map_err(mls)?.tls_serialize_detached().map_err(mls)?
        };
        store::outbox_replace_ciphertext(&tx, &row.client_msg_id, &ciphertext)?;
        tx.commit()?;
        Ok(store::OutboxRow { client_msg_id: row.client_msg_id.clone(), group_id: row.group_id.clone(), content: row.content.clone(), ciphertext })
    }

    // ------------------------------------------------------------ receiving

    /// Fetch and process everything new: mailbox, then groups, then send
    /// what is queued, then keep subscriptions current.
    pub async fn sync(&mut self) -> Result<()> {
        // The directory first: device changes and stale devices decide what
        // membership should be. It is one small request when nothing changed.
        self.sync_directory().await?;
        if std::mem::take(&mut self.announcements_due) | self.session()?.announcements.swap(false, std::sync::atomic::Ordering::AcqRel) {
            // Never in the way of messages: a server too old to have
            // announcements refuses the request, and that is all.
            match self.sync_announcements().await {
                Ok(()) => {}
                Err(e @ (Error::NotConnected | Error::Network(_))) => {
                    self.announcements_due = true;
                    return Err(e);
                }
                Err(e) => tracing::warn!(error = %e, "could not fetch announcements"),
            }
        }
        // Group logs before Welcomes: a device removed and added back while
        // it was offline must see its removal before the Welcome, which only
        // replaces a group the device is no longer in. Then the logs of
        // anything just joined.
        self.sync_groups().await?;
        let joined = self.sync_mailbox().await?;
        if joined > 0 {
            self.sync_groups().await?;
        }
        match self.reconcile().await {
            // Out of tokens for today: membership catches up tomorrow.
            Err(Error::NoTokens) => tracing::warn!("no tokens left to update group members"),
            r => r?,
        }
        self.continue_leaving().await?;
        self.flush_outbox().await?;
        self.resubscribe().await?;
        if joined > 0 {
            // Someone used one of our KeyPackages to add us.
            self.top_up_key_packages(None).await?;
            self.top_up_tokens().await?;
        }
        Ok(())
    }

    /// Fetch new announcements, whether this account is a host, and the
    /// maintenance plan.
    async fn sync_announcements(&mut self) -> Result<()> {
        let mut added = Vec::new();
        let mut changed = false;
        loop {
            let after = store::meta_u64(&self.conn, store::META_ANNOUNCEMENTS_SEEN)?;
            let req = Request::FetchAnnouncements(proto::FetchAnnouncements { after_id: after, limit: 0 });
            let r = expect_reply!(self.session()?.request(req).await?, Reply::Announcements)?;
            let tx = self.conn.transaction()?;
            if store::add_announcements(&tx, &r.entries)? > 0 {
                added.extend(r.entries.iter().filter(|a| a.id > after).cloned());
            }
            let host = if r.host { "1" } else { "0" };
            if store::meta_str(&tx, store::META_HOST)?.as_deref() != Some(host) {
                store::meta_put(&tx, store::META_HOST, host.as_bytes())?;
                changed = true;
            }
            let maintenance = r.maintenance.as_ref().map(prost::Message::encode_to_vec);
            if store::meta_get(&tx, store::META_MAINTENANCE)? != maintenance {
                match &maintenance {
                    Some(m) => store::meta_put(&tx, store::META_MAINTENANCE, m)?,
                    None => store::meta_delete(&tx, store::META_MAINTENANCE)?,
                }
                changed = true;
            }
            tx.commit()?;
            if !r.more {
                break;
            }
        }
        for a in &added {
            self.emit(Event::AnnouncementAdded(crate::Announcement {
                id: a.id,
                at_ms: a.created_at_ms,
                kind: crate::AnnouncementKind::from_wire(a.kind),
                text: a.text.clone(),
                starts_at_ms: a.starts_at_ms,
                minutes: a.minutes,
            }));
        }
        if changed || !added.is_empty() {
            self.emit(Event::AnnouncementsChanged(crate::announcements_from(&self.conn)?));
        }
        Ok(())
    }

    /// Mark every announcement read, and tell this account's other devices.
    pub async fn mark_announcements_read(&mut self) -> Result<()> {
        let Some(latest) = store::latest_announcement(&self.conn)? else { return Ok(()) };
        if !store::raise_announcements_read(&self.conn, latest.id)? {
            return Ok(());
        }
        self.emit(Event::AnnouncementsChanged(crate::announcements_from(&self.conn)?));
        if self.devices()?.len() > 1 {
            self.sync_to_own_devices(content::SelfSync { announcements_read: latest.id, ..Default::default() }).await?;
        }
        Ok(())
    }

    /// Post an announcement (hosts only), then fetch it back.
    pub async fn post_announcement(&mut self, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::InvalidInput("an announcement needs some text".into()));
        }
        if text.len() > proto::MAX_ANNOUNCEMENT_BYTES {
            return Err(Error::InvalidInput("the announcement is too long".into()));
        }
        let req = Request::Announce(proto::Announce { client_msg_id: random16()?.to_vec(), text: text.into() });
        expect_reply!(self.session()?.request(req).await?, Reply::Empty)?;
        self.sync_announcements().await
    }

    /// Plan maintenance (hosts only), then fetch what the server announced.
    pub async fn plan_maintenance(&mut self, starts_at_ms: u64, minutes: u32, reason: &str, quiet: bool) -> Result<()> {
        if minutes == 0 || minutes > proto::MAX_MAINTENANCE_MINUTES {
            return Err(Error::InvalidInput("maintenance lasts from a minute to a week".into()));
        }
        if reason.len() > proto::MAX_ANNOUNCEMENT_BYTES {
            return Err(Error::InvalidInput("the reason is too long".into()));
        }
        let req = Request::PlanMaintenance(proto::PlanMaintenance { starts_at_ms, minutes, reason: reason.trim().into(), quiet });
        expect_reply!(self.session()?.request(req).await?, Reply::Empty)?;
        self.sync_announcements().await
    }

    /// Cancel or end maintenance (hosts only), then fetch the announcement.
    pub async fn end_maintenance(&mut self) -> Result<()> {
        expect_reply!(self.session()?.request(Request::EndMaintenance(proto::EndMaintenance {})).await?, Reply::Empty)?;
        self.sync_announcements().await
    }

    async fn sync_mailbox(&mut self) -> Result<usize> {
        let mut joined = 0;
        loop {
            let mb = expect_reply!(self.session()?.request(Request::FetchMailbox(proto::FetchMailbox { limit: 0 })).await?, Reply::Mailbox)?;
            let Some(last) = mb.entries.last().map(|e| e.seq) else { break };
            for e in &mb.entries {
                if self.process_welcome(e).await? {
                    joined += 1;
                }
            }
            self.session()?.request(Request::AckMailbox(proto::AckMailbox { seq: last })).await?;
            if !mb.more {
                break;
            }
        }
        Ok(joined)
    }

    /// Join a group from a Welcome after checking every member. Returns
    /// whether a new conversation was joined.
    async fn process_welcome(&mut self, entry: &proto::MailboxEntry) -> Result<bool> {
        if entry.seq <= store::meta_u64(&self.conn, "mailbox_seq")? {
            return Ok(false);
        }
        for attempt in 0..2 {
            // All database work finishes inside this block, so no transaction
            // is ever held across a network call.
            let step = {
                let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
                let tx = self.conn.transaction()?;
                match join_from_welcome(&tx, me, &self.chains, &entry.welcome) {
                    Ok(joined) => {
                        store::meta_put_u64(&tx, "mailbox_seq", entry.seq)?;
                        tx.commit()?;
                        Step::Done(joined)
                    }
                    Err(Fail::NeedDirectory) if attempt == 0 => Step::Retry,
                    Err(Fail::NeedDirectory) => Step::Refused("an invitation listed a device its account does not authorize".into()),
                    Err(Fail::Reject(m)) => Step::Refused(m),
                    Err(Fail::Fatal(e)) => return Err(e),
                }
            };
            match step {
                Step::Done(Some(gid)) => {
                    self.after_join(gid).await?;
                    return Ok(true);
                }
                Step::Done(None) => return Ok(false),
                Step::Retry => self.sync_directory().await?,
                Step::Refused(msg) => {
                    store::meta_put_u64(&self.conn, "mailbox_seq", entry.seq)?;
                    self.notice(None, &format!("an invitation to a conversation was refused: {msg}"))?;
                    return Ok(false);
                }
            }
        }
        Ok(false)
    }

    /// Positions in every followed group, with current access keys.
    fn cursors(&self) -> Result<Vec<proto::GroupCursor>> {
        let provider = Provider::on(&self.conn);
        let mut out = Vec::new();
        for c in store::followed_groups(&self.conn)? {
            let group = load_group(&provider, &c.group_id)?;
            out.push(proto::GroupCursor {
                group_id: c.group_id.to_vec(),
                epoch: group.epoch().as_u64(),
                access_key: access_key_of(&group, &provider)?,
                after_seq: c.cursor,
            });
        }
        Ok(out)
    }

    /// Fetch and process new group log entries until there are none.
    pub async fn sync_groups(&mut self) -> Result<()> {
        for _ in 0..MAX_FETCH_ROUNDS {
            let cursors = self.cursors()?;
            if cursors.is_empty() {
                return Ok(());
            }
            let req = Request::FetchGroups(proto::FetchGroups { groups: cursors, limit: 0 });
            let batch = expect_reply!(self.anon()?.request(req).await?, Reply::GroupBatch)?;
            let mut progressed = false;
            for g in batch.groups {
                let gid = group_id_of(&g.group_id)?;
                if let Some(e) = g.error {
                    if e.code() == ErrorCode::BadAccessKey {
                        // The server keeps every epoch a member could still be
                        // in for 30 days, so this device fell behind for longer
                        // than that, or the group is gone. Stop following it;
                        // a member adding it back brings it in again.
                        store::stop_following(&self.conn, &gid)?;
                        self.notice(Some(&gid), "this device was away too long to catch up with this conversation; a member can add you back")?;
                    } else {
                        tracing::debug!(error = %e, "group fetch refused");
                    }
                    continue;
                }
                for e in &g.entries {
                    self.process_entry(gid, e).await?;
                    progressed = true;
                }
            }
            if !progressed && !batch.more {
                return Ok(());
            }
        }
        Err(Error::Protocol("server kept returning entries".into()))
    }

    async fn process_entry(&mut self, gid: [u8; 16], e: &proto::LogEntry) -> Result<()> {
        let Some(conv) = store::conversation(&self.conn, &gid)? else { return Ok(()) };
        if e.seq <= conv.cursor {
            return Ok(());
        }
        if let Some((id, req)) = store::pending_commit(&self.conn, &gid)?
            && id == e.client_msg_id
        {
            self.merge_own_commit(gid, e.seq)?;
            let req = proto::Commit::decode(req.as_slice()).map_err(|_| Error::Corrupt("pending commit"))?;
            if conv.kind == KIND_SELF && !req.welcome_to.is_empty() {
                // A new device of ours joined: give it our profile and
                // everything the self group keeps in step.
                self.share_profile(gid)?;
                let snapshot = self_snapshot(&self.conn)?;
                self.queue_body(gid, Body::SelfSync(snapshot))?;
            } else if self.welcomes_another_account(&req)? {
                self.share_profile(gid)?;
            }
            return self.emit_outcome(gid, Outcome::Conversation);
        }
        if store::is_own_send(&self.conn, &gid, &e.client_msg_id)? {
            let tx = self.conn.transaction()?;
            store::mark_sent(&tx, &gid, &e.client_msg_id, e.seq)?;
            store::set_cursor(&tx, &gid, e.seq)?;
            tx.commit()?;
            return Ok(());
        }
        let mine = self.me()?.account;
        for attempt in 0..2 {
            let step = {
                let tx = self.conn.transaction()?;
                match process_in_tx(&tx, &mine, &self.chains, &gid, e) {
                    Ok(outcome) => {
                        store::set_cursor(&tx, &gid, e.seq)?;
                        tx.commit()?;
                        Step::Done(outcome)
                    }
                    Err(Fail::NeedDirectory) if attempt == 0 => Step::Retry,
                    Err(Fail::NeedDirectory) => Step::Refused("a member's device is not authorized by its account".into()),
                    Err(Fail::Reject(m)) => Step::Refused(m),
                    Err(Fail::Fatal(err)) => return Err(err),
                }
            };
            match step {
                Step::Done((outcome, gossip)) => {
                    let new_members = matches!(outcome, Outcome::NewMembers);
                    self.emit_outcome(gid, outcome)?;
                    if new_members {
                        self.share_profile(gid)?;
                    }
                    if let Some((account, head)) = gossip {
                        self.check_gossip(account, &head).await?;
                    }
                    return Ok(());
                }
                Step::Retry => self.sync_directory().await?,
                Step::Refused(msg) => {
                    // Fail closed but visibly: skip the entry, tell the user.
                    tracing::warn!(seq = e.seq, epoch = e.epoch, kind = e.kind, "skipping a log entry that could not be processed");
                    store::set_cursor(&self.conn, &gid, e.seq)?;
                    self.notice(Some(&gid), &msg)?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Queue this account's profile into a group, for members who lack it.
    fn share_profile(&mut self, gid: [u8; 16]) -> Result<()> {
        let mine = self.me()?.account;
        match store::profile(&self.conn, &mine.0)? {
            Some(p) => self.queue_body(gid, Body::Profile(profile_body(&p))).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Whether a commit of ours welcomed a device of some other account.
    fn welcomes_another_account(&self, req: &proto::Commit) -> Result<bool> {
        let me = self.me()?;
        let mine = self.chains.get(&me.account).ok_or(Error::Corrupt("own chain"))?;
        Ok(req.welcome_to.iter().any(|d| DeviceId::from_slice(d).is_ok_and(|d| mine.device(&d).is_none())))
    }

    /// Every message carries its sender's chain head (design section 5,
    /// "Protection against a withholding server"). If this device's copy is
    /// behind, fetch the directory; if the server still cannot produce the
    /// links, or a different link sits at that position, warn the user once:
    /// the server may be hiding a device change, such as a stolen phone's
    /// removal. Messages from this account's other devices check our own chain.
    async fn check_gossip(&mut self, account: AccountId, head: &content::ChainHead) -> Result<()> {
        if self.gossip_warned.contains(&(account, head.seq)) {
            return Ok(());
        }
        let mut verdict = match self.chains.get(&account) {
            Some(chain) => compare_gossip(chain, head),
            None => return Ok(()),
        };
        if verdict == Gossip::Ahead {
            self.sync_directory().await?;
            verdict = self.chains.get(&account).map_or(Gossip::Ahead, |c| compare_gossip(c, head));
        }
        let who = store::username_of(&self.conn, &account.0)?.unwrap_or_else(|| "someone".into());
        let warning = match verdict {
            Gossip::Matches => return Ok(()),
            Gossip::Ahead => format!(
                "{who}'s messages mention device changes the server has not delivered; it may be hiding one. Compare safety numbers and device lists with {who}."
            ),
            Gossip::Conflicts => format!(
                "{who}'s messages and the server disagree about {who}'s devices. Do not trust new devices for {who} until you have compared safety numbers."
            ),
        };
        self.gossip_warned.insert((account, head.seq));
        self.notice(None, &warning)
    }

    fn emit_outcome(&self, gid: [u8; 16], outcome: Outcome) -> Result<()> {
        let conversation = ConversationId(gid);
        match outcome {
            Outcome::Nothing => {}
            Outcome::Added(id) | Outcome::Changed(id) => {
                if let Some(m) = store::message(&self.conn, &gid, &id)? {
                    let m = crate::message_from_row(&self.conn, &m)?;
                    self.emit(if matches!(outcome, Outcome::Added(_)) { Event::MessageAdded(m) } else { Event::MessageChanged(m) });
                }
            }
            Outcome::Deleted(id) => self.emit(Event::MessageDeleted { conversation, id: MessageId(id) }),
            Outcome::Profile(account) => {
                if let Some(username) = store::username_of(&self.conn, &account.0)? {
                    self.emit(Event::ProfileChanged { username });
                }
            }
            Outcome::SelfSync(changes) => {
                for a in changes.people {
                    if let Some(username) = store::username_of(&self.conn, &a.0)? {
                        self.emit(Event::ProfileChanged { username });
                    }
                }
                for g in changes.read {
                    self.emit_conversation_of(&g)?;
                }
                if changes.announcements {
                    self.emit(Event::AnnouncementsChanged(crate::announcements_from(&self.conn)?));
                }
            }
            Outcome::Conversation | Outcome::NewMembers => {
                if let Some(c) = store::conversation(&self.conn, &gid)?
                    && c.kind != KIND_SELF
                {
                    self.emit(Event::ConversationChanged(crate::conversation_from_row(&self.conn, &c)?));
                }
            }
            Outcome::Call(seen) => self.apply_call(gid, &seen)?,
        }
        Ok(())
    }

    // ------------------------------------------------------------ calls

    /// Apply a call event: update the live call, add its notice to the
    /// timeline, and tell the app.
    fn apply_call(&self, gid: [u8; 16], seen: &calls::Seen) -> Result<()> {
        let Some(me) = self.me.as_ref() else { return Ok(()) };
        let is_dm = store::conversation(&self.conn, &gid)?.is_some_and(|c| c.kind == KIND_DM);
        let viewer = calls::Viewer { account: me.account, device: me.device, is_dm, now: store::now_ms() };
        let mut live = self.calls.lock().unwrap_or_else(|p| p.into_inner());
        let (id, change) = live.apply(gid, &viewer, seen);
        // Notices are kept starting in lower case; the app capitalises them.
        let name = |account: &AccountId| -> Result<String> {
            if account == &me.account { Ok("you".into()) } else { store::shown_name(&self.conn, Some(&gid), &account.0) }
        };
        let kind = if seen.event.video { "a video call" } else { "a call" };
        let notice = match &change {
            calls::Change::Started => Some(format!("{} started {kind}", name(&seen.sender)?)),
            calls::Change::StartedLongAgo if seen.sender != me.account => Some(format!("{} called", name(&seen.sender)?)),
            calls::Change::Ended { missed, lasted_ms } => {
                let starter = live.get(&id).map(|c| c.started_by).unwrap_or(seen.sender);
                Some(match lasted_ms {
                    _ if *missed => format!("missed call from {}", name(&starter)?),
                    None if starter == me.account => "no one answered".into(),
                    Some(ms) => format!("call ended · {}", calls::lasted(*ms)),
                    None => "call ended".into(),
                })
            }
            _ => None,
        };
        if let Some(text) = notice {
            let missed = matches!(change, calls::Change::Ended { missed: true, .. });
            store::add_notice_of(&self.conn, Some(&gid), if missed { "missed_call" } else { "call" }, &text)?;
        }
        if change == calls::Change::Nothing || change == calls::Change::StartedLongAgo {
            return Ok(());
        }
        if let Some(state) = live.get(&id) {
            let call = self.call_view(&id, state)?;
            if state.ended {
                live.remove(&id);
            }
            drop(live);
            self.emit(Event::CallChanged(call));
        }
        Ok(())
    }

    /// A live call as the app sees it.
    fn call_view(&self, id: &[u8; 16], c: &calls::CallState) -> Result<crate::Call> {
        let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
        let username = |a: &AccountId| -> Result<String> { Ok(store::username_of(&self.conn, &a.0)?.unwrap_or_default()) };
        let participants = c
            .participants
            .iter()
            .map(|(p, (account, device))| {
                Ok(crate::CallParticipant {
                    participant_id: hex::encode(p),
                    username: username(account)?,
                    you: *account == me.account,
                    this_device: *account == me.account && *device == me.device,
                })
            })
            .collect::<Result<_>>()?;
        Ok(crate::Call {
            id: crate::CallId(*id),
            conversation: ConversationId(c.group),
            started_by: username(&c.started_by)?,
            started_at_ms: c.started_at,
            video: c.video,
            participants,
            ringing: c.ringing(store::now_ms()),
            joined: c.mine.is_some(),
            ended: c.ended,
        })
    }

    /// The live calls, in every conversation.
    pub fn calls(&self) -> Result<Vec<crate::Call>> {
        let live = self.calls.lock().unwrap_or_else(|p| p.into_inner());
        live.all().map(|(id, c)| self.call_view(id, c)).collect()
    }

    /// Start a call in a conversation, with this device in it, ringing
    /// `ring` (usernames) or, with none named, everyone in a DM or a small
    /// group. A larger group is not rung: the call shows there to join.
    pub async fn start_call(&mut self, gid: [u8; 16], video: bool, ring: &[String]) -> Result<crate::CallJoin> {
        let gid = self.live_group(gid)?;
        let (ring, members) = self.ring_targets(&gid, ring)?;
        let (call_id, participant) = (random16()?, random16()?);
        // The token first: a server without calls refuses it, and nobody
        // is rung for a call that cannot happen.
        let access = self.call_access(gid, call_id, participant).await?;
        let event = content::CallEvent {
            kind: content::CallKind::Start as i32,
            call_id: call_id.to_vec(),
            participant_id: participant.to_vec(),
            video,
            quiet: ring.is_empty() && members > calls::RING_ALL_MAX,
            ring,
        };
        self.send_body(gid, Body::Call(event)).await?;
        self.call_join(call_id, participant, access)
    }

    /// Ring people (usernames) into a live call: members not in it.
    pub async fn ring_call(&mut self, gid: [u8; 16], call_id: [u8; 16], usernames: &[String]) -> Result<()> {
        let gid = self.live_group(gid)?;
        if usernames.is_empty() {
            return Err(Error::InvalidInput("name someone to ring".into()));
        }
        let in_call: HashSet<AccountId> = {
            let live = self.calls.lock().unwrap_or_else(|p| p.into_inner());
            let call = live.get(&call_id).filter(|c| c.group == gid && !c.ended).ok_or(Error::NotFound("call"))?;
            call.participants.values().map(|(a, _)| *a).collect()
        };
        let (ring, _) = self.ring_targets(&gid, usernames)?;
        let ring: Vec<Vec<u8>> = ring.into_iter().filter(|a| !in_call.iter().any(|i| i.0.as_slice() == a.as_slice())).collect();
        if ring.is_empty() {
            return Err(Error::InvalidInput("they are already in the call".into()));
        }
        let event = content::CallEvent { kind: content::CallKind::Ring as i32, call_id: call_id.to_vec(), ring, ..Default::default() };
        self.send_body(gid, Body::Call(event)).await.map(|_| ())
    }

    /// The accounts of `usernames`, each a member of the group other than
    /// the user, and how many members the group has.
    fn ring_targets(&self, gid: &[u8; 16], usernames: &[String]) -> Result<(Vec<Vec<u8>>, usize)> {
        let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
        let (_, members) = group_members(&self.conn, gid)?;
        let mut ring = Vec::new();
        for u in usernames {
            let (account, _) = store::account_by_username(&self.conn, u)?.ok_or_else(|| Error::UnknownUser(u.clone()))?;
            if account == me.account || !members.iter().any(|(a, _)| *a == account) {
                return Err(Error::InvalidInput(format!("{u} is not someone else in this conversation")));
            }
            if !ring.contains(&account.0.to_vec()) {
                ring.push(account.0.to_vec());
            }
        }
        Ok((ring, members.len()))
    }

    /// Join a live call from this device.
    pub async fn join_call(&mut self, gid: [u8; 16], call_id: [u8; 16]) -> Result<crate::CallJoin> {
        let gid = self.live_group(gid)?;
        let already = {
            let live = self.calls.lock().unwrap_or_else(|p| p.into_inner());
            live.get(&call_id).filter(|c| c.group == gid && !c.ended).ok_or(Error::NotFound("call"))?.mine
        };
        // Already in it (rejoining after a dropped connection, say).
        if let Some(p) = already {
            let access = self.call_access(gid, call_id, p).await?;
            return self.call_join(call_id, p, access);
        }
        let participant = random16()?;
        let access = self.call_access(gid, call_id, participant).await?;
        let event = content::CallEvent { kind: content::CallKind::Join as i32, call_id: call_id.to_vec(), participant_id: participant.to_vec(), ..Default::default() };
        self.send_body(gid, Body::Call(event)).await?;
        self.call_join(call_id, participant, access)
    }

    /// Leave a call this device is in. In a DM, hanging up ends the call.
    pub async fn leave_call(&mut self, gid: [u8; 16], call_id: [u8; 16]) -> Result<()> {
        let gid = self.live_group(gid)?;
        let mine = {
            let live = self.calls.lock().unwrap_or_else(|p| p.into_inner());
            live.get(&call_id).filter(|c| c.group == gid).and_then(|c| c.mine)
        };
        let is_dm = store::conversation(&self.conn, &gid)?.is_some_and(|c| c.kind == KIND_DM);
        let end = content::CallEvent { kind: content::CallKind::End as i32, call_id: call_id.to_vec(), ..Default::default() };
        let Some(participant) = mine else {
            // Not in it here (a device that has restarted, or a one-shot
            // CLI): in a DM, hanging up from any device ends the call.
            return if is_dm { self.send_body(gid, Body::Call(end)).await.map(|_| ()) } else { Err(Error::NotFound("call")) };
        };
        let leave = content::CallEvent { kind: content::CallKind::Leave as i32, call_id: call_id.to_vec(), participant_id: participant.to_vec(), ..Default::default() };
        self.send_body(gid, Body::Call(leave)).await?;
        let still_live = self.calls.lock().unwrap_or_else(|p| p.into_inner()).get(&call_id).is_some();
        if is_dm && still_live {
            self.send_body(gid, Body::Call(end)).await?;
        }
        Ok(())
    }

    /// Decline a call that is ringing. In a DM this ends it for the caller.
    pub async fn decline_call(&mut self, gid: [u8; 16], call_id: [u8; 16]) -> Result<()> {
        let gid = self.live_group(gid)?;
        let event = content::CallEvent { kind: content::CallKind::Decline as i32, call_id: call_id.to_vec(), ..Default::default() };
        self.send_body(gid, Body::Call(event)).await.map(|_| ())
    }

    /// The media keys for a call under the conversation's current epoch.
    pub fn call_keys(&self, gid: [u8; 16], call_id: [u8; 16]) -> Result<crate::CallKeys> {
        let gid = self.live_group(gid)?;
        let participants: Vec<[u8; 16]> = {
            let live = self.calls.lock().unwrap_or_else(|p| p.into_inner());
            let call = live.get(&call_id).filter(|c| c.group == gid).ok_or(Error::NotFound("call"))?;
            call.participants.keys().copied().collect()
        };
        let provider = Provider::on(&self.conn);
        let group = load_group(&provider, &gid)?;
        let base = Zeroizing::new(group.export_secret(provider.crypto(), calls::EXPORTER_LABEL, &call_id, 32).map_err(mls)?);
        Ok(crate::CallKeys {
            key_index: (group.epoch().as_u64() % calls::KEY_RING) as u32,
            keys: participants.iter().map(|p| (hex::encode(p), calls::sender_key(&base, &call_id, p))).collect(),
        })
    }

    /// A LiveKit token for this device's participant ID in a call, asked of
    /// the server with the group's access key.
    async fn call_access(&mut self, gid: [u8; 16], call_id: [u8; 16], participant: [u8; 16]) -> Result<proto::CallAccess> {
        let access_key = {
            let provider = Provider::on(&self.conn);
            access_key_of(&load_group(&provider, &gid)?, &provider)?
        };
        let req = proto::CallToken { group_id: gid.to_vec(), access_key, call_id: call_id.to_vec(), participant_id: participant.to_vec() };
        match self.anon()?.request(Request::CallToken(req)).await? {
            Reply::CallAccess(access) => Ok(access),
            _ => Err(Error::Protocol("expected call access".into())),
        }
    }

    /// What the app needs to join, once this device is in the call.
    fn call_join(&self, call_id: [u8; 16], participant: [u8; 16], access: proto::CallAccess) -> Result<crate::CallJoin> {
        let call = {
            let live = self.calls.lock().unwrap_or_else(|p| p.into_inner());
            let state = live.get(&call_id).ok_or(Error::NotFound("call"))?;
            self.call_view(&call_id, state)?
        };
        Ok(crate::CallJoin {
            call,
            participant_id: hex::encode(participant),
            url: access.url,
            token: access.token,
            max_bitrate_kbps: access.max_bitrate_kbps,
            max_participants: access.max_participants,
        })
    }

    /// Emit [`Event::ConversationChanged`] for the conversation a group is
    /// shown in: itself, or for a retired duplicate DM, the live one.
    fn emit_conversation_of(&self, gid: &[u8]) -> Result<()> {
        let Some(c) = store::conversation(&self.conn, gid)? else { return Ok(()) };
        let shown = match (&c.peer_account, c.kind, c.active) {
            (Some(peer), KIND_DM, false) => store::dm_with(&self.conn, &store::account_id(peer)?)?,
            (_, KIND_SELF, _) => None,
            _ => Some(c),
        };
        if let Some(c) = shown {
            self.emit(Event::ConversationChanged(crate::conversation_from_row(&self.conn, &c)?));
        }
        Ok(())
    }

    /// Mark a conversation read, and tell this account's other devices.
    pub async fn mark_read(&mut self, gid: [u8; 16]) -> Result<()> {
        let groups = store::conversation_groups(&self.conn, &gid)?;
        if groups.is_empty() {
            return Err(Error::NotFound("conversation"));
        }
        let moved = store::mark_read(&self.conn, &groups)?;
        if moved.is_empty() {
            return Ok(());
        }
        self.emit_conversation_of(&gid)?;
        // Only worth a message when another device would read it.
        if self.devices()?.len() > 1 {
            let read = moved.into_iter().map(|(group, seq)| content::ReadEntry { group, seq }).collect();
            self.sync_to_own_devices(content::SelfSync { read, ..Default::default() }).await?;
        }
        Ok(())
    }

    /// Mark (or unmark) someone's safety number as verified, on every one of
    /// this account's devices.
    pub async fn set_verified(&mut self, username: &str, verified: bool) -> Result<()> {
        let account = self.resolve(username).await?;
        let entry = content::VerifiedEntry { account: account.0.to_vec(), verified };
        self.sync_to_own_devices(content::SelfSync { verified: vec![entry], ..Default::default() }).await?;
        self.emit(Event::ProfileChanged { username: username.into() });
        Ok(())
    }

    async fn resubscribe(&mut self) -> Result<()> {
        let cursors = self.cursors()?;
        let now: Vec<(Vec<u8>, u64)> = cursors.iter().map(|c| (c.group_id.clone(), c.epoch)).collect();
        if now == self.subscribed {
            return Ok(());
        }
        self.anon()?.request(Request::Subscribe(proto::Subscribe { groups: cursors })).await?;
        self.subscribed = now;
        Ok(())
    }

    // ------------------------------------------------------------ typing and presence

    /// Tell a conversation's members who are online that the user is typing.
    /// Call it as the user types; it sends at most once every three seconds.
    pub async fn send_typing(&mut self, gid: [u8; 16]) -> Result<()> {
        let gid = self.live_group(gid)?;
        let conv = store::conversation(&self.conn, &gid)?.ok_or(Error::NotFound("conversation"))?;
        if !conv.following || conv.leaving || conv.kind == KIND_SELF {
            return Err(Error::InvalidInput("you are no longer in this conversation".into()));
        }
        if self.typing_sent.get(&gid).is_some_and(|t| t.elapsed() < TYPING_EVERY) {
            return Ok(());
        }
        self.typing_sent.insert(gid, std::time::Instant::now());
        self.send_signal(gid, ephemeral::Kind::Typing(ephemeral::Typing {})).await
    }

    /// The presence this account shows, as chosen on this device.
    pub fn own_presence(&self) -> Result<PresenceSetting> {
        Ok(match store::meta_str(&self.conn, "presence")?.as_deref() {
            Some("away") => PresenceSetting::Away,
            Some("dnd") => PresenceSetting::DoNotDisturb,
            Some("invisible") => PresenceSetting::Invisible,
            _ => PresenceSetting::Online,
        })
    }

    /// Choose the presence others see, and announce it to every conversation.
    pub async fn set_presence(&mut self, setting: PresenceSetting) -> Result<()> {
        let word = match setting {
            PresenceSetting::Online => "online",
            PresenceSetting::Away => "away",
            PresenceSetting::DoNotDisturb => "dnd",
            PresenceSetting::Invisible => "invisible",
        };
        store::meta_put(&self.conn, "presence", word.as_bytes())?;
        // Going invisible announces "offline" once, then nothing.
        let state = match setting {
            PresenceSetting::Online => ephemeral::State::Online,
            PresenceSetting::Away => ephemeral::State::Away,
            PresenceSetting::DoNotDisturb => ephemeral::State::DoNotDisturb,
            PresenceSetting::Invisible => ephemeral::State::Offline,
        };
        self.announce(state, false).await
    }

    /// Repeat this account's presence, as a running app does every ten
    /// minutes. Invisible sends nothing. Groups quiet for a month are
    /// skipped, to keep relay traffic small.
    pub async fn announce_presence(&mut self) -> Result<()> {
        let state = match self.own_presence()? {
            PresenceSetting::Online => ephemeral::State::Online,
            PresenceSetting::Away => ephemeral::State::Away,
            PresenceSetting::DoNotDisturb => ephemeral::State::DoNotDisturb,
            PresenceSetting::Invisible => return Ok(()),
        };
        self.announce(state, true).await
    }

    async fn announce(&mut self, state: ephemeral::State, active_only: bool) -> Result<()> {
        let cutoff = store::now_ms().saturating_sub(QUIET_GROUP_MS) as i64;
        let groups: Vec<[u8; 16]> = store::broadcast_groups(&self.conn)?
            .into_iter()
            .filter(|g| !active_only || store::conversation(&self.conn, g).ok().flatten().is_some_and(|c| c.last_activity >= cutoff))
            .collect();
        for (i, gid) in groups.into_iter().enumerate() {
            // The server relays at most 20 signals a second per connection.
            if i > 0 && i % SIGNALS_PER_BURST == 0 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            self.send_signal(gid, ephemeral::Kind::Presence(ephemeral::PresenceSignal { state: state as i32 })).await?;
        }
        Ok(())
    }

    /// Sign, seal and relay one signal. Signals are best effort: one that
    /// misses its epoch or a rate limit is simply dropped.
    async fn send_signal(&mut self, gid: [u8; 16], kind: ephemeral::Kind) -> Result<()> {
        let (payload, access_key) = {
            let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
            let provider = Provider::on(&self.conn);
            let group = load_group(&provider, &gid)?;
            let key = Zeroizing::new(group.export_secret(provider.crypto(), ephemeral::EXPORTER_LABEL, &[], 32).map_err(mls)?);
            let ctx = ephemeral::Context { group_id: &gid, epoch: group.epoch().as_u64(), key };
            let signal = ephemeral::Signal {
                account: me.account.0.to_vec(),
                device: me.device.0.to_vec(),
                sent_at_ms: store::now_ms(),
                kind: Some(kind),
            };
            (ctx.seal(&signal, &me.keys.mls)?, access_key_of(&group, &provider)?)
        };
        let req = proto::SendEphemeral { group_id: gid.to_vec(), access_key, payload };
        match self.anon()?.request(Request::SendEphemeral(req)).await {
            Ok(_) => Ok(()),
            Err(Error::Server(e)) if matches!(e.code(), ErrorCode::WrongEpoch | ErrorCode::BadAccessKey | ErrorCode::RateLimited) => {
                tracing::debug!(error = %e, "signal dropped");
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Open the signals the server relayed, and report the ones that check
    /// out. Never touches the database.
    pub fn process_signals(&mut self) -> Result<()> {
        let Ok(anon) = self.anon() else { return Ok(()) };
        let waiting = anon.signals.take();
        let mine = self.me()?.account;
        for (group_id, payload) in waiting {
            let Ok(gid) = group_id_of(&group_id) else { continue };
            let Some(conv) = store::conversation(&self.conn, &gid)? else { continue };
            if !conv.following || conv.leaving {
                continue;
            }
            let opened = {
                let provider = Provider::on(&self.conn);
                let group = load_group(&provider, &gid)?;
                let key = Zeroizing::new(group.export_secret(provider.crypto(), ephemeral::EXPORTER_LABEL, &[], 32).map_err(mls)?);
                let ctx = ephemeral::Context { group_id: &gid, epoch: group.epoch().as_u64(), key };
                let leaf_key = |a: &AccountId, d: &DeviceId| {
                    group.members().find_map(|m| leaf_of_member(&m).ok().filter(|l| l.account == *a && l.device == *d).map(|l| l.signature_key))
                };
                ctx.open(&payload, leaf_key, store::now_ms())
            };
            let signal = match opened {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(error = %e);
                    continue;
                }
            };
            let Ok(account) = store::account_id(&signal.account) else { continue };
            if account == mine {
                continue;
            }
            let Some(username) = store::username_of(&self.conn, &account.0)? else { continue };
            match signal.kind {
                Some(ephemeral::Kind::Typing(_)) if conv.kind != KIND_SELF => {
                    let template = store::profile(&self.conn, &account.0)?.map(|p| p.typing_template).unwrap_or_default();
                    let name = store::shown_name(&self.conn, Some(&gid), &account.0)?;
                    let text = content::render_typing(&template, &name);
                    self.emit(Event::Typing { conversation: ConversationId(gid), username, text });
                }
                Some(ephemeral::Kind::Presence(p)) => {
                    let state = ephemeral::State::try_from(p.state).unwrap_or(ephemeral::State::Offline);
                    let changed = self.presence.insert(account, (state, std::time::Instant::now())).map(|(s, _)| s) != Some(state);
                    if changed {
                        self.emit(Event::Presence { username, presence: Presence::from(state) });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// What this device last heard of someone's presence. Silence for 25
    /// minutes means offline.
    pub fn presence_of(&self, account: &AccountId) -> Presence {
        match self.presence.get(account) {
            Some((state, at)) if at.elapsed() < PRESENCE_TIMEOUT => Presence::from(*state),
            _ => Presence::Offline,
        }
    }

    // ------------------------------------------------------------ profile

    /// Replace this account's profile with `row` (its version is assigned
    /// here) and send it to every conversation.
    pub async fn set_profile(&mut self, mut row: store::ProfileRow) -> Result<()> {
        let invalid = |e: content::ProfileError| Error::InvalidInput(e.to_string());
        content::check_display_name(&row.display_name).map_err(invalid)?;
        content::check_typing_template(&row.typing_template).map_err(invalid)?;
        content::check_note(&row.note_text).map_err(invalid)?;
        content::check_arrival(&row.arrival).map_err(invalid)?;
        let mine = self.me()?.account;
        row.version = store::profile(&self.conn, &mine.0)?.map_or(0, |p| p.version) + 1;
        store::upsert_profile(&self.conn, &mine.0, &row)?;
        // Every conversation, and the self group so this account's other
        // devices learn it too.
        let body = profile_body(&row);
        for gid in store::broadcast_groups(&self.conn)? {
            self.queue_body(gid, Body::Profile(body.clone()))?;
        }
        self.emit(Event::ProfileChanged { username: self.me()?.username.clone() });
        match self.flush_outbox().await {
            Ok(()) | Err(Error::NotConnected | Error::Network(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Set or clear a private nickname for someone. Shared only with this
    /// account's own devices, through the self group.
    pub async fn set_nickname(&mut self, username: &str, nickname: &str) -> Result<()> {
        content::check_display_name(nickname).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let account = self.resolve(username).await?;
        let entry = content::NicknameEntry { account: account.0.to_vec(), name: nickname.into() };
        self.sync_to_own_devices(content::SelfSync { nicknames: vec![entry], ..Default::default() }).await?;
        self.emit(Event::ProfileChanged { username: username.into() });
        Ok(())
    }

    /// Apply a change here and send it to this account's other devices. With
    /// no self group yet, it stays on this device.
    async fn sync_to_own_devices(&mut self, change: content::SelfSync) -> Result<()> {
        match store::self_group(&self.conn)? {
            Some(gid) => {
                self.queue_body(gid, Body::SelfSync(change))?;
                match self.flush_outbox().await {
                    Ok(()) | Err(Error::NotConnected | Error::Network(_)) => Ok(()),
                    Err(e) => Err(e),
                }
            }
            None => {
                check_self_sync(&change).map_err(Error::InvalidInput)?;
                let tx = self.conn.transaction()?;
                apply_self_sync(&tx, &change, SyncSource::Local(None))?;
                tx.commit()?;
                Ok(())
            }
        }
    }
}

/// How a gossiped chain head compares with the chain this device holds.
#[derive(Debug, PartialEq, Eq)]
enum Gossip {
    /// The same link sits at that position.
    Matches,
    /// The sender knows links this device does not have yet.
    Ahead,
    /// A different link sits at that position: someone forked the chain.
    Conflicts,
}

fn compare_gossip(chain: &Chain, head: &content::ChainHead) -> Gossip {
    match chain.hash_at(head.seq) {
        Some(h) if h.0[..] == head.head[..] => Gossip::Matches,
        Some(_) => Gossip::Conflicts,
        None => Gossip::Ahead,
    }
}

/// A group's metadata and its member accounts, each with its number of
/// devices, as this device's MLS state has them.
pub(crate) fn group_members(conn: &Connection, gid: &[u8]) -> Result<(GroupMeta, Vec<(AccountId, u32)>)> {
    let provider = Provider::on(conn);
    let group = load_group(&provider, gid)?;
    let meta = GroupMeta::from_extensions(group.extensions()).map_err(mls)?;
    let mut counts: Vec<(AccountId, u32)> = Vec::new();
    for m in group.members() {
        let Ok(leaf) = leaf_of_member(&m) else { continue };
        match counts.iter_mut().find(|(a, _)| *a == leaf.account) {
            Some((_, n)) => *n += 1,
            None => counts.push((leaf.account, 1)),
        }
    }
    Ok((meta, counts))
}

/// Group names follow the display-name rules and may not be empty.
fn check_group_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::InvalidInput("give the group a name".into()));
    }
    content::check_display_name(name).map_err(|e| Error::InvalidInput(e.to_string().replace("display name", "group name")))
}

/// Device names follow the display-name rules and may not be empty.
fn check_device_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::InvalidInput("give the device a name".into()));
    }
    content::check_display_name(name).map_err(|e| Error::InvalidInput(e.to_string().replace("display name", "device name")))
}

/// Whether a body belongs in a group: messages go in conversations, never in
/// the self group; profiles go everywhere.
fn body_allowed(body: &Body, self_group: bool) -> bool {
    match body {
        Body::Profile(_) => true,
        Body::Text(_) | Body::Edit(_) | Body::Delete(_) | Body::ConversationName(_) | Body::Call(_) => !self_group,
        Body::SelfSync(_) => self_group,
    }
}

/// Check every entry of a self-group sync before any is applied.
fn check_self_sync(s: &content::SelfSync) -> std::result::Result<(), String> {
    for n in &s.nicknames {
        if n.account.len() != 32 {
            return Err("a nickname names an invalid account".into());
        }
        content::check_display_name(&n.name).map_err(|e| e.to_string())?;
    }
    for d in &s.device_names {
        if d.device.len() != 16 {
            return Err("a device name names an invalid device".into());
        }
        content::check_display_name(&d.name).map_err(|e| e.to_string())?;
    }
    if s.verified.iter().any(|v| v.account.len() != 32) {
        return Err("a verification names an invalid account".into());
    }
    if s.read.iter().any(|r| r.group.len() != 16) {
        return Err("a read position names an invalid group".into());
    }
    Ok(())
}

/// Where a self-group sync being applied came from.
enum SyncSource<'a> {
    /// This device made it; `Some` is the self-group message carrying it.
    Local(Option<&'a [u8]>),
    /// The self group's log, at `seq`.
    Log { seq: u64, snapshot: bool },
}

/// What applying a self-group sync changed.
#[derive(Debug, Default)]
struct SyncChanges {
    /// Accounts whose nickname or verification changed.
    people: Vec<AccountId>,
    /// Groups whose read position moved.
    read: Vec<Vec<u8>>,
    /// The announcements read position moved.
    announcements: bool,
}

/// Apply a checked self-group sync (see [`store::Synced`] for the ordering
/// rules). Read positions merge by taking the larger.
fn apply_self_sync(tx: &Connection, s: &content::SelfSync, from: SyncSource<'_>) -> Result<SyncChanges> {
    let set = |kind, key: &[u8], value: &str| -> Result<bool> {
        match from {
            SyncSource::Local(msg_id) => store::synced_set_local(tx, kind, key, value, msg_id).map(|()| true),
            SyncSource::Log { seq, snapshot: true } => store::synced_fill(tx, kind, key, value, seq),
            SyncSource::Log { seq, snapshot: false } => store::synced_apply(tx, kind, key, value, seq),
        }
    };
    let mut changes = SyncChanges::default();
    for n in &s.nicknames {
        if set(store::Synced::Nickname, &n.account, &n.name)? {
            changes.people.extend(store::account_id(&n.account).ok());
        }
    }
    for d in &s.device_names {
        set(store::Synced::DeviceName, &d.device, &d.name)?;
    }
    for v in &s.verified {
        if set(store::Synced::Verified, &v.account, if v.verified { store::VERIFIED } else { "" })? {
            changes.people.extend(store::account_id(&v.account).ok());
        }
    }
    for r in &s.read {
        if store::raise_read(tx, &r.group, r.seq)? {
            changes.read.push(r.group.clone());
        }
    }
    changes.announcements = store::raise_announcements_read(tx, s.announcements_read)?;
    Ok(changes)
}

/// The full self-group snapshot for a device joining it.
fn self_snapshot(conn: &Connection) -> Result<content::SelfSync> {
    Ok(content::SelfSync {
        nicknames: store::synced_all(conn, store::Synced::Nickname)?
            .into_iter()
            .map(|(account, name)| content::NicknameEntry { account, name })
            .collect(),
        device_names: store::synced_all(conn, store::Synced::DeviceName)?
            .into_iter()
            .map(|(device, name)| content::DeviceNameEntry { device, name })
            .collect(),
        verified: store::synced_all(conn, store::Synced::Verified)?
            .into_iter()
            .map(|(account, _)| content::VerifiedEntry { account, verified: true })
            .collect(),
        read: store::read_positions(conn)?.into_iter().map(|(group, seq)| content::ReadEntry { group, seq }).collect(),
        announcements_read: store::meta_u64(conn, store::META_ANNOUNCEMENTS_READ)?,
        snapshot: true,
    })
}

/// The content form of a stored profile.
fn profile_body(p: &store::ProfileRow) -> content::Profile {
    content::Profile {
        version: p.version,
        display_name: p.display_name.clone(),
        typing_template: p.typing_template.clone(),
        note: (!p.note_text.is_empty()).then(|| content::Note {
            text: p.note_text.clone(),
            set_at_ms: p.note_set_at,
            expires_at_ms: p.note_expires_at,
        }),
        arrival: p.arrival.clone(),
    }
}

/// A message that exists, is not deleted, and was sent by `me`.
fn own_live_message(conn: &Connection, gid: &[u8], target: &[u8], me: &AccountId) -> Result<()> {
    match store::message_meta(conn, gid, target)? {
        Some(m) if m.sender_account == *me && !m.deleted => Ok(()),
        Some(m) if m.deleted => Err(Error::InvalidInput("that message was deleted".into())),
        Some(_) => Err(Error::NotAllowed("only the sender can edit or delete a message")),
        None => Err(Error::NotFound("message")),
    }
}

/// Record a newly joined or created conversation. If it is a second DM with
/// the same peer (both sides created one at once), the group with the lower
/// ID stays live and the other is retired; both sides choose the same one.
fn install_conversation(tx: &Transaction<'_>, gid: &[u8; 16], meta: &GroupMeta, me: &AccountId) -> Result<()> {
    match meta.conversation_kind() {
        Some(MlsKind::Dm) => {
            let peer = meta.admins.iter().find(|a| a[..] != me.0).ok_or(Error::Corrupt("dm without a peer"))?;
            let peer = store::account_id(peer)?;
            match store::dm_with(tx, &peer)? {
                Some(existing) if existing.group_id == *gid => {}
                Some(existing) if *gid < existing.group_id => {
                    store::set_inactive(tx, &existing.group_id)?;
                    store::insert_conversation(tx, gid, KIND_DM, Some(&peer), "", true)?;
                }
                Some(_) => store::insert_conversation(tx, gid, KIND_DM, Some(&peer), "", false)?,
                None => store::insert_conversation(tx, gid, KIND_DM, Some(&peer), "", true)?,
            }
        }
        Some(MlsKind::Group) => store::insert_conversation(tx, gid, KIND_GROUP, None, &meta.name, true)?,
        Some(MlsKind::SelfGroup) => {
            if meta.admins.len() != 1 || meta.admins[0][..] != me.0 {
                return Err(Error::Corrupt("another account's self group"));
            }
            match store::self_group(tx)? {
                Some(existing) if existing == *gid => {}
                Some(existing) if *gid < existing => {
                    store::set_inactive(tx, &existing)?;
                    store::insert_conversation(tx, gid, KIND_SELF, None, "", true)?;
                }
                Some(_) => store::insert_conversation(tx, gid, KIND_SELF, None, "", false)?,
                None => store::insert_conversation(tx, gid, KIND_SELF, None, "", true)?,
            }
        }
        _ => return Err(Error::Corrupt("group kind")),
    }
    Ok(())
}

fn join_from_welcome(
    tx: &Transaction<'_>,
    me: &Me,
    chains: &HashMap<AccountId, Chain>,
    bytes: &[u8],
) -> std::result::Result<Option<[u8; 16]>, Fail> {
    let welcome = match MlsMessageIn::tls_deserialize_exact(bytes).map_err(|_| reject("malformed invitation"))?.extract() {
        MlsMessageBodyIn::Welcome(w) => w,
        _ => return Err(reject("not an invitation")),
    };
    let provider = Provider::on(tx);
    let builder = StagedWelcome::build_from_welcome(&provider, &post_mls::join_config(), welcome)
        .map_err(|e| reject(format!("cannot open it: {e}")))?;
    // A Welcome for a group this device already has replaces that group only
    // if the device is no longer in it (it was removed, or left): it is being
    // added back, and the conversation and its messages carry on. A group the
    // device is still in is never replaced, or anyone holding one of its
    // KeyPackages could knock it out of a conversation with a forged Welcome
    // reusing the group's ID. The ID is read before staging to decide, and
    // checked against the verified one after.
    let peeked: [u8; 16] =
        builder.processed_welcome().unverified_group_info().group_id().as_slice().try_into().map_err(|_| reject("bad group id"))?;
    let existing = store::conversation(tx, &peeked)?;
    if existing.as_ref().is_some_and(|c| c.following && !c.leaving) {
        return Ok(None);
    }
    let builder = if existing.is_some() { builder.replace_old_group() } else { builder };
    let staged = builder.build().map_err(|e| reject(format!("cannot open it: {e}")))?;
    let gid: [u8; 16] = staged.group_context().group_id().as_slice().try_into().map_err(|_| reject("bad group id"))?;
    if gid != peeked {
        return Err(reject("an invitation's group changed while it was opened"));
    }
    let meta = GroupMeta::from_extensions(staged.group_context().extensions()).map_err(|e| reject(format!("bad group metadata: {e}")))?;
    let leaves = staged.members().map(|m| leaf_of_member(&m)).collect::<std::result::Result<Vec<_>, _>>()?;
    authorize_welcome(chains, &leaves)?;
    let accounts: HashSet<AccountId> = leaves.iter().map(|l| l.account).collect();
    if !leaves.iter().any(|l| l.account == me.account && l.device == me.device) {
        return Err(reject("this device is not in it"));
    }
    let admins: HashSet<AccountId> = meta.admins.iter().filter_map(|a| store::account_id(a).ok()).collect();
    match meta.conversation_kind() {
        Some(MlsKind::Dm) if accounts.len() != 2 || accounts != admins => {
            return Err(reject("a direct message must be between exactly its two people"));
        }
        Some(MlsKind::SelfGroup) if accounts.len() != 1 || admins != HashSet::from([me.account]) => {
            return Err(reject("a device group must hold only this account's devices"));
        }
        _ => {}
    }
    if existing.is_some() {
        if let Ok(mut old) = load_group(&provider, &gid) {
            old.delete(provider.storage()).map_err(fatal)?;
        }
        store::pending_commit_remove(tx, &gid)?;
        store::rejoin(tx, &gid)?;
        staged.into_group(&provider).map_err(fatal)?;
    } else {
        staged.into_group(&provider).map_err(fatal)?;
        install_conversation(tx, &gid, &meta, &me.account)?;
    }
    Ok(Some(gid))
}

fn process_in_tx(
    tx: &Transaction<'_>,
    me: &AccountId,
    chains: &HashMap<AccountId, Chain>,
    gid: &[u8; 16],
    e: &proto::LogEntry,
) -> std::result::Result<(Outcome, Option<(AccountId, content::ChainHead)>), Fail> {
    let provider = Provider::on(tx);
    let mut group = load_group(&provider, gid)?;
    let msg = MlsMessageIn::tls_deserialize_exact(&e.payload)
        .map_err(|_| reject("a malformed message was skipped"))?
        .try_into_protocol_message()
        .map_err(|_| reject("a malformed message was skipped"))?;
    let processed = group.process_message(&provider, msg).map_err(|err| reject(format!("a message could not be decrypted: {err}")))?;
    let (sender, sender_device) =
        post_mls::parse_credential(processed.credential()).map_err(|_| reject("a message had an invalid sender"))?;
    match processed.into_content() {
        ProcessedMessageContent::ApplicationMessage(app) => {
            let bytes = app.into_bytes();
            let outcome = handle_content(tx, me, gid, &sender, &sender_device, e, &bytes)?;
            // The sender's chain head, checked once this entry is committed.
            let gossip = Content::decode(bytes.as_slice()).ok().and_then(|c| c.sender_chain).map(|h| (sender, h));
            Ok((outcome, gossip))
        }
        ProcessedMessageContent::StagedCommitMessage(staged) => {
            check_commit(&group, &staged, &sender, chains)?;
            // Someone else's commit took this epoch, so ours can never land.
            if store::pending_commit(tx, gid)?.is_some() {
                group.clear_pending_commit(provider.storage()).map_err(fatal)?;
                store::pending_commit_remove(tx, gid)?;
            }
            let removed = staged.self_removed();
            let new_meta = GroupMeta::from_extensions(staged.group_context().extensions()).map_err(fatal)?;
            let members: HashSet<AccountId> = group.members().filter_map(|m| leaf_of_member(&m).ok()).map(|l| l.account).collect();
            let adds_accounts = staged
                .add_proposals()
                .filter_map(|a| post_mls::leaf_identity(a.add_proposal().key_package().leaf_node()).ok())
                .any(|l| !members.contains(&l.account));
            group.merge_staged_commit(&provider, *staged).map_err(fatal)?;
            if removed {
                store::stop_following(tx, gid)?;
            } else if new_meta.conversation_kind() == Some(MlsKind::Group) {
                store::set_name(tx, gid, &new_meta.name)?;
            }
            let outcome = if adds_accounts && !removed { Outcome::NewMembers } else { Outcome::Conversation };
            Ok((outcome, None))
        }
        ProcessedMessageContent::ProposalMessage(p) => {
            check_proposal(&group, &sender, &p)?;
            group.store_pending_proposal(provider.storage(), *p).map_err(fatal)?;
            Ok((Outcome::Nothing, None))
        }
        _ => Err(reject("an unsupported message was skipped")),
    }
}

/// The only proposal Post sends: a member leaving, removing its own
/// account's devices. Anything else is refused before it can reach the
/// proposal store and ride along in an honest member's commit.
fn check_proposal(group: &MlsGroup, sender: &AccountId, p: &QueuedProposal) -> std::result::Result<(), Fail> {
    let Proposal::Remove(r) = p.proposal() else { return Err(reject("an unsupported proposal was skipped")) };
    let target = group.members().find(|m| m.index == r.removed()).ok_or_else(|| reject("a proposal named no member"))?;
    if leaf_of_member(&target)?.account != *sender {
        return Err(reject("someone proposed removing another member"));
    }
    Ok(())
}

/// Policy for a commit by `committer` (design section 6, "Group policy").
fn check_commit(
    group: &MlsGroup,
    staged: &StagedCommit,
    committer: &AccountId,
    chains: &HashMap<AccountId, Chain>,
) -> std::result::Result<(), Fail> {
    let old = GroupMeta::from_extensions(group.extensions()).map_err(fatal)?;
    let new = GroupMeta::from_extensions(staged.group_context().extensions()).map_err(|_| reject("a change set invalid group details"))?;
    let is_dm = old.conversation_kind() == Some(MlsKind::Dm);
    let is_self = old.conversation_kind() == Some(MlsKind::SelfGroup);
    let is_admin = old.admins.iter().any(|a| a[..] == committer.0);
    if new != old {
        if is_dm || is_self || !is_admin {
            return Err(reject("someone who is not an admin tried to change the group"));
        }
        if new.kind != old.kind {
            return Err(reject("someone tried to turn the group into a different kind of conversation"));
        }
        // Exactly Post's extensions, so nothing rides along unchecked.
        let expected = post_mls::group_context_extensions(&new).map_err(fatal)?;
        if staged.group_context().extensions() != &expected {
            return Err(reject("a change set group details this app does not recognise"));
        }
    }
    let members: Vec<LeafIdentity> = group.members().filter_map(|m| leaf_of_member(&m).ok()).collect();
    let current: HashSet<AccountId> = members.iter().map(|l| l.account).collect();
    for add in staged.add_proposals() {
        let leaf = post_mls::leaf_identity(add.add_proposal().key_package().leaf_node()).map_err(|_| reject("an added device has an invalid credential"))?;
        authorize(chains, std::slice::from_ref(&leaf))?;
        if !current.contains(&leaf.account) {
            // A DM's two accounts are fixed, and a self group never gains another account.
            let allowed = if is_dm {
                old.admins.iter().any(|a| a[..] == leaf.account.0)
            } else {
                is_admin && !is_self
            };
            if !allowed {
                return Err(reject("someone who is not an admin tried to add a person"));
            }
        }
    }
    if let Some(leaf) = staged.update_path_leaf_node() {
        let id = post_mls::leaf_identity(leaf).map_err(|_| reject("a change came from an invalid device"))?;
        if id.account != *committer {
            return Err(reject("a change was signed by a different account"));
        }
        authorize(chains, std::slice::from_ref(&id))?;
    }
    // Removals are justified by facts that do not change with time: the
    // committer's own account, the member's own leave proposal, the device's
    // removal from its chain, or an admin's decision. Staleness is not one of
    // them: it comes and goes, and a device that came back would refuse its
    // own earlier removal. Stale leaves are removed by their account's other
    // devices or by an admin, which these rules already allow.
    for rem in staged.remove_proposals() {
        let idx = rem.remove_proposal().removed();
        let Some(m) = group.members().find(|m| m.index == idx) else { return Err(reject("a removal named no member")) };
        let leaf = leaf_of_member(&m)?;
        let device_removed = chains.get(&leaf.account).is_some_and(|c| !c.device(&leaf.device).is_some_and(|d| d.is_active()));
        // Who asked: the committer, or the member whose proposal it commits.
        let proposer = match rem.sender() {
            Sender::Member(i) => group.members().find(|m| m.index == *i).and_then(|m| leaf_of_member(&m).ok()).map(|l| l.account),
            _ => None,
        };
        let leaving = proposer == Some(leaf.account);
        if !(leaf.account == *committer || leaving || device_removed || (!is_dm && !is_self && is_admin)) {
            // Perhaps the removal is in a chain link this device has not
            // fetched yet; the caller refreshes the directory and retries once.
            return Err(Fail::NeedDirectory);
        }
    }
    if staged.update_proposals().next().is_some() {
        return Err(reject("an unsupported change was skipped"));
    }
    // A group always keeps an admin among its members, or nobody could
    // manage it again.
    if new.conversation_kind() == Some(MlsKind::Group) {
        let removed: HashSet<LeafNodeIndex> = staged.remove_proposals().map(|r| r.remove_proposal().removed()).collect();
        let after: HashSet<AccountId> = group
            .members()
            .filter(|m| !removed.contains(&m.index))
            .filter_map(|m| leaf_of_member(&m).ok().map(|l| l.account))
            .chain(staged.add_proposals().filter_map(|a| post_mls::leaf_identity(a.add_proposal().key_package().leaf_node()).ok().map(|l| l.account)))
            .collect();
        if !new.admins.iter().any(|a| store::account_id(a).is_ok_and(|a| after.contains(&a))) {
            return Err(reject("a change would have left the group with no admin"));
        }
    }
    Ok(())
}

fn handle_content(
    tx: &Transaction<'_>,
    me: &AccountId,
    gid: &[u8; 16],
    sender: &AccountId,
    sender_device: &DeviceId,
    e: &proto::LogEntry,
    bytes: &[u8],
) -> std::result::Result<Outcome, Fail> {
    let c = Content::decode(bytes).map_err(|_| reject("a malformed message was skipped"))?;
    if c.id.len() != 16 || c.id != e.client_msg_id {
        return Err(reject("a message's id did not match its delivery"));
    }
    let id: [u8; 16] = c.id.as_slice().try_into().map_err(|_| reject("bad message id"))?;
    let conv = store::conversation(tx, gid)?;
    if conv.as_ref().is_some_and(|conv| conv.leaving) {
        // The user left: this device only follows the group until its
        // removal lands, and keeps nothing more from it.
        return Ok(Outcome::Nothing);
    }
    let in_self_group = conv.is_some_and(|conv| conv.kind == KIND_SELF);
    match &c.body {
        Some(b) if !body_allowed(b, in_self_group) => return Err(reject("a message of the wrong kind was skipped")),
        // A newer client's self-group data: nothing to show, nothing to store.
        None if in_self_group => return Ok(Outcome::Nothing),
        _ => {}
    }
    // Sent from another of this account's devices: it is ours, and the server
    // has it, since it came from the log.
    let outgoing = sender == me;
    let new_message = |body| NewMessage {
        group: gid,
        id: &id,
        sender_account: sender,
        sender_device,
        outgoing,
        sent_at: c.sent_at_ms,
        seq: Some(e.seq),
        state: if outgoing { store::STATE_SENT } else { store::STATE_RECEIVED },
        body,
    };
    match &c.body {
        Some(Body::Text(t)) => {
            if t.body.len() > content::MAX_TEXT {
                return Err(reject("a message was too long"));
            }
            let added = store::insert_message(tx, &new_message(Some(&t.body)))?;
            store::touch(tx, gid)?;
            Ok(if added { Outcome::Added(id) } else { Outcome::Nothing })
        }
        Some(Body::Edit(ed)) => {
            let Some(target) = store::message_meta(tx, gid, &ed.target)? else { return Ok(Outcome::Nothing) };
            if target.deleted {
                return Ok(Outcome::Nothing);
            }
            if target.sender_account != *sender {
                return Err(reject("someone tried to edit a message they did not send"));
            }
            if ed.body.is_empty() || ed.body.len() > content::MAX_TEXT {
                return Err(reject("an edit was empty or too long"));
            }
            store::add_version(tx, gid, &ed.target, &id, Some(e.seq), &ed.body, c.sent_at_ms)?;
            Ok(Outcome::Changed(ed.target.as_slice().try_into().map_err(|_| reject("bad message id"))?))
        }
        Some(Body::Delete(d)) => {
            let Some(target) = store::message_meta(tx, gid, &d.target)? else { return Ok(Outcome::Nothing) };
            if target.sender_account != *sender {
                return Err(reject("someone tried to delete a message they did not send"));
            }
            store::delete_message(tx, gid, &d.target)?;
            Ok(Outcome::Deleted(d.target.as_slice().try_into().map_err(|_| reject("bad message id"))?))
        }
        Some(Body::Profile(p)) => {
            let note = p.note.clone().unwrap_or_default();
            let valid = content::check_display_name(&p.display_name).is_ok()
                && content::check_typing_template(&p.typing_template).is_ok()
                && content::check_note(&note.text).is_ok();
            if !valid {
                return Err(reject("someone sent an invalid profile"));
            }
            let row = store::ProfileRow {
                version: p.version,
                display_name: p.display_name.clone(),
                typing_template: p.typing_template.clone(),
                note_text: note.text,
                note_set_at: note.set_at_ms,
                note_expires_at: note.expires_at_ms,
                // Only a look: one that is not a valid name is dropped
                // rather than refusing the whole profile.
                arrival: if content::check_arrival(&p.arrival).is_ok() { p.arrival.clone() } else { String::new() },
            };
            Ok(if store::upsert_profile(tx, &sender.0, &row)? { Outcome::Profile(*sender) } else { Outcome::Nothing })
        }
        Some(Body::ConversationName(n)) => {
            if content::check_display_name(&n.name).is_err() {
                return Err(reject("someone set an invalid name"));
            }
            store::set_conversation_name(tx, gid, &sender.0, &n.name)?;
            Ok(Outcome::Conversation)
        }
        Some(Body::Call(ev)) => {
            ev.check().map_err(reject)?;
            Ok(Outcome::Call(calls::Seen { sender: *sender, device: *sender_device, sent_at: c.sent_at_ms, event: ev.clone() }))
        }
        Some(Body::SelfSync(s)) => {
            // The self group holds only our own devices; check anyway.
            if sender != me {
                return Err(reject("another account wrote to this account's device group"));
            }
            check_self_sync(s).map_err(reject)?;
            Ok(Outcome::SelfSync(apply_self_sync(tx, s, SyncSource::Log { seq: e.seq, snapshot: s.snapshot })?))
        }
        None => {
            let added = store::insert_message(tx, &new_message(None))?;
            Ok(if added { Outcome::Added(id) } else { Outcome::Nothing })
        }
    }
}

#[cfg(test)]
#[path = "adversarial_tests.rs"]
mod adversarial_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use post_identity::chain::seal;

    #[test]
    fn refreshes_fall_due_weekly_spread_by_group() {
        let day = 24 * 3600 * 1000;
        let (a, b) = ([0u8; 16], [0xff; 16]);
        let t0 = 1_000 * day;
        // Never early, never later than a week.
        for g in [&a, &b] {
            assert!(!refresh_due(g, t0, t0 + 5 * day, 0));
            assert!(refresh_due(g, t0, t0 + 7 * day, 0));
        }
        // Groups fall due at different times within the last day.
        let first_due = |g: &[u8; 16]| (0..=7 * 24).find(|h| refresh_due(g, t0, t0 + h * 3600 * 1000, 0)).expect("due within a week");
        assert_ne!(first_due(&a), first_due(&b));
        // An explicit refresh makes every older leaf due at once.
        assert!(refresh_due(&a, t0, t0 + 1, t0 + 1));
        assert!(!refresh_due(&a, t0 + 2, t0 + 3, t0 + 1));
    }

    #[test]
    fn gossiped_heads_are_compared_by_position() {
        let laptop = DeviceKeys::generate().expect("keys");
        let (mut chain, _) = Chain::create(&laptop, "chat.example.org", "alice", false, 1).expect("chain");
        let genesis = chain.head();
        let head = |seq: u64, hash: [u8; 32]| content::ChainHead { seq, head: hash.to_vec() };

        assert_eq!(compare_gossip(&chain, &head(0, genesis.0)), Gossip::Matches);
        // The sender has added a device this copy has not seen.
        assert_eq!(compare_gossip(&chain, &head(1, [9; 32])), Gossip::Ahead);
        // A different genesis at the same position.
        assert_eq!(compare_gossip(&chain, &head(0, [9; 32])), Gossip::Conflicts);
        // Malformed hashes never match.
        assert_eq!(compare_gossip(&chain, &content::ChainHead { seq: 0, head: vec![1, 2] }), Gossip::Conflicts);

        // Once the link arrives, the same gossip matches.
        let phone = DeviceKeys::generate().expect("keys");
        let bytes = chain.next_link(laptop.device_id(), Op::AddDevice { device: phone.entry(false) }, 2).expect("link");
        chain
            .append(&seal(&bytes, &laptop.sign_link(&bytes).expect("sign"), Some(&phone.cosign_link(&bytes).expect("cosign"))))
            .expect("append");
        assert_eq!(compare_gossip(&chain, &head(1, chain.head().0)), Gossip::Matches);
        assert_eq!(compare_gossip(&chain, &head(1, genesis.0)), Gossip::Conflicts);
    }
}
