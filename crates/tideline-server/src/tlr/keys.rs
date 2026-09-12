//! The checkpoint signing key.
//!
//! Checkpoints are what close the truncation gap a hash chain leaves open, so
//! the key that signs them is the record's trust root. It is loaded from the
//! environment where one is configured, and generated ephemerally otherwise —
//! with a warning, because an ephemeral key means checkpoints do not survive a
//! restart and truncation protection lapses across deploys.

use base64::Engine as _;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde_json::json;
use sha2::{Digest, Sha256};
use tideline_proto::{Checkpoint, Hash};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

pub struct SigningKeyring {
    key: SigningKey,
    key_id: String,
}

impl SigningKeyring {
    pub fn generate() -> Self {
        Self::from_seed(rand::random::<[u8; 32]>())
    }

    fn from_seed(seed: [u8; 32]) -> Self {
        let key = SigningKey::from_bytes(&seed);
        // Derived, not chosen: a server-picked id could be reused across a
        // rotation, making two different keys indistinguishable in the record.
        let digest = Sha256::digest(key.verifying_key().as_bytes());
        let key_id = Hash(digest.into()).to_hex()[..16].to_string();
        Self { key, key_id }
    }

    pub fn from_seed_b64(s: &str) -> Option<Self> {
        let raw = B64.decode(s).ok()?;
        let seed: [u8; 32] = raw.try_into().ok()?;
        Some(Self::from_seed(seed))
    }

    pub fn to_seed_b64(&self) -> String {
        B64.encode(self.key.to_bytes())
    }

    /// Load `TIDELINE_CHECKPOINT_KEY`, or generate an ephemeral key and say so.
    pub fn from_env() -> Self {
        match std::env::var("TIDELINE_CHECKPOINT_KEY")
            .ok()
            .filter(|s| !s.is_empty())
        {
            Some(seed) => match Self::from_seed_b64(&seed) {
                Some(k) => {
                    tracing::info!(key_id = %k.key_id(), "checkpoint key loaded");
                    k
                }
                None => {
                    tracing::error!(
                        "TIDELINE_CHECKPOINT_KEY is not base64 of a 32-byte seed; \
                         generating an ephemeral key instead"
                    );
                    Self::generate()
                }
            },
            None => {
                let k = Self::generate();
                tracing::warn!(
                    key_id = %k.key_id(),
                    seed = %k.to_seed_b64(),
                    "no TIDELINE_CHECKPOINT_KEY set — generated an ephemeral one. \
                     Checkpoints signed with it cannot be verified after a restart, \
                     so truncation protection lapses across deploys. Set this seed \
                     in the environment to keep it."
                );
                k
            }
        }
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn public(&self) -> VerifyingKey {
        self.key.verifying_key()
    }

    pub fn sign_head(&self, run_id: &str, seq: u64, head_hash: Hash, ts: u64) -> Checkpoint {
        Checkpoint::sign(run_id, seq, head_hash, ts, &self.key_id, &self.key)
    }

    /// The `/v1/.well-known/tideline` discovery document.
    pub fn well_known(&self) -> serde_json::Value {
        json!({
            "protocol_versions": [1],
            "capabilities": [
                "runs", "events", "approvals", "redaction", "checkpoints", "watch"
            ],
            "keys": [{
                "id": self.key_id,
                "alg": "ed25519",
                "public_key": B64.encode(self.public().as_bytes()),
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_key_signs_and_verifies() {
        let k = SigningKeyring::generate();
        let cp = k.sign_head("r1", 3, Hash::of(b"head"), 1_700_000_000_000);
        assert!(cp.verify(&k.public()).is_ok());
        assert_eq!(cp.key_id, k.key_id());
    }

    #[test]
    fn the_key_id_is_derived_from_the_public_key() {
        // A key id a server chooses freely could be reused across rotations,
        // making two different keys indistinguishable in the record.
        let a = SigningKeyring::generate();
        let b = SigningKeyring::generate();
        assert_ne!(a.key_id(), b.key_id());
        assert_eq!(a.key_id().len(), 16);
    }

    #[test]
    fn a_key_round_trips_through_its_seed_encoding() {
        let a = SigningKeyring::generate();
        let b = SigningKeyring::from_seed_b64(&a.to_seed_b64()).expect("decodes");
        assert_eq!(a.key_id(), b.key_id());
    }

    #[test]
    fn a_malformed_seed_is_rejected() {
        assert!(SigningKeyring::from_seed_b64("nonsense").is_none());
    }

    #[test]
    fn the_published_document_carries_the_public_key() {
        let k = SigningKeyring::generate();
        let doc = k.well_known();
        assert_eq!(doc["protocol_versions"][0], 1);
        assert_eq!(doc["keys"][0]["id"], k.key_id());
        assert!(doc["keys"][0]["public_key"].as_str().unwrap().len() > 16);
    }
}
