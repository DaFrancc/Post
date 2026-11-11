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
use postern_identity::chain::validate_username;
use postern_identity::{AccountId, Chain, DeviceEntry, DeviceId, DeviceKeys, HybridSignature, HybridSigningKey, Link, Op, labels};
use postern_mls::{ConversationKind as MlsKind, GroupMeta, LeafIdentity, SUITE};
use postern_proto::{self as proto, ErrorCode, Reply, Request};
use prost::Message as _;
use rusqlite::{Connection, Transaction};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::content::{self, Body, Content};
use crate::link::{self as linking, Channel, LinkCode};
use crate::net::{ServerConn, expect_reply};
use crate::provider::Provider;
use crate::store::{self, KIND_DM, KIND_GROUP, KIND_SELF, NewMessage};
use crate::{ConversationId, Error, Event, MessageId, Result};

/// Exporter label for group access keys (design section 8).
pub const ACCESS_LABEL: &str = "postern/group-access/v1";
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
            credential: postern_mls::credential(&self.account, &self.device),
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
    /// Nicknames for these accounts changed.
    Nicknames(Vec<AccountId>),
    Conversation,
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
    let (account, device) = postern_mls::parse_credential(&m.credential).map_err(|_| reject("a member has an invalid credential"))?;
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
    postern_identity::chain::validate_domain(&domain).map_err(|_| Error::InvalidInput(format!("{domain} is not a valid server domain")))?;
    Ok(domain)
}

pub(crate) struct Engine {
    pub conn: Connection,
    pub me: Option<Me>,
    pub registered: bool,
    chains: HashMap<AccountId, Chain>,
    session: Option<ServerConn>,
    anon: Option<ServerConn>,
    subscribed: Vec<(Vec<u8>, u64)>,
    /// Devices whose last KeyPackage claim came back empty, and when.
    no_key_packages: HashMap<DeviceId, std::time::Instant>,
    link: Option<PendingLink>,
    pub events: broadcast::Sender<Event>,
}

