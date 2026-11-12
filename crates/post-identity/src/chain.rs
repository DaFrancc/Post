//! The device chain: a signed, hash-chained log of which devices belong to an
//! account (design section 5).
//!
//! There is no account-level private key. The first link (genesis) is signed
//! by the account's first device, and the account ID is the hash of that link.
//! Each later link is signed by a device that is active at that point, and
//! points at the hash of the previous link.
//!
//! A [`Chain`] value is always fully verified: it can only be built from a
//! valid genesis link and only grows by links that pass every rule below.
//! A rejected link leaves the chain unchanged.
//!
//! Rules:
//! - Every link carries a hybrid signature (Ed25519 + ML-DSA-65) by its signer
//!   over [`labels::CHAIN_LINK`] and the link bytes.
//! - A link must have the next sequence number and point at the current head.
//! - The signer must be an active (added, not removed) device.
//! - `AddDevice` must also be countersigned by the new device, proving it
//!   holds its keys. Device IDs and MLS keys never repeat in a chain.
//! - `RemoveDevice` may target any active device, including the signer, but
//!   never the last one.
//! - `RotateMlsKey` may only be signed by the device whose key it rotates.
//! - At most [`MAX_ACTIVE_DEVICES`] devices are active at once.
//! - An account is a bot account if its genesis device is flagged `bot`.
//!   Every device added later must match: bot accounts add only bot devices,
//!   other accounts never do. Clients badge bot accounts.
//!
//! The chain is public: the server and every device that downloads the
//! directory can read it. So it holds only what verification needs, keys and
//! the bot flag. Device names live in the account's encrypted self group.

use std::collections::BTreeSet;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::hybrid::{DeviceId, HybridSignature, HybridSigningKey, HybridVerifyingKey};
use crate::wire::{self, WireDeviceEntry, WireLink, WireSignedLink, decode_canonical, wire_link};
use crate::{Error, labels};

/// The only link format version.
pub const LINK_VERSION: u32 = 1;
/// Most devices an account may have active at once.
pub const MAX_ACTIVE_DEVICES: usize = 16;

/// Hash of a link's bytes: `SHA-256(label ‖ link bytes)`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LinkHash(pub [u8; 32]);

impl LinkHash {
    /// Hash encoded link bytes.
    pub fn of(link_bytes: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update([labels::LINK_HASH.len() as u8]);
        h.update(labels::LINK_HASH.as_bytes());
        h.update(link_bytes);
        Self(h.finalize().into())
    }

    fn from_slice(b: &[u8], what: &'static str) -> Result<Self, Error> {
        Ok(Self(b.try_into().map_err(|_| Error::Malformed(what))?))
    }
}

/// An account's permanent ID: the [`LinkHash`] of its genesis link. It commits
/// to the server domain, the username and the first device.
pub type AccountId = LinkHash;

impl fmt::Display for LinkHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for LinkHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LinkHash({})", &hex::encode(self.0)[..16])
    }
}

/// A device as listed in the chain. Public, so it carries no name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    /// Signs chain links and login challenges.
    pub chain_key: HybridVerifyingKey,
    /// The Ed25519 key this device's MLS leaves must use.
    pub mls_sig_pub: [u8; 32],
    /// Whether this is a bot account's device.
    pub bot: bool,
}

impl DeviceEntry {
    /// This device's ID, derived from its chain key.
    pub fn device_id(&self) -> DeviceId {
        self.chain_key.device_id()
    }

    /// Encode on its own, as a new device sends it to the device that will
    /// add it (design section 5, "Linking a device").
    pub fn encode(&self) -> Vec<u8> {
        use prost::Message;
        self.to_wire().encode_to_vec()
    }

    /// Decode an entry made by [`DeviceEntry::encode`]. Rejects non-canonical
    /// encodings and invalid keys.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_wire(&decode_canonical::<WireDeviceEntry>(bytes, "device entry")?)
    }

    fn to_wire(&self) -> WireDeviceEntry {
        WireDeviceEntry {
            chain_pub_ed25519: self.chain_key.ed25519_bytes().to_vec(),
            chain_pub_mldsa65: self.chain_key.mldsa65_bytes().to_vec(),
            mls_sig_pub: self.mls_sig_pub.to_vec(),
            bot: self.bot,
        }
    }

    fn from_wire(w: &WireDeviceEntry) -> Result<Self, Error> {
        Ok(Self {
            chain_key: HybridVerifyingKey::from_parts(&w.chain_pub_ed25519, &w.chain_pub_mldsa65)?,
            mls_sig_pub: parse_mls_key(&w.mls_sig_pub)?,
            bot: w.bot,
        })
    }
}

