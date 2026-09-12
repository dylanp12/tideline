//! Emits the TLR/1 conformance corpus into `conformance/corpus/`.
//!
//! Run from the repo root: `cargo run -p tideline-proto --example gen_corpus`
//! The output is committed. Regenerating it is a protocol change and must be
//! reviewed as one.

use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tideline_proto::{EventKind, Hash, RunEvent, ZERO};

fn vector(name: &str, event_json: &str, prev: Hash) -> Value {
    let e: RunEvent = serde_json::from_str(event_json).expect("vector parses");
    let canon = e.core().canon();
    json!({
        "name": name,
        // Stored as a string, deliberately. Re-parsing into a JSON value would
        // normalise metadata spacing and key order, so the committed bytes
        // would stop matching the bytes the hash was taken over — silently
        // breaking the two vectors that exist to pin exactly that.
        "event_json": event_json,
        "prev_hash": prev.to_hex(),
        "canon_sha256": Hash::of(&canon).to_hex(),
        "hash": e.core().hash(&prev).to_hex(),
    })
}

fn main() {
    let prev = Hash::of(b"a previous event");
    let big = format!(
        r#"{{"seq":7,"ts":1757635208000,"kind":"message","content":"{}"}}"#,
        "x".repeat(100_000)
    );
    let redacted_vec = format!(
        r#"{{"seq":6,"ts":1757635207000,"kind":"tool_call","name":"kyc_lookup","redacted":{{"content":"{}"}}}}"#,
        Hash::of(b"applicant dossier").to_hex()
    );

    let events = json!({
        "protocol": "tlr/1",
        "note": "Canon is 181 bytes. canon_sha256 is SHA-256 of those bytes; hash is \
                 SHA-256(canon || prev_hash). `event_json` is the exact source text an \
                 implementation must parse — do not reformat it.",
        "vectors": [
            vector("minimal-run-started",
                r#"{"seq":0,"ts":1757635200000,"kind":"run_started"}"#, ZERO),
            vector("all-fields",
                r#"{"seq":3,"ts":1757635204120,"kind":"tool_call","role":"tool","name":"pull_credit_report","content":"score=690","metadata":{"bureau":"experian","ms":410}}"#, prev),
            vector("empty-string-differs-from-absent",
                r#"{"seq":1,"ts":1757635201000,"kind":"message","role":"","content":""}"#, prev),
            vector("unicode-in-every-field",
                r#"{"seq":2,"ts":1757635202000,"kind":"message","role":"用户","name":"naïve-café","content":"€40 000 — ≈ 38% DTI 🏦","metadata":{"jurisdiction":"ÅLAND"}}"#, prev),
            vector("metadata-key-order-is-preserved",
                r#"{"seq":4,"ts":1757635205000,"kind":"model_call","metadata":{"z":1,"a":2,"nested":{"b":[1,2.5,null,true]}}}"#, prev),
            vector("metadata-spacing-is-preserved",
                r#"{"seq":5,"ts":1757635206000,"kind":"model_call","metadata":{ "a" : 1 }}"#, prev),
            vector("redacted-content-keeps-its-digest", &redacted_vec, prev),
            vector("large-content", &big, prev),
        ]
    });

    // A valid chain, then each way of breaking one.
    let mut chain: Vec<RunEvent> = Vec::new();
    let mut prev_hash = ZERO;
    for seq in 0..4u64 {
        let mut e = RunEvent {
            seq,
            ts: 1_757_635_200_000 + seq,
            kind: if seq == 0 {
                EventKind::RunStarted
            } else {
                EventKind::Message
            },
            content: Some(format!("event {seq}")),
            prev_hash,
            ..Default::default()
        };
        e.hash = e.core().hash(&prev_hash);
        prev_hash = e.hash;
        chain.push(e);
    }

    let mut edited = chain.clone();
    edited[2].content = Some("tampered".into());

    let mut deleted = chain.clone();
    deleted.remove(2);

    let mut redacted = chain.clone();
    let kept: BTreeMap<String, Hash> = [("content".to_string(), Hash::of(b"event 2"))]
        .into_iter()
        .collect();
    redacted[2].content = None;
    redacted[2].redacted = Some(kept);

    // Cases carry their events as source text, for the same reason vectors do:
    // an implementation that parses and re-serialises would reorder metadata
    // keys and change the hashes, so every language must see identical bytes.
    let case = |name: &str, events: &[RunEvent], expect: &str, at_seq: Option<u64>| {
        let mut v = json!({
            "name": name,
            "events_json": serde_json::to_string(events).expect("serialise case"),
            "expect": expect,
        });
        if let Some(seq) = at_seq {
            v["at_seq"] = json!(seq);
        }
        v
    };

    let chains = json!({
        "protocol": "tlr/1",
        "note": "`events_json` is the exact source text an implementation must parse.",
        "cases": [
            case("valid", &chain, "ok", None),
            case("valid-after-redaction", &redacted, "ok", None),
            case("edited-content", &edited, "hash_mismatch", Some(2)),
            case("deleted-event", &deleted, "seq_gap", Some(2)),
            case("empty", &[], "empty", None),
        ]
    });

    // Fixtures: whole records, in the shape a CLI or an auditor receives them.
    let mut sealed = chain.clone();
    let mut prev = sealed.last().expect("non-empty").hash;
    for (seq, kind) in [
        (4u64, EventKind::Message),
        (5, EventKind::Decision),
        (6, EventKind::Message),
    ] {
        let mut e = RunEvent {
            seq,
            ts: 1_757_635_200_000 + seq,
            kind,
            content: Some(format!("event {seq}")),
            prev_hash: prev,
            ..Default::default()
        };
        e.hash = e.core().hash(&prev);
        prev = e.hash;
        sealed.push(e);
    }
    let mut finish = RunEvent {
        seq: 7,
        ts: 1_757_635_200_007,
        kind: EventKind::RunFinished,
        metadata: serde_json::value::RawValue::from_string(json!({ "event_count": 8 }).to_string())
            .ok(),
        prev_hash: prev,
        ..Default::default()
    };
    finish.hash = finish.core().hash(&prev);
    sealed.push(finish);

    let mut tampered = sealed.clone();
    tampered[3].content = Some("altered after the fact".into());

    // A checkpoint over the sealed record, signed with a fixed seed so the
    // fixture is reproducible. This is what makes truncation detectable: the
    // chain alone cannot see a removed tail.
    let key = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
    let head = sealed.last().expect("non-empty");
    let checkpoint = tideline_proto::Checkpoint::sign(
        "fixture-run",
        head.seq,
        head.hash,
        1_757_635_260_000,
        "fixture-key",
        &key,
    );

    std::fs::create_dir_all("conformance/fixtures").expect("create fixtures dir");
    {
        let mut s = serde_json::to_string_pretty(&checkpoint).expect("serialise checkpoint");
        s.push('\n');
        std::fs::write("conformance/fixtures/sealed-run-checkpoint.json", s)
            .expect("write checkpoint");
        let public =
            base64::engine::general_purpose::STANDARD.encode(key.verifying_key().as_bytes());
        std::fs::write(
            "conformance/fixtures/sealed-run-pubkey.txt",
            format!("{public}\n"),
        )
        .expect("write public key");
        println!("wrote conformance/fixtures/sealed-run-checkpoint.json");
        println!("wrote conformance/fixtures/sealed-run-pubkey.txt");
    }
    for (path, events) in [
        ("conformance/fixtures/sealed-run.json", &sealed),
        ("conformance/fixtures/tampered-run.json", &tampered),
        ("conformance/fixtures/open-run.json", &chain),
    ] {
        let mut s = serde_json::to_string_pretty(events).expect("serialise fixture");
        s.push('\n');
        std::fs::write(path, s).expect("write fixture");
        println!("wrote {path}");
    }

    std::fs::create_dir_all("conformance/corpus").expect("create corpus dir");
    for (path, value) in [
        ("conformance/corpus/events.json", events),
        ("conformance/corpus/chains.json", chains),
    ] {
        let mut s = serde_json::to_string_pretty(&value).expect("serialise corpus");
        s.push('\n');
        std::fs::write(path, s).expect("write corpus");
        println!("wrote {path}");
    }
}
