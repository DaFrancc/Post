//! Device linking: the code, the sealed channel, and what goes in each slot
//! (design section 5, "Linking a device").
//!
//! The new device makes a random 125-bit code and shows it (typed, or as a QR
//! code in the app). Both devices derive two things from it:
//!
//! - a rendezvous ID, which names a set of write-once slots on the server;
//! - a channel key, which seals every slot with XChaCha20-Poly1305.
//!
//! The server sees only the rendezvous ID and ciphertext. It cannot read or
//! forge a slot, and recovering the code from the rendezvous ID means
//! searching 2^125 codes. Each slot is bound to its rendezvous and number,
//! so sealed payloads cannot be moved between slots.
//!
//! The exchange:
//!
//! | Slot | Written by | Holds |
//! |---|---|---|
//! | 0 `OFFER` | new device | its `DeviceEntry` and chosen name |
//! | 1 `ANSWER` | existing device | the unsigned `AddDevice` link, the username |
//! | 2 `COSIGN` | new device | its countersignature over that link |
//! | 3 `DONE` | existing device | the server accepted the link |
//! | 4 `READY` | new device | it has logged in and uploaded KeyPackages |
//! | 5 `ADDED` | existing device | it added the new device to its conversations |
//!
//! No private key crosses the channel. The new device countersigns only a
//! link that adds exactly its own entry.

#![allow(missing_docs)]

use std::fmt;
use std::time::Duration;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use postern_proto::{self as proto, ErrorCode, Reply, Request};
use prost::Message;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::net::{ServerConn, expect_reply};
use crate::{Error, Result};

pub const SLOT_OFFER: u32 = 0;
pub const SLOT_ANSWER: u32 = 1;
pub const SLOT_COSIGN: u32 = 2;
pub const SLOT_DONE: u32 = 3;
pub const SLOT_READY: u32 = 4;
pub const SLOT_ADDED: u32 = 5;

/// Crockford's base32: no I, L, O or U, so a code survives being read aloud
/// or copied by hand.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Characters in a code: 25 × 5 = 125 bits.
const CODE_CHARS: usize = 25;

/// A link code: 125 random bits.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkCode(Zeroizing<[u8; 16]>);

impl LinkCode {
    pub fn generate() -> Result<Self> {
        let mut b = Zeroizing::new([0u8; 16]);
        getrandom::fill(&mut b[..]).map_err(|_| Error::Rng)?;
        b[0] &= 0x1f;
        Ok(Self(b))
    }

    /// Read a code as typed: case, spaces and dashes do not matter, and the
    /// look-alikes O, I and L read as 0, 1 and 1.
    pub fn parse(s: &str) -> Result<Self> {
        let bad = || Error::InvalidInput("that is not a link code: it has 25 letters and digits".into());
        let mut v: u128 = 0;
        let mut n = 0;
        for c in s.chars().filter(|c| !c.is_whitespace() && *c != '-') {
            let c = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            let d = ALPHABET.iter().position(|&a| char::from(a) == c).ok_or_else(bad)?;
            v = (v << 5) | d as u128;
            n += 1;
            if n > CODE_CHARS {
                return Err(bad());
            }
        }
        if n != CODE_CHARS {
            return Err(bad());
        }
        Ok(Self(Zeroizing::new(v.to_be_bytes())))
    }
}

impl fmt::Display for LinkCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = u128::from_be_bytes(*self.0);
        for i in 0..CODE_CHARS {
            if i > 0 && i % 5 == 0 {
                f.write_str("-")?;
            }
            let d = (v >> (5 * (CODE_CHARS - 1 - i))) & 31;
            write!(f, "{}", char::from(ALPHABET[d as usize]))?;
        }
        Ok(())
    }
}

impl fmt::Debug for LinkCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinkCode(..)")
    }
}

/// The sealed channel a code defines.
pub struct Channel {
    pub rendezvous: [u8; 16],
    cipher: XChaCha20Poly1305,
}

