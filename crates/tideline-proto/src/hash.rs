use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::hash::Hash as StdHash;

/// A SHA-256 digest. Serialises as lowercase hex, which is what every TLR/1
/// wire field and every corpus vector uses.
#[derive(Clone, Copy, PartialEq, Eq, StdHash, Debug, Default)]
pub struct Hash(pub [u8; 32]);

/// The all-zero digest: the `prev_hash` of event 0, and the digest of a field
/// that was never set.
pub const ZERO: Hash = Hash([0u8; 32]);

const HEX: &[u8; 16] = b"0123456789abcdef";

impl Hash {
    /// SHA-256 of the given bytes.
    pub fn of(bytes: &[u8]) -> Hash {
        Hash(Sha256::digest(bytes).into())
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        }
        s
    }

    pub fn from_hex(s: &str) -> Option<Hash> {
        if s.len() != 64 {
            return None;
        }
        let bytes = s.as_bytes();
        let mut out = [0u8; 32];
        for (i, slot) in out.iter_mut().enumerate() {
            let hi = (bytes[2 * i] as char).to_digit(16)?;
            let lo = (bytes[2 * i + 1] as char).to_digit(16)?;
            *slot = ((hi << 4) | lo) as u8;
        }
        Some(Hash(out))
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }
}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Hash, D::Error> {
        let s = String::deserialize(d)?;
        Hash::from_hex(&s).ok_or_else(|| D::Error::custom("expected 64 hex characters"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_known_input() {
        // The SHA-256 of the empty string — the standard test vector.
        assert_eq!(
            Hash::of(b"").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hex_round_trips() {
        let h = Hash::of(b"tideline");
        assert_eq!(Hash::from_hex(&h.to_hex()), Some(h));
    }

    #[test]
    fn rejects_malformed_hex() {
        assert_eq!(Hash::from_hex("abc"), None);
        assert_eq!(Hash::from_hex(&"z".repeat(64)), None);
    }

    #[test]
    fn zero_is_recognisable() {
        assert!(ZERO.is_zero());
        assert!(!Hash::of(b"x").is_zero());
    }

    #[test]
    fn serialises_as_a_hex_string() {
        let h = Hash::of(b"x");
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, format!("\"{}\"", h.to_hex()));
        assert_eq!(serde_json::from_str::<Hash>(&json).unwrap(), h);
    }
}