fn parse_mls_key(b: &[u8]) -> Result<[u8; 32], Error> {
    let k: [u8; 32] = b.try_into().map_err(|_| Error::Malformed("mls signature key"))?;
    match ed25519_dalek::VerifyingKey::from_bytes(&k) {
        Ok(vk) if !vk.is_weak() => Ok(k),
        _ => Err(Error::Malformed("mls signature key")),
    }
}

/// What a link does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Create the account with its first device.
    Genesis {
        /// The server this account lives on, such as `chat.example.org`.
        server_domain: String,
        /// The account's username on that server.
        username: String,
        /// The first device.
        device: DeviceEntry,
    },
    /// Add a device. Must be countersigned by it.
    AddDevice {
        /// The new device.
        device: DeviceEntry,
    },
    /// Remove a device.
    RemoveDevice {
        /// The device to remove.
        device_id: DeviceId,
    },
    /// Replace a device's MLS signature key. Signed by that device only.
    RotateMlsKey {
        /// The device whose key changes. Must be the signer.
        device_id: DeviceId,
        /// The new key.
        mls_sig_pub: [u8; 32],
    },
}

/// A decoded, structurally valid link. Not yet checked against any chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Format version, always [`LINK_VERSION`].
    pub version: u32,
    /// The account. `None` only in the genesis link.
    pub account_id: Option<AccountId>,
    /// Position in the chain, starting at 0.
    pub seq: u64,
    /// Hash of the previous link. `None` only in the genesis link.
    pub prev_hash: Option<LinkHash>,
    /// When the signer made the link, in Unix milliseconds. Informational:
    /// clocks are not trusted, so it is not checked.
    pub timestamp_ms: u64,
    /// The device that signed the link.
    pub signer: DeviceId,
    /// What the link does.
    pub op: Op,
}

impl Link {
    /// Encode to the bytes that are hashed and signed.
    pub fn encode(&self) -> Vec<u8> {
        use prost::Message;
        let op = match &self.op {
            Op::Genesis { server_domain, username, device } => {
                wire_link::Op::Genesis(wire::WireGenesis {
                    server_domain: server_domain.clone(),
                    username: username.clone(),
                    device: Some(device.to_wire()),
                })
            }
            Op::AddDevice { device } => {
                wire_link::Op::AddDevice(wire::WireAddDevice { device: Some(device.to_wire()) })
            }
            Op::RemoveDevice { device_id } => {
                wire_link::Op::RemoveDevice(wire::WireRemoveDevice { device_id: device_id.0.to_vec() })
            }
            Op::RotateMlsKey { device_id, mls_sig_pub } => {
                wire_link::Op::RotateMlsKey(wire::WireRotateMlsKey {
                    device_id: device_id.0.to_vec(),
                    mls_sig_pub: mls_sig_pub.to_vec(),
                })
            }
        };
        WireLink {
            version: self.version,
            account_id: self.account_id.map(|a| a.0.to_vec()).unwrap_or_default(),
            seq: self.seq,
            prev_hash: self.prev_hash.map(|h| h.0.to_vec()).unwrap_or_default(),
            timestamp_ms: self.timestamp_ms,
            signer: self.signer.0.to_vec(),
            op: Some(op),
        }
        .encode_to_vec()
    }