impl Channel {
    pub fn new(code: &LinkCode) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(b"postern/link/v1"), &code.0[..]);
        let mut rendezvous = [0u8; 16];
        let mut key = Zeroizing::new([0u8; 32]);
        // Lengths are far below HKDF's limit, so expansion cannot fail.
        let _ = hk.expand(b"rendezvous", &mut rendezvous);
        let _ = hk.expand(b"channel key", &mut key[..]);
        Self { rendezvous, cipher: XChaCha20Poly1305::new((&*key).into()) }
    }

    fn aad(&self, slot: u32) -> [u8; 20] {
        let mut aad = [0u8; 20];
        aad[..16].copy_from_slice(&self.rendezvous);
        aad[16..].copy_from_slice(&slot.to_be_bytes());
        aad
    }

    /// `nonce ‖ ciphertext`, with a random 24-byte nonce.
    pub fn seal(&self, slot: u32, plain: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).map_err(|_| Error::Rng)?;
        let ct = self
            .cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad: &self.aad(slot) })
            .map_err(|_| Error::Protocol("sealing failed".into()))?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn open(&self, slot: u32, sealed: &[u8]) -> Result<Vec<u8>> {
        let bad = || Error::Protocol("a link message failed to open; the code may be wrong".into());
        if sealed.len() < 24 {
            return Err(bad());
        }
        let (nonce, ct) = sealed.split_at(24);
        self.cipher.decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: &self.aad(slot) }).map_err(|_| bad())
    }

    /// Seal `msg` into `slot`.
    pub async fn put(&self, conn: &ServerConn, slot: u32, msg: &impl Message) -> Result<()> {
        let req = proto::LinkPut { rendezvous: self.rendezvous.to_vec(), slot, payload: self.seal(slot, &msg.encode_to_vec())? };
        match conn.request(Request::LinkPut(req)).await {
            Ok(_) => Ok(()),
            Err(Error::Server(e)) if e.code() == ErrorCode::NotFound => Err(expired()),
            Err(e) => Err(e),
        }
    }

    /// Wait up to `timeout` for `slot`, then open it. `None` if it was not
    /// written in time.
    pub async fn get<M: Message + Default>(&self, conn: &ServerConn, slot: u32, timeout: Duration) -> Result<Option<M>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let wait_ms = u32::try_from(left.as_millis()).unwrap_or(u32::MAX).min(proto::MAX_LINK_WAIT_MS);
            let req = proto::LinkGet { rendezvous: self.rendezvous.to_vec(), slot, wait_ms };
            let got = match conn.request(Request::LinkGet(req)).await {
                Ok(r) => expect_reply!(r, Reply::LinkSlot)?,
                Err(Error::Server(e)) if e.code() == ErrorCode::NotFound => return Err(expired()),
                Err(e) => return Err(e),
            };
            if got.present {
                let plain = self.open(slot, &got.payload)?;
                return M::decode(plain.as_slice()).map(Some).map_err(|_| Error::Protocol("malformed link message".into()));
            }
            if left.is_zero() || wait_ms == 0 {
                return Ok(None);
            }
        }
    }
}

fn expired() -> Error {
    Error::InvalidInput("no device is waiting with that code; it may have expired (codes last 10 minutes)".into())
}

// ---------------------------------------------------------------- slot payloads

#[derive(Clone, PartialEq, Message)]
pub struct Offer {
    /// `DeviceEntry::encode()` of the new device.
    #[prost(bytes = "vec", tag = "1")]
    pub entry: Vec<u8>,
    /// The name the user gave the new device. Kept in the self group, never
    /// in the public chain.
    #[prost(string, tag = "2")]
    pub name: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct Answer {
    /// The `AddDevice` link bytes, signed by nobody yet.
    #[prost(bytes = "vec", tag = "1")]
    pub link: Vec<u8>,
    #[prost(string, tag = "2")]
    pub username: String,
    /// How many conversations the new device is about to join, so it can
    /// upload enough KeyPackages.
    #[prost(uint32, tag = "3")]
    pub conversations: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Cosign {
    #[prost(bytes = "vec", tag = "1")]
    pub ed25519: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub mldsa65: Vec<u8>,
}

/// A step finished. Carries a count where one is useful.
#[derive(Clone, PartialEq, Message)]
pub struct Step {
    #[prost(uint32, tag = "1")]
    pub count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_roundtrip_and_tolerate_typing() {
        for _ in 0..200 {
            let c = LinkCode::generate().expect("rng");
            let s = c.to_string();
            assert_eq!(s.len(), 29);
            assert_eq!(LinkCode::parse(&s).expect("parse"), c);
            assert_eq!(LinkCode::parse(&s.to_lowercase().replace('-', " ")).expect("parse lower"), c);
        }
        let c = LinkCode::parse("00000-00000-00000-00000-0000Z").expect("parse");
        assert_eq!(LinkCode::parse("ooooo-ooooo-ooooo-ooooo-oooo z").expect("look-alikes"), c);
        assert!(LinkCode::parse("short").is_err());
        assert!(LinkCode::parse("00000-00000-00000-00000-000000").is_err());
        assert!(LinkCode::parse("00000-00000-00000-00000-0000U").is_err());
    }

    #[test]
    fn channels_seal_per_code_and_slot() {
        let code = LinkCode::generate().expect("rng");
        let a = Channel::new(&code);
        let b = Channel::new(&LinkCode::parse(&code.to_string()).expect("parse"));
        assert_eq!(a.rendezvous, b.rendezvous);
        let sealed = a.seal(SLOT_OFFER, b"entry").expect("seal");
        assert_eq!(b.open(SLOT_OFFER, &sealed).expect("open"), b"entry");
        // Not in another slot, and not under another code.
        assert!(b.open(SLOT_ANSWER, &sealed).is_err());
        let other = Channel::new(&LinkCode::generate().expect("rng"));
        assert_ne!(other.rendezvous, a.rendezvous);
        assert!(other.open(SLOT_OFFER, &sealed).is_err());
        let mut tampered = sealed.clone();
        tampered[30] ^= 1;
        assert!(b.open(SLOT_OFFER, &tampered).is_err());
    }
}
