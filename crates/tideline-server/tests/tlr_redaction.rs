//! Erasure under GDPR Article 17 must not destroy the Article 12 record.

use tideline::tlr::sqlite::SqliteTlrStore;
use tideline::tlr::store::{NewEvent, NewRun, StoreError, TlrStore};
use tideline_proto::{verify_chain, Agent, EventKind, Hash};

async fn run_with_content() -> SqliteTlrStore {
    let s = SqliteTlrStore::in_memory().unwrap();
    s.create_run(
        "t1",
        NewRun {
            run_id: "r1".into(),
            agent: Agent {
                name: "a".into(),
                version: "1".into(),
            },
            subject_ref: None,
            labels: Default::default(),
        },
    )
    .await
    .unwrap();
    s.append(
        "t1",
        "r1",
        NewEvent {
            kind: EventKind::ToolCall,
            role: None,
            name: Some("kyc_lookup".into()),
            content: Some("applicant dossier".into()),
            metadata: None,
        },
        None,
    )
    .await
    .unwrap();
    s.append(
        "t1",
        "r1",
        NewEvent {
            kind: EventKind::Message,
            role: None,
            name: None,
            content: Some("after".into()),
            metadata: None,
        },
        None,
    )
    .await
    .unwrap();
    s
}

#[tokio::test]
async fn redaction_erases_the_value_and_keeps_the_chain() {
    let s = run_with_content().await;
    s.redact(
        "t1",
        "r1",
        1,
        &["content".to_string()],
        "GDPR Art 17 request #55",
    )
    .await
    .unwrap();

    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    verify_chain(&events).expect("chain still verifies after erasure");

    let target = &events[1];
    assert!(target.content.is_none(), "plaintext is gone");
    assert_eq!(
        target.redacted.as_ref().unwrap().get("content"),
        Some(&Hash::of(b"applicant dossier")),
        "the digest that commits to it is retained"
    );
}

#[tokio::test]
async fn redaction_appends_an_auditable_event() {
    let s = run_with_content().await;
    s.redact(
        "t1",
        "r1",
        1,
        &["content".to_string()],
        "GDPR Art 17 request #55",
    )
    .await
    .unwrap();
    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, EventKind::Redaction);
    let meta = last.metadata_raw().unwrap();
    assert!(meta.contains("\"target_seq\":1"), "got {meta}");
    assert!(meta.contains("GDPR Art 17 request #55"), "got {meta}");
    assert!(meta.contains("content"), "got {meta}");
}

#[tokio::test]
async fn redacting_twice_is_harmless() {
    let s = run_with_content().await;
    s.redact("t1", "r1", 1, &["content".to_string()], "a")
        .await
        .unwrap();
    s.redact("t1", "r1", 1, &["content".to_string()], "b")
        .await
        .unwrap();
    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    verify_chain(&events).expect("chain still verifies");
}

#[tokio::test]
async fn structural_fields_cannot_be_redacted() {
    // Erasing seq, ts, or kind would destroy the record's shape rather than
    // its contents.
    let s = run_with_content().await;
    for field in ["seq", "ts", "kind"] {
        match s.redact("t1", "r1", 1, &[field.to_string()], "x").await {
            Err(StoreError::Invalid(_)) => {}
            other => panic!("expected Invalid for {field}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn redacting_an_unknown_event_is_not_found() {
    let s = run_with_content().await;
    assert_eq!(
        s.redact("t1", "r1", 99, &["content".to_string()], "x")
            .await
            .unwrap_err(),
        StoreError::NotFound
    );
}
