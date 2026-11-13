//! The attachment format (design section 9): a file encrypted under a fresh
//! random key with ChaCha20-Poly1305 in the STREAM construction, 64 KiB at a
//! time. Each chunk's nonce carries its counter and a last-chunk flag, so
//! chunks cannot be reordered, dropped or cut short without failing.
//!
//! ```text
//! version (1) ‖ nonce prefix (7) ‖ chunk ‖ chunk ‖ … ‖ last chunk
//! ```
//!
//! Every chunk but the last holds exactly [`CHUNK`] bytes of plaintext; the
//! last holds the rest, possibly all of a full chunk, possibly none.

use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit as _};
use zeroize::Zeroizing;

use crate::{Error, Result};

/// Plaintext bytes per chunk.
pub const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;
const VERSION: u8 = 1;
const PREFIX: usize = 7;
const HEADER: usize = 1 + PREFIX;

/// Encrypt `plaintext` under a new random key. Returns the key and the
/// ciphertext.
pub fn encrypt(plaintext: &[u8]) -> Result<(Zeroizing<[u8; 32]>, Vec<u8>)> {
    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::fill(key.as_mut()).map_err(|_| Error::Rng)?;
    let mut prefix = [0u8; PREFIX];
    getrandom::fill(&mut prefix).map_err(|_| Error::Rng)?;
    let aead = ChaCha20Poly1305::new(key.as_ref().into());
    let mut enc = EncryptorBE32::from_aead(aead, prefix.as_ref().into());
    let mut out = Vec::with_capacity(HEADER + plaintext.len() + (plaintext.len() / CHUNK + 1) * TAG);
    out.push(VERSION);
    out.extend_from_slice(&prefix);
    let mut chunks = plaintext.chunks(CHUNK).peekable();
    let fail = |_| Error::Picture("encrypting the file failed".into());
    loop {
        match chunks.next() {
            Some(c) if chunks.peek().is_some() => out.extend(enc.encrypt_next(c).map_err(fail)?),
            last => {
                out.extend(enc.encrypt_last(last.unwrap_or(&[])).map_err(fail)?);
                break;
            }
        }
    }
    Ok((key, out))
}

/// Decrypt what [`encrypt`] made. Fails on a wrong key, any changed,
/// reordered or missing chunk, or a missing last chunk.
pub fn decrypt(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let bad = || Error::Picture("the file is damaged or the key is wrong".into());
    if ciphertext.len() < HEADER + TAG || ciphertext[0] != VERSION {
        return Err(bad());
    }
    let (prefix, body) = ciphertext[1..].split_at(PREFIX);
    let aead = ChaCha20Poly1305::new(key.into());
    let mut dec = DecryptorBE32::from_aead(aead, prefix.into());
    let mut out = Vec::with_capacity(body.len());
    let mut chunks = body.chunks(CHUNK + TAG).peekable();
    while let Some(c) = chunks.next() {
        if chunks.peek().is_some() {
            out.extend(dec.decrypt_next(c).map_err(|_| bad())?);
        } else {
            out.extend(dec.decrypt_last(c).map_err(|_| bad())?);
            return Ok(out);
        }
    }
    Err(bad())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_at_every_boundary() {
        for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 3 * CHUNK + 5] {
            let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let (key, ct) = encrypt(&plain).expect("encrypt");
            assert_eq!(decrypt(&key, &ct).expect("decrypt"), plain, "{len} bytes");
        }
    }

    #[test]
    fn tampering_fails() {
        let plain = vec![7u8; 2 * CHUNK + 100];
        let (key, ct) = encrypt(&plain).expect("encrypt");
        // A changed byte.
        let mut flipped = ct.clone();
        flipped[HEADER + 10] ^= 1;
        assert!(decrypt(&key, &flipped).is_err());
        // Cut short at a chunk boundary: the new last chunk is not marked last.
        assert!(decrypt(&key, &ct[..HEADER + 2 * (CHUNK + TAG)]).is_err());
        // Chunks swapped.
        let mut swapped = ct[..HEADER].to_vec();
        swapped.extend_from_slice(&ct[HEADER + CHUNK + TAG..HEADER + 2 * (CHUNK + TAG)]);
        swapped.extend_from_slice(&ct[HEADER..HEADER + CHUNK + TAG]);
        swapped.extend_from_slice(&ct[HEADER + 2 * (CHUNK + TAG)..]);
        assert!(decrypt(&key, &swapped).is_err());
        // Another key.
        assert!(decrypt(&[0; 32], &ct).is_err());
        // Nothing at all.
        assert!(decrypt(&key, &[]).is_err());
    }
}
