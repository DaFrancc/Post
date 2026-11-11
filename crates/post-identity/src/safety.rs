//! Safety numbers (design section 5).
//!
//! Derived from two account IDs only, so they do not change when either
//! person adds or removes devices. Shown as 60 digits in 12 groups of 5.

use std::fmt;

use sha2::{Digest, Sha512};

use crate::{AccountId, labels};

/// A safety number for a pair of accounts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SafetyNumber([u32; 12]);

/// Compute the safety number for two accounts. Symmetric: both people see the
/// same number.
///
/// `SHA-512(label ‖ lower ID ‖ higher ID)`, then each 5-byte chunk of the first
/// 60 bytes, read big-endian, modulo 100 000 gives one 5-digit group. The bias
/// from the modulo is below 2^-23 per group.
pub fn safety_number(a: &AccountId, b: &AccountId) -> SafetyNumber {
    let (lo, hi) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let mut h = Sha512::new();
    h.update([labels::SAFETY.len() as u8]);
    h.update(labels::SAFETY.as_bytes());
    h.update(lo.0);
    h.update(hi.0);
    let d = h.finalize();
    let mut groups = [0u32; 12];
    for (i, g) in groups.iter_mut().enumerate() {
        let n = d[i * 5..i * 5 + 5].iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
        *g = (n % 100_000) as u32;
    }
    SafetyNumber(groups)
}

impl SafetyNumber {
    /// The 60 digits with no spaces.
    pub fn digits(&self) -> String {
        self.0.iter().map(|g| format!("{g:05}")).collect()
    }

    /// The 12 groups of 5 digits.
    pub fn groups(&self) -> [u32; 12] {
        self.0
    }
}

impl fmt::Display for SafetyNumber {
    /// Groups of 5 digits separated by spaces.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.0.iter().map(|g| format!("{g:05}")).collect();
        f.write_str(&parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LinkHash;

    fn id(b: u8) -> AccountId {
        LinkHash([b; 32])
    }

    #[test]
    fn symmetric() {
        assert_eq!(safety_number(&id(1), &id(2)), safety_number(&id(2), &id(1)));
    }

    #[test]
    fn distinct_pairs_differ() {
        assert_ne!(safety_number(&id(1), &id(2)), safety_number(&id(1), &id(3)));
    }

    #[test]
    fn format() {
        let n = safety_number(&id(1), &id(2));
        assert_eq!(n.digits().len(), 60);
        assert!(n.digits().chars().all(|c| c.is_ascii_digit()));
        assert_eq!(n.to_string().split(' ').count(), 12);
    }

    #[test]
    fn known_answer() {
        // Pins the derivation: changing it would silently change every
        // safety number users have already compared.
        let n = safety_number(&id(0xaa), &id(0x55));
        assert_eq!(n.digits(), KNOWN);
    }

    // Computed independently in Python with hashlib.sha512 (label
    // "post/safety/v1", since the rename from Postern).
    const KNOWN: &str = "858317212914039783517033624204980070347113454622751233093423";
}
