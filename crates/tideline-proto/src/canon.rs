use crate::{EventKind, Hash, ZERO};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The fixed width of a canonical event: a 5-byte domain tag, two big-endian
/// u64s, and five 32-byte field digests.
pub const CANON_LEN: usize = 181;

const DOMAIN: &[u8; 5] = b"tlr1\n";

/// The field names that may appear in a `redacted` map. `kind`, `seq`, and `ts`
/// are structural and never redactable.
pub const REDACTABLE: &[&str] = &["role", "name", "content", "metadata"];

/// Exactly the parts of an event the chain commits to.
///
/// Borrowed rather than owned so verification never copies a large `content`.
#[derive(Clone, Copy, Debug)]
pub struct EventCore<'a> {
    pub seq: u64,
    pub ts: u64,
    pub kind: EventKind,
    pub role: Option<&'a str>,
    pub name: Option<&'a str>,
    pub content: Option<&'a str>,
    /// The exact UTF-8 of the `metadata` member as the client sent it, never
    /// re-serialised.
    pub metadata_raw: Option<&'a str>,
    /// Retained digests of fields erased by a redaction.
    pub redacted: Option<&'a BTreeMap<String, Hash>>,
}

impl EventCore<'_> {
    /// Resolve one field to its 32-byte digest:
    /// present -> SHA-256 of the value; erased -> the retained digest;
    /// never set -> zero.
    fn digest(&self, field: &str, value: Option<&str>) -> Hash {
        match value {
            Some(v) => Hash::of(v.as_bytes()),
            None => self
                .redacted
                .and_then(|m| m.get(field))
                .copied()
                .unwrap_or(ZERO),
        }
    }

    /// The canonical 181-byte encoding of this event.
    pub fn canon(&self) -> [u8; CANON_LEN] {
        let mut out = [0u8; CANON_LEN];
        out[..5].copy_from_slice(DOMAIN);
        out[5..13].copy_from_slice(&self.seq.to_be_bytes());
        out[13..21].copy_from_slice(&self.ts.to_be_bytes());
        out[21..53].copy_from_slice(&Hash::of(self.kind.as_str().as_bytes()).0);
        out[53..85].copy_from_slice(&self.digest("role", self.role).0);
        out[85..117].copy_from_slice(&self.digest("name", self.name).0);
        out[117..149].copy_from_slice(&self.digest("content", self.content).0);
        out[149..181].copy_from_slice(&self.digest("metadata", self.metadata_raw).0);
        out
    }

    /// This event's hash, committing to its predecessor.
    pub fn hash(&self, prev: &Hash) -> Hash {
        let mut h = Sha256::new();
        h.update(self.canon());
        h.update(prev.0);
        Hash(h.finalize().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn core() -> EventCore<'static> {
        EventCore {
            seq: 3,
            ts: 1_757_635_204_120,
            kind: EventKind::ToolCall,
            role: Some("tool"),
            name: Some("pull_credit_report"),
            content: Some("score=690"),
            metadata_raw: Some(r#"{"bureau":"experian"}"#),
            redacted: None,
        }
    }

    #[test]
    fn canon_is_exactly_181_bytes() {
        assert_eq!(core().canon().len(), CANON_LEN);
        assert_eq!(CANON_LEN, 181);
    }

    #[test]
    fn canon_layout_is_the_specified_one() {
        let c = core();
        let bytes = c.canon();
        assert_eq!(&bytes[..5], b"tlr1\n");
        assert_eq!(&bytes[5..13], &3u64.to_be_bytes());
        assert_eq!(&bytes[13..21], &1_757_635_204_120u64.to_be_bytes());
        assert_eq!(&bytes[21..53], &Hash::of(b"tool_call").0);
        assert_eq!(&bytes[53..85], &Hash::of(b"tool").0);
        assert_eq!(&bytes[85..117], &Hash::of(b"pull_credit_report").0);
        assert_eq!(&bytes[117..149], &Hash::of(b"score=690").0);
        assert_eq!(&bytes[149..181], &Hash::of(br#"{"bureau":"experian"}"#).0);
    }

    #[test]
    fn an_absent_field_digests_to_zero() {
        let mut c = core();
        c.role = None;
        assert_eq!(&c.canon()[53..85], &[0u8; 32]);
    }

    #[test]
    fn an_empty_field_differs_from_an_absent_one() {
        // Distinguishing "" from absent matters: one is a claim, the other is
        // the absence of a claim.
        let mut empty = core();
        empty.role = Some("");
        let mut absent = core();
        absent.role = None;
        assert_ne!(empty.canon(), absent.canon());
    }

    #[test]
    fn a_redacted_field_keeps_its_original_digest() {
        // The whole point: erasing content must not change the hash.
        let before = core();
        let original = Hash::of(b"score=690");

        let mut redacted_map = BTreeMap::new();
        redacted_map.insert("content".to_string(), original);
        let mut after = core();
        after.content = None;
        after.redacted = Some(&redacted_map);

        assert_eq!(before.canon(), after.canon());
    }

    #[test]
    fn a_redaction_entry_for_a_present_field_is_ignored() {
        // The present value always wins, so a forged `redacted` map cannot
        // rewrite an event that still has its content.
        let mut map = BTreeMap::new();
        map.insert("content".to_string(), Hash::of(b"something else"));
        let mut forged = core();
        forged.redacted = Some(&map);
        assert_eq!(forged.canon(), core().canon());
    }

    #[test]
    fn hash_commits_to_the_predecessor() {
        let c = core();
        let a = c.hash(&ZERO);
        let b = c.hash(&Hash::of(b"different prev"));
        assert_ne!(a, b);

        let mut expect = Sha256::new();
        expect.update(c.canon());
        expect.update(ZERO.0);
        assert_eq!(a, Hash(expect.finalize().into()));
    }

    #[test]
    fn matches_an_independently_computed_vector() {
        // Cross-checked against a standalone Python implementation so this
        // crate is not merely consistent with itself.
        assert_eq!(
            core().hash(&ZERO).to_hex(),
            "ab8fe526b13ca7c3d3ab687024c7c9bb08cdafe69f0c92f18b138dd25ba6472c"
        );
    }
}
