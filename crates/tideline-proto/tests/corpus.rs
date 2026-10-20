//! Replays the committed conformance corpus. A change here means the protocol
//! changed, and every published SDK is now wrong.

use serde_json::Value;
use tideline_proto::{verify_chain, ChainError, Hash, RunEvent};

/// Load a corpus file, or `None` when it is absent.
///
/// The corpus lives at the repo root, not inside the crate, so it is not part of
/// the published package. A downstream `cargo test` on the crate alone should
/// skip these rather than fail on a missing file.
fn load(path: &str) -> Option<Value> {
    let full = concat!(env!("CARGO_MANIFEST_DIR"), "/../..").to_string() + "/" + path;
    let text = std::fs::read_to_string(&full).ok()?;
    Some(serde_json::from_str(&text).expect("corpus is valid JSON"))
}

#[test]
fn every_event_vector_reproduces() {
    let Some(corpus) = load("conformance/corpus/events.json") else {
        eprintln!("corpus absent (published crate) — skipping");
        return;
    };
    let vectors = corpus["vectors"].as_array().expect("vectors array");
    assert!(vectors.len() >= 8, "corpus shrank unexpectedly");

    for v in vectors {
        let name = v["name"].as_str().unwrap();
        let event: RunEvent =
            serde_json::from_str(v["event_json"].as_str().expect(name)).expect(name);
        let prev = Hash::from_hex(v["prev_hash"].as_str().unwrap()).expect(name);

        assert_eq!(
            Hash::of(&event.core().canon()).to_hex(),
            v["canon_sha256"].as_str().unwrap(),
            "canon changed for vector {name}"
        );
        assert_eq!(
            event.core().hash(&prev).to_hex(),
            v["hash"].as_str().unwrap(),
            "hash changed for vector {name}"
        );
    }
}

#[test]
fn every_chain_case_reaches_its_verdict() {
    let Some(corpus) = load("conformance/corpus/chains.json") else {
        eprintln!("corpus absent (published crate) — skipping");
        return;
    };
    for case in corpus["cases"].as_array().expect("cases array") {
        let name = case["name"].as_str().unwrap();
        let events: Vec<RunEvent> =
            serde_json::from_str(case["events_json"].as_str().expect(name)).expect(name);
        let got = verify_chain(&events);

        match case["expect"].as_str().unwrap() {
            "ok" => {
                got.unwrap_or_else(|e| panic!("case {name} should verify, got {e:?}"));
            }
            "empty" => assert_eq!(got.unwrap_err(), ChainError::Empty, "case {name}"),
            "hash_mismatch" => match got.unwrap_err() {
                ChainError::HashMismatch { seq, .. } => {
                    assert_eq!(seq, case["at_seq"].as_u64().unwrap(), "case {name}")
                }
                other => panic!("case {name}: expected HashMismatch, got {other:?}"),
            },
            "seq_gap" => match got.unwrap_err() {
                ChainError::SeqGap { expected, .. } => {
                    assert_eq!(expected, case["at_seq"].as_u64().unwrap(), "case {name}")
                }
                other => panic!("case {name}: expected SeqGap, got {other:?}"),
            },
            other => panic!("case {name}: unknown expectation {other}"),
        }
    }
}