    /// Decode and check structure. Rejects non-canonical encodings.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let w: WireLink = decode_canonical(bytes, "link")?;
        if w.version != LINK_VERSION {
            return Err(Error::UnsupportedVersion(w.version));
        }
        let op = match w.op.as_ref().ok_or(Error::Malformed("link op"))? {
            wire_link::Op::Genesis(g) => {
                validate_domain(&g.server_domain)?;
                validate_username(&g.username)?;
                Op::Genesis {
                    server_domain: g.server_domain.clone(),
                    username: g.username.clone(),
                    device: DeviceEntry::from_wire(g.device.as_ref().ok_or(Error::Malformed("device"))?)?,
                }
            }
            wire_link::Op::AddDevice(a) => Op::AddDevice {
                device: DeviceEntry::from_wire(a.device.as_ref().ok_or(Error::Malformed("device"))?)?,
            },
            wire_link::Op::RemoveDevice(r) => Op::RemoveDevice { device_id: DeviceId::from_slice(&r.device_id)? },
            wire_link::Op::RotateMlsKey(r) => Op::RotateMlsKey {
                device_id: DeviceId::from_slice(&r.device_id)?,
                mls_sig_pub: parse_mls_key(&r.mls_sig_pub)?,
            },
        };
        let is_genesis = matches!(op, Op::Genesis { .. });
        let optional_hash = |b: &[u8], what| -> Result<Option<LinkHash>, Error> {
            match (is_genesis, b.is_empty()) {
                (true, true) => Ok(None),
                (false, false) => LinkHash::from_slice(b, what).map(Some),
                _ => Err(Error::Malformed(what)),
            }
        };
        let link = Link {
            version: w.version,
            account_id: optional_hash(&w.account_id, "account id")?,
            seq: w.seq,
            prev_hash: optional_hash(&w.prev_hash, "prev hash")?,
            timestamp_ms: w.timestamp_ms,
            signer: DeviceId::from_slice(&w.signer)?,
            op,
        };
        if is_genesis && link.seq != 0 {
            return Err(Error::GenesisNotFirst);
        }
        Ok(link)
    }
}

/// A link with its signatures, as stored and sent.
struct SignedLink {
    link_bytes: Vec<u8>,
    link: Link,
    sig: HybridSignature,
    cosig: Option<HybridSignature>,
}

impl SignedLink {
    fn decode(raw: &[u8]) -> Result<Self, Error> {
        let w: WireSignedLink = decode_canonical(raw, "signed link")?;
        let cosig = match (w.cosig_ed25519.is_empty(), w.cosig_mldsa65.is_empty()) {
            (true, true) => None,
            (false, false) => Some(HybridSignature::from_parts(&w.cosig_ed25519, &w.cosig_mldsa65)?),
            _ => return Err(Error::Malformed("countersignature")),
        };
        Ok(Self {
            link: Link::decode(&w.link)?,
            sig: HybridSignature::from_parts(&w.sig_ed25519, &w.sig_mldsa65)?,
            cosig,
            link_bytes: w.link,
        })
    }
}

/// Wrap signed link bytes with their signatures into the stored/sent form.
pub fn seal(link_bytes: &[u8], sig: &HybridSignature, cosig: Option<&HybridSignature>) -> Vec<u8> {
    use prost::Message;
    WireSignedLink {
        link: link_bytes.to_vec(),
        sig_ed25519: sig.ed25519_bytes().to_vec(),
        sig_mldsa65: sig.mldsa65_bytes().to_vec(),
        cosig_ed25519: cosig.map(|c| c.ed25519_bytes().to_vec()).unwrap_or_default(),
        cosig_mldsa65: cosig.map(|c| c.mldsa65_bytes().to_vec()).unwrap_or_default(),
    }
    .encode_to_vec()
}

/// A device's private keys.
#[derive(Debug)]
pub struct DeviceKeys {
    /// Signs chain links and login challenges.
    pub chain: HybridSigningKey,
    /// Signs this device's MLS leaves and KeyPackages.
    pub mls: ed25519_dalek::SigningKey,
}

impl DeviceKeys {
    /// Generate fresh keys from the operating system's randomness.
    pub fn generate() -> Result<Self, Error> {
        let mut seed = zeroize::Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut seed[..]).map_err(|_| Error::Rng)?;
        Ok(Self { chain: HybridSigningKey::generate()?, mls: ed25519_dalek::SigningKey::from_bytes(&seed) })
    }

    /// This device's ID.
    pub fn device_id(&self) -> DeviceId {
        self.chain.verifying_key().device_id()
    }

    /// The chain entry describing this device.
    pub fn entry(&self, bot: bool) -> DeviceEntry {
        DeviceEntry {
            chain_key: self.chain.verifying_key(),
            mls_sig_pub: self.mls.verifying_key().to_bytes(),
            bot,
        }
    }

    /// Sign link bytes as the link's signer.
    pub fn sign_link(&self, link_bytes: &[u8]) -> Result<HybridSignature, Error> {
        self.chain.sign(labels::CHAIN_LINK, link_bytes)
    }

    /// Countersign an `AddDevice` link that adds this device. Callers should
    /// first decode the link and check that it adds this device's entry.
    pub fn cosign_link(&self, link_bytes: &[u8]) -> Result<HybridSignature, Error> {
        self.chain.sign(labels::CHAIN_COSIGN, link_bytes)
    }
}

