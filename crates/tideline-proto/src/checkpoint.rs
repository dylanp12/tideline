use crate::Hash;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// A 7-byte domain tag, two field digests, a head hash, and two big-endian u64s.
pub const CHECKPOINT_CANON_LEN: usize = 119;

const DOMAIN: &[u8; 7] = b"tlrcp1\n";

/// A server's signed assertion that a run's head was `head_hash` at `seq`.
///
/// A hash chain cannot detect a truncated tail. Anyone holding a checkpoint
/// issued after the deleted events can prove the truncation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub run_id: String,
    pub seq: u64,
    pub head_hash: Hash,
    pub ts: u64,
    pub key_id: String,
    /// Standard base64 of the 64-byte Ed25519 signature over `canon()`.
    pub sig: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointError {
    MalformedSignature,
    BadSignature,
}

impl Checkpoint {
    /// The canonical bytes a signature covers. Variable-length fields are
    /// committed by digest, exactly as in event canon, so the encoding is
    /// fixed-width and trivially portable.
    pub fn canon(&self) -> [u8; CHECKPOINT_CANON_LEN] {
        let mut out = [0u8; CHECKPOINT_CANON_LEN];
        out[..7].copy_from_slice(DOMAIN);
        out[7..39].copy_from_slice(&Hash::of(self.run_id.as_bytes()).0);
        out[39..47].copy_from_slice(&self.seq.to_be_bytes());
        out[47..79].copy_from_slice(&self.head_hash.0);
        out[79..87].copy_from_slice(&self.ts.to_be_bytes());
        out[87..119].copy_from_slice(&Hash::of(self.key_id.as_bytes()).0);
        out
    }

    /// Build and sign a checkpoint.
    pub fn sign(
        run_id: &str,
        seq: u64,
        head_hash: Hash,
        ts: u64,
        key_id: &str,
        key: &SigningKey,
    ) -> Checkpoint {
        let mut cp = Checkpoint {
            run_id: run_id.to_string(),
            seq,
            head_hash,
            ts,
            key_id: key_id.to_string(),
            sig: String::new(),
        };
        let sig = key.sign(&cp.canon());
        cp.sig = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        cp
    }

    /// Verify the signature against a published public key.
    pub fn verify(&self, key: &VerifyingKey) -> Result<(), CheckpointError> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&self.sig)
            .map_err(|_| CheckpointError::MalformedSignature)?;
        let bytes: [u8; 64] = raw
            .try_into()
            .map_err(|_| CheckpointError::MalformedSignature)?;
        key.verify(&self.canon(), &Signature::from_bytes(&bytes))
            .map_err(|_| CheckpointError::BadSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Hash;
    use ed25519_dalek::SigningKey;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn cp(k: &SigningKey) -> Checkpoint {
        Checkpoint::sign(
            "loan-4821",
            12,
            Hash::of(b"head"),
            1_757_635_260_000,
            "k1",
            k,
        )
    }

    #[test]
    fn canon_is_exactly_119_bytes() {
        let c = cp(&key());
        assert_eq!(c.canon().len(), CHECKPOINT_CANON_LEN);
        assert_eq!(CHECKPOINT_CANON_LEN, 119);
    }

    #[test]
    fn a_signed_checkpoint_verifies() {
        let k = key();
        assert!(cp(&k).verify(&k.verifying_key()).is_ok());
    }

    #[test]
    fn rejects_the_wrong_key() {
        let c = cp(&key());
        let other = SigningKey::from_bytes(&[9u8; 32]);
        assert_eq!(
            c.verify(&other.verifying_key()).unwrap_err(),
            CheckpointError::BadSignature
        );
    }

    #[test]
    fn rejects_an_altered_head_hash() {
        // Moving the head is exactly the truncation a checkpoint must expose.
        let k = key();
        let mut c = cp(&k);
        c.head_hash = Hash::of(b"rewritten");
        assert_eq!(
            c.verify(&k.verifying_key()).unwrap_err(),
            CheckpointError::BadSignature
        );
    }

    #[test]
    fn rejects_an_altered_seq() {
        let k = key();
        let mut c = cp(&k);
        c.seq = 3;
        assert_eq!(
            c.verify(&k.verifying_key()).unwrap_err(),
            CheckpointError::BadSignature
        );
    }

    #[test]
    fn rejects_malformed_signature_encoding() {
        let k = key();
        let mut c = cp(&k);
        c.sig = "not base64!!".into();
        assert_eq!(
            c.verify(&k.verifying_key()).unwrap_err(),
            CheckpointError::MalformedSignature
        );
    }

    #[test]
    fn round_trips_over_json() {
        let k = key();
        let json = serde_json::to_string(&cp(&k)).unwrap();
        let back: Checkpoint = serde_json::from_str(&json).unwrap();
        assert!(back.verify(&k.verifying_key()).is_ok());
    }
}
