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

    /// Load the signing key: from the environment, else from a key file, else
    /// generate one and persist it.
    ///
    /// A key that changes on restart is nearly as bad as none: every checkpoint
    /// signed before the restart becomes unverifiable, so truncation protection
    /// lapses on every deploy. Persisting a generated key next to the records
    /// keeps it verifiable. That does mean whoever can read the records can also
    /// sign them, which is precisely why a deployment whose threat model includes
    /// its own operator anchors checkpoints externally rather than relying on
    /// this key alone.
    pub fn from_env() -> Self {
        if let Some(seed) = std::env::var("TIDELINE_CHECKPOINT_KEY")
            .ok()
            .filter(|s| !s.is_empty())
        {
            return match Self::from_seed_b64(&seed) {
                Some(k) => {
                    tracing::info!(key_id = %k.key_id(), "checkpoint key loaded from the environment");
                    k
                }
                None => {
                    tracing::error!(
                        "TIDELINE_CHECKPOINT_KEY is not base64 of a 32-byte seed — falling \
                         back to a key file"
                    );
                    Self::from_file_or_generate()
                }
            };
        }
        Self::from_file_or_generate()
    }

    /// The key file: `TIDELINE_CHECKPOINT_KEY_FILE`, else beside the record
    /// database, so it persists with the volume that holds what it signs.
    fn key_path() -> Option<std::path::PathBuf> {
        if let Some(p) = std::env::var("TIDELINE_CHECKPOINT_KEY_FILE")
            .ok()
            .filter(|s| !s.is_empty())
        {
            return Some(std::path::PathBuf::from(p));
        }
        let db = std::env::var("TIDELINE_RECORD_DB")
            .ok()
            .filter(|s| !s.is_empty())?;
        Some(std::path::Path::new(&db).with_extension("signing-key"))
    }

    fn from_file_or_generate() -> Self {
        let Some(path) = Self::key_path() else {
            let k = Self::generate();
            tracing::warn!(
                key_id = %k.key_id(),
                seed = %k.to_seed_b64(),
                "no checkpoint key and nowhere to keep one (records are in memory) — \
                 generated an ephemeral key. Checkpoints signed with it cannot be \
                 verified after a restart."
            );
            return k;
        };

        if let Ok(seed) = std::fs::read_to_string(&path) {
            if let Some(k) = Self::from_seed_b64(seed.trim()) {
                tracing::info!(key_id = %k.key_id(), path = %path.display(), "checkpoint key loaded");
                return k;
            }
            tracing::error!(
                path = %path.display(),
                "the checkpoint key file is not base64 of a 32-byte seed — refusing to \
                 overwrite it. Fix or remove it; generating a new key here would \
                 invalidate every checkpoint already signed."
            );
            std::process::exit(1);
        }

        let k = Self::generate();
        match write_private(&path, &k.to_seed_b64()) {
            Ok(()) => tracing::info!(
                key_id = %k.key_id(),
                path = %path.display(),
                "generated a checkpoint key and saved it (mode 0600)"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                path = %path.display(),
                key_id = %k.key_id(),
                "generated a checkpoint key but could not save it, so it will change on \
                 restart and checkpoints signed now will not verify afterwards"
            ),
        }
        k
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

/// Write a secret, readable only by its owner.
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{contents}")
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
    fn a_key_persists_across_reloads() {
        // A key that changes on restart invalidates every checkpoint signed
        // before it, so truncation protection would lapse on every deploy.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tideline.signing-key");
        std::env::set_var("TIDELINE_CHECKPOINT_KEY_FILE", &path);
        std::env::remove_var("TIDELINE_CHECKPOINT_KEY");

        let first = SigningKeyring::from_env();
        assert!(path.exists(), "the key should have been saved");
        let second = SigningKeyring::from_env();
        assert_eq!(first.key_id(), second.key_id());

        // And a checkpoint signed by the first still verifies under the second.
        let cp = first.sign_head("r1", 3, Hash::of(b"head"), 1);
        assert!(cp.verify(&second.public()).is_ok());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "a signing key must not be world-readable"
            );
        }
        std::env::remove_var("TIDELINE_CHECKPOINT_KEY_FILE");
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