/// A device's history in the chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRecord {
    /// The device's current entry (its MLS key reflects any rotation).
    pub entry: DeviceEntry,
    /// The device's ID.
    pub id: DeviceId,
    /// Sequence number of the link that added it.
    pub added_seq: u64,
    /// Sequence number of the link that removed it, if any.
    pub removed_seq: Option<u64>,
}

impl DeviceRecord {
    /// Whether the device is currently part of the account.
    pub fn is_active(&self) -> bool {
        self.removed_seq.is_none()
    }
}

/// A fully verified device chain.
#[derive(Clone, Debug)]
pub struct Chain {
    account_id: AccountId,
    server_domain: String,
    username: String,
    /// Signed links exactly as received, in order.
    links: Vec<Vec<u8>>,
    /// Hash of each link's bytes, in order.
    hashes: Vec<LinkHash>,
    devices: Vec<DeviceRecord>,
    /// Every MLS key the chain has ever listed, including those of removed
    /// devices and keys rotated away from. None may be listed again.
    used_mls_keys: BTreeSet<[u8; 32]>,
}

impl Chain {
    /// Verify a genesis link and start a chain from it.
    pub fn from_genesis(raw: &[u8]) -> Result<Self, Error> {
        let sl = SignedLink::decode(raw)?;
        let Op::Genesis { server_domain, username, device } = &sl.link.op else {
            return Err(Error::NotGenesis);
        };
        if sl.link.signer != device.device_id() {
            return Err(Error::SignerNotAllowed);
        }
        if sl.cosig.is_some() {
            return Err(Error::UnexpectedCosignature);
        }
        device.chain_key.verify(labels::CHAIN_LINK, &sl.link_bytes, &sl.sig)?;
        let hash = LinkHash::of(&sl.link_bytes);
        Ok(Self {
            account_id: hash,
            server_domain: server_domain.clone(),
            username: username.clone(),
            links: vec![raw.to_vec()],
            hashes: vec![hash],
            devices: vec![DeviceRecord { id: device.device_id(), entry: device.clone(), added_seq: 0, removed_seq: None }],
            used_mls_keys: BTreeSet::from([device.mls_sig_pub]),
        })
    }