impl Engine {
    pub fn new(conn: Connection, events: broadcast::Sender<Event>) -> Result<Self> {
        let chains = store::load_chains(&conn)?;
        let mut e = Self {
            conn,
            me: None,
            registered: false,
            chains,
            session: None,
            anon: None,
            subscribed: Vec::new(),
            no_key_packages: HashMap::new(),
            link: None,
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

    /// Both connections' wake and close signals, for the listen loop.
    pub fn signals(&self) -> Result<[(std::sync::Arc<tokio::sync::Notify>, crate::net::Closed); 2]> {
        let (s, a) = (self.session()?, self.anon()?);
        Ok([(s.wake.clone(), s.closed_signal()), (a.wake.clone(), a.closed_signal())])
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
        self.registered = false;
        Ok(())
    }

    /// Log in, then bring everything up to date.
    pub async fn connect(&mut self) -> Result<()> {
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
            let mut group = postern_mls::group_builder(&GroupMeta::self_group(&me.account))
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
        let raw = postern_identity::chain::seal(&bytes, &self.me()?.keys.sign_link(&bytes)?, Some(&cosig));
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
            postern_identity::chain::seal(&bytes, &me.keys.sign_link(&bytes)?, None)
        };
        self.append_own_link(raw).await?;
        self.reconcile().await?;
        self.flush_outbox().await
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
            let verified = match self.chains.get(&account) {
                Some(c) => {
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
            tx.commit()?;
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
                let mut b = KeyPackage::builder().leaf_node_capabilities(postern_mls::capabilities());
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
        let crypto = postern_mls::crypto();
        let me = self.me()?.device;
        let mut out = Vec::new();
        for c in claimed.key_packages {
            let v = match postern_mls::validate_key_package(&c.key_package, &crypto) {
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

        let gid = random16()?;
        let request = {
            let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
            let tx = self.conn.transaction()?;
            let provider = Provider::on(&tx);
            let meta = GroupMeta::dm(&me.account, &peer);
            let mut group = postern_mls::group_builder(&meta)
                .map_err(mls)?
                .with_group_id(GroupId::from_slice(&gid))
                .build(&provider, &me.signer, me.credential())
                .map_err(mls)?;
            let (_, welcome, _) = group.add_members(&provider, &me.signer, &kps).map_err(mls)?;
            let staged = group.pending_commit().ok_or(Error::Corrupt("pending commit"))?;
            let access = staged.export_secret(provider.crypto(), ACCESS_LABEL, &[], 32).map_err(mls)?;
            let welcome_to = kps.iter().filter_map(|kp| postern_mls::leaf_identity(kp.leaf_node()).ok()).map(|l| l.device.0.to_vec()).collect();
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
        Ok(ConversationId(store::dm_with(&self.conn, &peer)?.map_or(gid, |c| c.group_id)))
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
    async fn commit_changes(&mut self, gid: [u8; 16], adds: Vec<KeyPackage>, removes: Vec<LeafNodeIndex>) -> Result<bool> {
        let req = {
            let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
            let tx = self.conn.transaction()?;
            let req = {
                let provider = Provider::on(&tx);
                let mut group = load_group(&provider, &gid)?;
                let access_key = access_key_of(&group, &provider)?;
                let welcome_to = adds
                    .iter()
                    .filter_map(|kp| postern_mls::leaf_identity(kp.leaf_node()).ok())
                    .map(|l| l.device.0.to_vec())
                    .collect();
                let bundle = group
                    .commit_builder()
                    .propose_adds(adds)
                    .propose_removals(removes)
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
        let tx = self.conn.transaction()?;
        {
            let provider = Provider::on(&tx);
            load_group(&provider, &gid)?.merge_pending_commit(&provider).map_err(mls)?;
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
                if adds.is_empty() && p.remove.is_empty() {
                    continue;
                }
                lost_race |= !self.commit_changes(p.gid, adds, p.remove).await?;
            }
            // Our commits are merged as they come back through the log.
            self.sync_groups().await?;
            if !lost_race {
                return Ok(());
            }
        }
        Ok(())
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
            let mut present = HashSet::new();
            let mut remove = Vec::new();
            for m in group.members() {
                let Ok(leaf) = leaf_of_member(&m) else { continue };
                if leaf.account == me.account {
                    present.insert(leaf.device);
                }
                let removed = self.chains.get(&leaf.account).is_some_and(|ch| !ch.device(&leaf.device).is_some_and(|d| d.is_active()));
                if removed {
                    remove.push(m.index);
                }
            }
            // A retired duplicate DM only needs its stale devices removed.
            let add: Vec<DeviceId> = if c.active {
                mine.active_devices().map(|d| d.id).filter(|d| !present.contains(d) && !self.backing_off(d)).collect()
            } else {
                Vec::new()
            };
            if !add.is_empty() || !remove.is_empty() {
                out.push(Plan { gid: c.group_id, add, remove });
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
        let crypto = postern_mls::crypto();
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
                let authorized = |v: &postern_mls::ValidKeyPackage| {
                    v.leaf.account == account
                        && v.leaf.device == device
                        && self.chains.get(&account).is_some_and(|ch| ch.authorizes_leaf(&device, &v.leaf.signature_key))
                };
                match postern_mls::validate_key_package(&c.key_package, &crypto) {
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
        if !conv.following {
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
        let ciphertext = {
            let provider = Provider::on(&tx);
            let mut group = load_group(&provider, &gid)?;
            group.create_message(&provider, &me.signer, &bytes).map_err(mls)?.tls_serialize_detached().map_err(mls)?
        };
        store::outbox_push(&tx, &store::OutboxRow { client_msg_id: id.to_vec(), group_id: gid.to_vec(), content: bytes, ciphertext })?;
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
            Body::SelfSync(s) => {
                for n in &s.nicknames {
                    store::synced_set_local(&tx, store::Synced::Nickname, &n.account, &n.name, Some(&id))?;
                }
                for d in &s.device_names {
                    store::synced_set_local(&tx, store::Synced::DeviceName, &d.device, &d.name, Some(&id))?;
                }
                Outcome::Nicknames(s.nicknames.iter().filter_map(|n| store::account_id(&n.account).ok()).collect())
            }
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
                    retries += 1;
                    self.sync_groups().await?;
                    self.reencrypt(&row)?;
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

    fn reencrypt(&mut self, row: &store::OutboxRow) -> Result<()> {
        let me = self.me.as_ref().ok_or(Error::NotRegistered)?;
        let tx = self.conn.transaction()?;
        let ciphertext = {
            let provider = Provider::on(&tx);
            let mut group = load_group(&provider, &row.group_id)?;
            group.create_message(&provider, &me.signer, &row.content).map_err(mls)?.tls_serialize_detached().map_err(mls)?
        };
        store::outbox_replace_ciphertext(&tx, &row.client_msg_id, &ciphertext)?;
        tx.commit()?;
        Ok(())
    }

    // ------------------------------------------------------------ receiving

    /// Fetch and process everything new: mailbox, then groups, then send
    /// what is queued, then keep subscriptions current.
    pub async fn sync(&mut self) -> Result<()> {
        let joined = self.sync_mailbox().await?;
        self.sync_groups().await?;
        match self.reconcile().await {
            // Out of tokens for today: membership catches up tomorrow.
            Err(Error::NoTokens) => tracing::warn!("no tokens left to update group members"),
            r => r?,
        }
        self.flush_outbox().await?;
        self.resubscribe().await?;
        if joined > 0 {
            // Someone used one of our KeyPackages to add us.
            self.top_up_key_packages(None).await?;
            self.top_up_tokens().await?;
        }
        Ok(())
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
                    tracing::debug!(error = %e, "group fetch refused");
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
                let mine = self.me()?.account;
                if let Some(p) = store::profile(&self.conn, &mine.0)? {
                    self.queue_body(gid, Body::Profile(profile_body(&p)))?;
                }
                let snapshot = self_snapshot(&self.conn)?;
                self.queue_body(gid, Body::SelfSync(snapshot))?;
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
                Step::Done(outcome) => return self.emit_outcome(gid, outcome),
                Step::Retry => self.sync_directory().await?,
                Step::Refused(msg) => {
                    // Fail closed but visibly: skip the entry, tell the user.
                    store::set_cursor(&self.conn, &gid, e.seq)?;
                    self.notice(Some(&gid), &msg)?;
                    return Ok(());
                }
            }
        }
        Ok(())
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
            Outcome::Nicknames(accounts) => {
                for a in accounts {
                    if let Some(username) = store::username_of(&self.conn, &a.0)? {
                        self.emit(Event::ProfileChanged { username });
                    }
                }
            }
            Outcome::Conversation => {
                if let Some(c) = store::conversation(&self.conn, &gid)?
                    && c.kind != KIND_SELF
                {
                    self.emit(Event::ConversationChanged(crate::conversation_from_row(&self.conn, &c)?));
                }
            }
        }
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

    // ------------------------------------------------------------ profile

    /// Replace this account's profile with `row` (its version is assigned
    /// here) and send it to every conversation.
    pub async fn set_profile(&mut self, mut row: store::ProfileRow) -> Result<()> {
        let invalid = |e: content::ProfileError| Error::InvalidInput(e.to_string());
        content::check_display_name(&row.display_name).map_err(invalid)?;
        content::check_typing_template(&row.typing_template).map_err(invalid)?;
        content::check_note(&row.note_text).map_err(invalid)?;
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
                for n in &change.nicknames {
                    store::synced_set_local(&tx, store::Synced::Nickname, &n.account, &n.name, None)?;
                }
                for d in &change.device_names {
                    store::synced_set_local(&tx, store::Synced::DeviceName, &d.device, &d.name, None)?;
                }
                tx.commit()?;
                Ok(())
            }
        }
    }
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
        Body::Text(_) | Body::Edit(_) | Body::Delete(_) | Body::ConversationName(_) => !self_group,
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
    Ok(())
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
    let staged = StagedWelcome::new_from_welcome(&provider, &postern_mls::join_config(), welcome, None)
        .map_err(|e| reject(format!("cannot open it: {e}")))?;
    let gid: [u8; 16] = staged.group_context().group_id().as_slice().try_into().map_err(|_| reject("bad group id"))?;
    if store::conversation(tx, &gid)?.is_some() {
        return Ok(None);
    }
    let meta = GroupMeta::from_extensions(staged.group_context().extensions()).map_err(|e| reject(format!("bad group metadata: {e}")))?;
    let leaves = staged.members().map(|m| leaf_of_member(&m)).collect::<std::result::Result<Vec<_>, _>>()?;
    authorize(chains, &leaves)?;
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
    staged.into_group(&provider).map_err(fatal)?;
    install_conversation(tx, &gid, &meta, &me.account)?;
    Ok(Some(gid))
}

fn process_in_tx(
    tx: &Transaction<'_>,
    me: &AccountId,
    chains: &HashMap<AccountId, Chain>,
    gid: &[u8; 16],
    e: &proto::LogEntry,
) -> std::result::Result<Outcome, Fail> {
    let provider = Provider::on(tx);
    let mut group = load_group(&provider, gid)?;
    let msg = MlsMessageIn::tls_deserialize_exact(&e.payload)
        .map_err(|_| reject("a malformed message was skipped"))?
        .try_into_protocol_message()
        .map_err(|_| reject("a malformed message was skipped"))?;
    let processed = group.process_message(&provider, msg).map_err(|err| reject(format!("a message could not be decrypted: {err}")))?;
    let (sender, sender_device) =
        postern_mls::parse_credential(processed.credential()).map_err(|_| reject("a message had an invalid sender"))?;
    match processed.into_content() {
        ProcessedMessageContent::ApplicationMessage(app) => handle_content(tx, me, gid, &sender, &sender_device, e, &app.into_bytes()),
        ProcessedMessageContent::StagedCommitMessage(staged) => {
            check_commit(&group, &staged, &sender, chains)?;
            // Someone else's commit took this epoch, so ours can never land.
            if store::pending_commit(tx, gid)?.is_some() {
                group.clear_pending_commit(provider.storage()).map_err(fatal)?;
                store::pending_commit_remove(tx, gid)?;
            }
            let removed = staged.self_removed();
            let new_meta = GroupMeta::from_extensions(staged.group_context().extensions()).map_err(fatal)?;
            group.merge_staged_commit(&provider, *staged).map_err(fatal)?;
            if removed {
                store::stop_following(tx, gid)?;
            } else if new_meta.conversation_kind() == Some(MlsKind::Group) {
                store::set_name(tx, gid, &new_meta.name)?;
            }
            Ok(Outcome::Conversation)
        }
        _ => Err(reject("an unsupported proposal was skipped")),
    }
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
    if new != old && (is_dm || is_self || !is_admin) {
        return Err(reject("someone who is not an admin tried to change the group"));
    }
    let members: Vec<LeafIdentity> = group.members().filter_map(|m| leaf_of_member(&m).ok()).collect();
    let current: HashSet<AccountId> = members.iter().map(|l| l.account).collect();
    for add in staged.add_proposals() {
        let leaf = postern_mls::leaf_identity(add.add_proposal().key_package().leaf_node()).map_err(|_| reject("an added device has an invalid credential"))?;
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
        let id = postern_mls::leaf_identity(leaf).map_err(|_| reject("a change came from an invalid device"))?;
        if id.account != *committer {
            return Err(reject("a change was signed by a different account"));
        }
        authorize(chains, std::slice::from_ref(&id))?;
    }
    for rem in staged.remove_proposals() {
        let idx = rem.remove_proposal().removed();
        let Some(m) = group.members().find(|m| m.index == idx) else { return Err(reject("a removal named no member")) };
        let leaf = leaf_of_member(&m)?;
        let device_removed = chains.get(&leaf.account).is_some_and(|c| !c.device(&leaf.device).is_some_and(|d| d.is_active()));
        if !(leaf.account == *committer || device_removed || (!is_dm && !is_self && is_admin)) {
            // Perhaps the removal is in a chain link this device has not
            // fetched yet; the caller refreshes the directory and retries once.
            return Err(Fail::NeedDirectory);
        }
    }
    if staged.update_proposals().next().is_some() {
        return Err(reject("an unsupported change was skipped"));
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
    let in_self_group = store::conversation(tx, gid)?.is_some_and(|conv| conv.kind == KIND_SELF);
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
        Some(Body::SelfSync(s)) => {
            // The self group holds only our own devices; check anyway.
            if sender != me {
                return Err(reject("another account wrote to this account's device group"));
            }
            check_self_sync(s).map_err(reject)?;
            let set = |kind, key: &[u8], name: &str| -> Result<bool> {
                if s.snapshot { store::synced_fill(tx, kind, key, name, e.seq) } else { store::synced_apply(tx, kind, key, name, e.seq) }
            };
            let mut changed = Vec::new();
            for n in &s.nicknames {
                if set(store::Synced::Nickname, &n.account, &n.name)? {
                    changed.extend(store::account_id(&n.account).ok());
                }
            }
            for d in &s.device_names {
                set(store::Synced::DeviceName, &d.device, &d.name)?;
            }
            Ok(Outcome::Nicknames(changed))
        }
        None => {
            let added = store::insert_message(tx, &new_message(None))?;
            Ok(if added { Outcome::Added(id) } else { Outcome::Nothing })
        }
    }
}
