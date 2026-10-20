use crate::{EventCore, EventKind, Hash, ZERO};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

/// One event on the record.
///
/// `metadata` is held as `RawValue` so the exact bytes the client sent survive
/// to the hasher untouched.
///
/// **Deserialize a record straight from the response bytes.** Parsing into a
/// generic JSON value first and then converting — `serde_json::from_value`, or
/// its equivalent in any language — re-serialises the metadata and reorders its
/// keys, which changes the digest and makes a perfectly good record fail
/// verification. Use `serde_json::from_str`/`from_slice`, or `reqwest`'s
/// `.json()`, which does the same. See `key_order_changes_the_hash` below for
/// why the protocol cannot paper over this.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunEvent {
    pub seq: u64,
    pub ts: u64,
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Box<RawValue>>,
    /// Retained digests of fields erased by a redaction, keyed by field name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<BTreeMap<String, Hash>>,
    #[serde(default)]
    pub prev_hash: Hash,
    #[serde(default)]
    pub hash: Hash,
}

impl RunEvent {
    /// The metadata exactly as it arrived, or `None` when unset.
    pub fn metadata_raw(&self) -> Option<&str> {
        self.metadata.as_deref().map(RawValue::get)
    }

    /// The hashable core of this event.
    pub fn core(&self) -> EventCore<'_> {
        EventCore {
            seq: self.seq,
            ts: self.ts,
            kind: self.kind,
            role: self.role.as_deref(),
            name: self.name.as_deref(),
            content: self.content.as_deref(),
            metadata_raw: self.metadata_raw(),
            redacted: self.redacted.as_ref(),
        }
    }
}

/// Who ran, when, and about what. Written into the chain as the metadata of the
/// `run_started` event, so it is committed like any other field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunEnvelope {
    pub run_id: String,
    pub started_ts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ts: Option<u64>,
    pub agent: Agent,
    /// An opaque reference to the subject of the decision. Never personal data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_ref: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub head_seq: u64,
    #[serde(default)]
    pub head_hash: Hash,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    pub version: String,
}

impl Agent {
    pub fn new(name: &str, version: &str) -> Agent {
        Agent {
            name: name.to_string(),
            version: version.to_string(),
        }
    }
}

impl Default for RunEvent {
    fn default() -> Self {
        RunEvent {
            seq: 0,
            ts: 0,
            kind: EventKind::Message,
            role: None,
            name: None,
            content: None,
            metadata: None,
            redacted: None,
            prev_hash: ZERO,
            hash: ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRE: &str = r#"{
        "seq": 3, "ts": 1757635204120, "kind": "tool_call",
        "role": "tool", "name": "pull_credit_report", "content": "score=690",
        "metadata": {"bureau":"experian","ms":410},
        "prev_hash": "0000000000000000000000000000000000000000000000000000000000000000",
        "hash": "0000000000000000000000000000000000000000000000000000000000000000"
    }"#;

    #[test]
    fn preserves_metadata_bytes_verbatim() {
        // Canon hashes the bytes the client sent. Re-serialisation would
        // change spacing or key order and break every downstream verifier.
        let e: RunEvent = serde_json::from_str(WIRE).unwrap();
        assert_eq!(e.metadata_raw(), Some(r#"{"bureau":"experian","ms":410}"#));
    }

    #[test]
    fn exposes_a_core_for_hashing() {
        let e: RunEvent = serde_json::from_str(WIRE).unwrap();
        let c = e.core();
        assert_eq!(c.seq, 3);
        assert_eq!(c.kind, EventKind::ToolCall);
        assert_eq!(c.content, Some("score=690"));
        assert_eq!(c.metadata_raw, Some(r#"{"bureau":"experian","ms":410}"#));
    }

    #[test]
    fn omitted_fields_are_none_not_empty() {
        let e: RunEvent = serde_json::from_str(r#"{"seq":0,"ts":1,"kind":"run_started"}"#).unwrap();
        assert!(e.role.is_none());
        assert!(e.content.is_none());
        assert_eq!(e.metadata_raw(), None);
        assert!(e.prev_hash.is_zero());
    }

    #[test]
    fn a_direct_parse_preserves_metadata_bytes_exactly() {
        // The one thing the whole chain rests on: what the server sent is what
        // gets hashed.
        let src = r#"{"seq":1,"ts":2,"kind":"model_call","metadata":{ "z":1, "a":2 }}"#;
        let e: RunEvent = serde_json::from_str(src).unwrap();
        assert_eq!(e.metadata_raw(), Some(r#"{ "z":1, "a":2 }"#));
    }

    #[test]
    fn key_order_changes_the_hash() {
        // Why a client must not re-serialise metadata. These two events are
        // equal as JSON documents and different as records, because canon
        // commits to bytes, not to a parsed value.
        //
        // The protocol could have avoided this with a canonicalisation scheme,
        // but RFC 8785's number and Unicode rules disagree across languages far
        // more often than key order does, and hashing what the agent actually
        // sent is worth more than hashing a normalisation of it.
        let a: RunEvent = serde_json::from_str(
            r#"{"seq":1,"ts":2,"kind":"model_call","metadata":{"z":1,"a":2}}"#,
        )
        .unwrap();
        let b: RunEvent = serde_json::from_str(
            r#"{"seq":1,"ts":2,"kind":"model_call","metadata":{"a":2,"z":1}}"#,
        )
        .unwrap();
        assert_ne!(a.core().hash(&ZERO), b.core().hash(&ZERO));
    }

    #[test]
    fn envelope_round_trips() {
        let env = RunEnvelope {
            run_id: "loan-4821".into(),
            started_ts: 1757635200000,
            ended_ts: None,
            agent: Agent {
                name: "underwriter".into(),
                version: "2.1.0".into(),
            },
            subject_ref: Some("applicant-4821".into()),
            labels: [("product".to_string(), "personal-loan".to_string())]
                .into_iter()
                .collect(),
            head_seq: 0,
            head_hash: ZERO,
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: RunEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agent.name, "underwriter");
        assert_eq!(back.subject_ref.as_deref(), Some("applicant-4821"));
        assert_eq!(back.labels["product"], "personal-loan");
    }
}