    /// Verify a whole chain from its signed links, genesis first.
    pub fn from_links<'a>(links: impl IntoIterator<Item = &'a [u8]>) -> Result<Self, Error> {
        let mut it = links.into_iter();
        let mut chain = Self::from_genesis(it.next().ok_or(Error::NotGenesis)?)?;
        for raw in it {
            chain.append(raw)?;
        }
        Ok(chain)
    }

    /// Create a new account. Returns the chain and its signed genesis link.
    /// `bot` fixes whether this is a bot account, for good.
    pub fn create(
        keys: &DeviceKeys,
        server_domain: &str,
        username: &str,
        bot: bool,
        timestamp_ms: u64,
    ) -> Result<(Self, Vec<u8>), Error> {
        let device = keys.entry(bot);
        let link = Link {
            version: LINK_VERSION,
            account_id: None,
            seq: 0,
            prev_hash: None,
            timestamp_ms,
            signer: device.device_id(),
            op: Op::Genesis { server_domain: server_domain.into(), username: username.into(), device },
        };
        let bytes = link.encode();
        // Decode our own output so creation enforces exactly the rules verification does.
        Link::decode(&bytes)?;
        let raw = seal(&bytes, &keys.sign_link(&bytes)?, None);
        Ok((Self::from_genesis(&raw)?, raw))
    }

    /// Verify the next link and, only if every rule passes, append it.
    pub fn append(&mut self, raw: &[u8]) -> Result<(), Error> {
        let sl = SignedLink::decode(raw)?;
        let change = self.check(&sl)?;
        self.apply(change, sl.link.seq);
        self.links.push(raw.to_vec());
        self.hashes.push(LinkHash::of(&sl.link_bytes));
        Ok(())
    }

    /// Merge links fetched from the server or learned from gossip, starting at
    /// sequence number `from`. Links we already hold must match ours, or this
    /// returns [`Error::Fork`]. Returns how many new links were appended. On
    /// error, links appended before the failing one are kept: each was valid.
    pub fn extend_from(&mut self, from: u64, raws: &[Vec<u8>]) -> Result<usize, Error> {
        if from > self.next_seq() {
            return Err(Error::Gap { have: self.next_seq(), from });
        }
        let mut added = 0;
        for (i, raw) in raws.iter().enumerate() {
            let seq = from + i as u64;
            if seq < self.next_seq() {
                // Compare link content, not the envelope: ML-DSA signatures are
                // randomized, so one link can carry different valid signatures.
                let sl = SignedLink::decode(raw)?;
                if LinkHash::of(&sl.link_bytes) != self.hashes[seq as usize] {
                    return Err(Error::Fork { seq });
                }
            } else {
                self.append(raw)?;
                added += 1;
            }
        }
        Ok(added)
    }

    /// Build the bytes of the next link, ready for signing. The operation is
    /// checked against the current chain first, apart from signatures.
    pub fn next_link(&self, signer: DeviceId, op: Op, timestamp_ms: u64) -> Result<Vec<u8>, Error> {
        let link = Link {
            version: LINK_VERSION,
            account_id: Some(self.account_id),
            seq: self.next_seq(),
            prev_hash: Some(self.head()),
            timestamp_ms,
            signer,
            op,
        };
        let bytes = link.encode();
        let link = Link::decode(&bytes)?;
        self.check_rules(&link)?;
        Ok(bytes)
    }

    /// Build, sign and append a link that needs no countersignature.
    /// Returns the signed link to upload.
    pub fn sign_and_append(&mut self, signer: &DeviceKeys, op: Op, timestamp_ms: u64) -> Result<Vec<u8>, Error> {
        let bytes = self.next_link(signer.device_id(), op, timestamp_ms)?;
        let raw = seal(&bytes, &signer.sign_link(&bytes)?, None);
        self.append(&raw)?;
        Ok(raw)
    }

    /// Check a decoded link against every rule, without changing the chain.
    fn check<'l>(&self, sl: &'l SignedLink) -> Result<Change<'l>, Error> {
        let change = self.check_rules(&sl.link)?;
        let signer = self.device(&sl.link.signer).ok_or(Error::UnknownSigner)?;
        signer.entry.chain_key.verify(labels::CHAIN_LINK, &sl.link_bytes, &sl.sig)?;
        match (&change, &sl.cosig) {
            (Change::Add(entry), Some(cosig)) => entry
                .chain_key
                .verify(labels::CHAIN_COSIGN, &sl.link_bytes, cosig)
                .map_err(|_| Error::BadCosignature)?,
            (Change::Add(_), None) => return Err(Error::MissingCosignature),
            (_, Some(_)) => return Err(Error::UnexpectedCosignature),
            (_, None) => {}
        }
        Ok(change)
    }

    /// Every rule except signatures.
    fn check_rules<'l>(&self, link: &'l Link) -> Result<Change<'l>, Error> {
        if link.seq != self.next_seq() {
            return Err(Error::WrongSequence { expected: self.next_seq(), got: link.seq });
        }
        if link.account_id != Some(self.account_id) {
            return Err(Error::WrongAccount);
        }
        if link.prev_hash != Some(self.head()) {
            return Err(Error::WrongPrevHash);
        }
        let signer = self.device(&link.signer).ok_or(Error::UnknownSigner)?;
        if !signer.is_active() {
            return Err(Error::SignerRemoved);
        }
        match &link.op {
            Op::Genesis { .. } => Err(Error::GenesisNotFirst),
            Op::AddDevice { device } => {
                if self.device(&device.device_id()).is_some() || self.used_mls_keys.contains(&device.mls_sig_pub) {
                    return Err(Error::AlreadyPresent);
                }
                if self.active_devices().count() >= MAX_ACTIVE_DEVICES {
                    return Err(Error::TooManyDevices);
                }
                if device.bot != self.is_bot() {
                    return Err(Error::BotMismatch);
                }
                Ok(Change::Add(device))
            }
            Op::RemoveDevice { device_id } => {
                let target = self.device(device_id).ok_or(Error::UnknownDevice)?;
                if !target.is_active() {
                    return Err(Error::DeviceRemoved);
                }
                if self.active_devices().count() == 1 {
                    return Err(Error::LastDevice);
                }
                Ok(Change::Remove(*device_id))
            }
            Op::RotateMlsKey { device_id, mls_sig_pub } => {
                if *device_id != link.signer {
                    return Err(Error::SignerNotAllowed);
                }
                if self.used_mls_keys.contains(mls_sig_pub) {
                    return Err(Error::AlreadyPresent);
                }
                Ok(Change::Rotate(*device_id, *mls_sig_pub))
            }
        }
    }

    /// Record a checked change. Only called after [`Self::check`] succeeds.
    fn apply(&mut self, change: Change<'_>, seq: u64) {
        match change {
            Change::Add(entry) => {
                self.used_mls_keys.insert(entry.mls_sig_pub);
                self.devices.push(DeviceRecord { id: entry.device_id(), entry: entry.clone(), added_seq: seq, removed_seq: None });
            }
            Change::Remove(id) => {
                if let Some(d) = self.devices.iter_mut().find(|d| d.id == id) {
                    d.removed_seq = Some(seq);
                }
            }
            Change::Rotate(id, key) => {
                self.used_mls_keys.insert(key);
                if let Some(d) = self.devices.iter_mut().find(|d| d.id == id) {
                    d.entry.mls_sig_pub = key;
                }
            }
        }
    }

    /// The account's permanent ID.
    pub fn account_id(&self) -> AccountId {
        self.account_id
    }

    /// The account's username.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// The account's server.
    pub fn server_domain(&self) -> &str {
        &self.server_domain
    }

    /// Whether this is a bot account: its genesis device is flagged `bot`.
    /// Fixed for the account's lifetime.
    pub fn is_bot(&self) -> bool {
        self.devices[0].entry.bot
    }

    /// Sequence number the next link must carry; also the number of links.
    pub fn next_seq(&self) -> u64 {
        self.links.len() as u64
    }

    /// Hash of the newest link. Gossiped inside messages (design section 5).
    pub fn head(&self) -> LinkHash {
        *self.hashes.last().expect("a chain always has its genesis link")
    }

    /// Hash of the link at `seq`, if the chain has one. Used to check gossiped
    /// chain heads against the copy this device holds.
    pub fn hash_at(&self, seq: u64) -> Option<LinkHash> {
        usize::try_from(seq).ok().and_then(|i| self.hashes.get(i)).copied()
    }

    /// The signed links, genesis first, exactly as received.
    pub fn links(&self) -> &[Vec<u8>] {
        &self.links
    }

    /// Every device ever added, in order, including removed ones.
    pub fn devices(&self) -> &[DeviceRecord] {
        &self.devices
    }

    /// Devices currently part of the account.
    pub fn active_devices(&self) -> impl Iterator<Item = &DeviceRecord> {
        self.devices.iter().filter(|d| d.is_active())
    }

    /// Look up a device by ID, active or removed.
    pub fn device(&self, id: &DeviceId) -> Option<&DeviceRecord> {
        self.devices.iter().find(|d| d.id == *id)
    }

    /// Whether `id` is an active device whose MLS key is `mls_sig_pub`. This is
    /// the membership check every MLS leaf must pass (design section 6).
    pub fn authorizes_leaf(&self, id: &DeviceId, mls_sig_pub: &[u8]) -> bool {
        self.device(id).is_some_and(|d| d.is_active() && d.entry.mls_sig_pub[..] == *mls_sig_pub)
    }
}

/// The effect of a checked link. Borrows from the link it came from, so
/// checking copies nothing; [`Chain::apply`] copies only what it keeps.
enum Change<'l> {
    Add(&'l DeviceEntry),
    Remove(DeviceId),
    Rotate(DeviceId, [u8; 32]),
}

/// Usernames: 3 to 32 characters, lowercase ASCII letters, digits and `_`,
/// starting with a letter.
pub fn validate_username(u: &str) -> Result<(), Error> {
    let ok = (3..=32).contains(&u.len())
        && u.starts_with(|c: char| c.is_ascii_lowercase())
        && u.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok { Ok(()) } else { Err(Error::InvalidUsername) }
}

/// Server domains: a lowercase DNS name, optionally followed by `:port`.
pub fn validate_domain(d: &str) -> Result<(), Error> {
    let (host, port) = match d.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (d, None),
    };
    let label_ok = |l: &str| {
        (1..=63).contains(&l.len())
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    let host_ok = (1..=253).contains(&host.len()) && host.split('.').all(label_ok);
    let port_ok = port.is_none_or(|p| p.parse::<u16>().is_ok_and(|n| n > 0) && !p.starts_with('0'));
    if host_ok && port_ok { Ok(()) } else { Err(Error::InvalidDomain) }
}
