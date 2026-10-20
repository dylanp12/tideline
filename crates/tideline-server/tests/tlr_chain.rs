//! The store's core promise: appends form a verifiable chain, retries do not
//! duplicate, and a sealed run refuses further writes.

use tideline::tlr::sqlite::SqliteTlrStore;
use tideline::tlr::store::{NewEvent, NewRun, RunQuery, StoreError, TlrStore};
use tideline_proto::{verify_chain, Agent, EventKind, ZERO};

fn store() -> SqliteTlrStore {
    SqliteTlrStore::in_memory().expect("open store")
}

fn new_run(id: &str) -> NewRun {
    NewRun {
        run_id: id.to_string(),
        agent: Agent {
            name: "underwriter".into(),
            version: "2.1.0".into(),
        },
        subject_ref: Some("applicant-4821".into()),
        labels: [("product".to_string(), "personal-loan".to_string())]
            .into_iter()
            .collect(),
    }
}

fn event(kind: EventKind, content: &str) -> NewEvent {
    NewEvent {
        kind,
        role: None,
        name: None,
        content: Some(content.to_string()),
        metadata: None,
    }
}

#[tokio::test]
async fn a_new_run_starts_with_a_chained_run_started_event() {
    let s = store();
    let env = s.create_run("t1", new_run("r1")).await.unwrap();
    assert_eq!(env.head_seq, 0);
    assert!(!env.head_hash.is_zero());

    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::RunStarted);
    assert_eq!(events[0].prev_hash, ZERO);
    // The envelope is committed by the chain, not stored beside it.
    let meta = events[0].metadata_raw().expect("envelope in metadata");
    assert!(meta.contains("underwriter"), "got {meta}");
    assert!(meta.contains("applicant-4821"), "got {meta}");
}

#[tokio::test]
async fn appends_form_a_chain_the_verifier_accepts() {
    let s = store();
    s.create_run("t1", new_run("r1")).await.unwrap();
    for i in 0..5 {
        s.append(
            "t1",
            "r1",
            event(EventKind::Message, &format!("m{i}")),
            None,
        )
        .await
        .unwrap();
    }
    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    let ok = verify_chain(&events).expect("chain verifies");
    assert_eq!(ok.len, 6);
    assert_eq!(ok.head_seq, 5);
    assert!(!ok.sealed);
}

#[tokio::test]
async fn append_returns_the_assigned_seq_and_hashes() {
    let s = store();
    let env = s.create_run("t1", new_run("r1")).await.unwrap();
    let r = s
        .append("t1", "r1", event(EventKind::ToolCall, "x"), None)
        .await
        .unwrap();
    assert_eq!(r.seq, 1);
    assert_eq!(r.prev_hash, env.head_hash);
    assert!(r.ts > 0);
}

#[tokio::test]
async fn a_repeated_idempotency_key_does_not_append_twice() {
    // A client retrying after a timeout must not place a phantom event into
    // evidence — it would verify, because the chain proves integrity, not
    // correctness at the time of writing.
    let s = store();
    s.create_run("t1", new_run("r1")).await.unwrap();
    let a = s
        .append("t1", "r1", event(EventKind::Message, "once"), Some("k-1"))
        .await
        .unwrap();
    let b = s
        .append("t1", "r1", event(EventKind::Message, "once"), Some("k-1"))
        .await
        .unwrap();
    assert_eq!(a.seq, b.seq);
    assert_eq!(a.hash, b.hash);
    assert_eq!(s.events("t1", "r1", 0, 100).await.unwrap().len(), 2);
}

#[tokio::test]
async fn different_idempotency_keys_both_append() {
    let s = store();
    s.create_run("t1", new_run("r1")).await.unwrap();
    s.append("t1", "r1", event(EventKind::Message, "a"), Some("k-1"))
        .await
        .unwrap();
    s.append("t1", "r1", event(EventKind::Message, "b"), Some("k-2"))
        .await
        .unwrap();
    assert_eq!(s.events("t1", "r1", 0, 100).await.unwrap().len(), 3);
}

#[tokio::test]
async fn sealing_writes_run_finished_and_blocks_further_appends() {
    let s = store();
    s.create_run("t1", new_run("r1")).await.unwrap();
    s.append("t1", "r1", event(EventKind::Message, "a"), None)
        .await
        .unwrap();
    s.seal("t1", "r1").await.unwrap();

    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    let ok = verify_chain(&events).unwrap();
    assert!(ok.sealed);
    // run_finished carries the count, so truncating a sealed run is detectable
    // from the record alone.
    let meta = events.last().unwrap().metadata_raw().unwrap();
    assert!(meta.contains("\"event_count\":3"), "got {meta}");

    match s
        .append("t1", "r1", event(EventKind::Message, "late"), None)
        .await
    {
        Err(StoreError::Conflict(_)) => {}
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn tenants_cannot_see_each_others_runs() {
    let s = store();
    s.create_run("t1", new_run("shared-id")).await.unwrap();
    assert_eq!(
        s.get_run("t2", "shared-id").await.unwrap_err(),
        StoreError::NotFound
    );
    assert!(s
        .events("t2", "shared-id", 0, 100)
        .await
        .unwrap()
        .is_empty());
    // Both tenants may use the same run id without collision.
    s.create_run("t2", new_run("shared-id")).await.unwrap();
}

#[tokio::test]
async fn a_duplicate_run_id_conflicts() {
    let s = store();
    s.create_run("t1", new_run("r1")).await.unwrap();
    match s.create_run("t1", new_run("r1")).await {
        Err(StoreError::Conflict(_)) => {}
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn events_paginate_from_an_offset() {
    let s = store();
    s.create_run("t1", new_run("r1")).await.unwrap();
    for i in 0..10 {
        s.append(
            "t1",
            "r1",
            event(EventKind::Message, &format!("m{i}")),
            None,
        )
        .await
        .unwrap();
    }
    let page = s.events("t1", "r1", 5, 3).await.unwrap();
    assert_eq!(page.len(), 3);
    assert_eq!(page[0].seq, 5);
    assert_eq!(page[2].seq, 7);
}

#[tokio::test]
async fn listing_runs_filters_by_agent_and_label() {
    let s = store();
    s.create_run("t1", new_run("r1")).await.unwrap();
    let mut other = new_run("r2");
    other.agent.name = "pricing".into();
    other.labels = [("product".to_string(), "motor".to_string())]
        .into_iter()
        .collect();
    s.create_run("t1", other).await.unwrap();

    let all = s.list_runs("t1", RunQuery::default()).await.unwrap();
    assert_eq!(all.runs.len(), 2);

    let by_agent = s
        .list_runs(
            "t1",
            RunQuery {
                agent: Some("pricing".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(by_agent.runs.len(), 1);
    assert_eq!(by_agent.runs[0].run_id, "r2");

    let by_label = s
        .list_runs(
            "t1",
            RunQuery {
                label: Some("product=personal-loan".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(by_label.runs.len(), 1);
    assert_eq!(by_label.runs[0].run_id, "r1");
}

#[tokio::test]
async fn a_missing_run_is_not_found_rather_than_a_panic() {
    let s = store();
    assert_eq!(
        s.get_run("t1", "nope").await.unwrap_err(),
        StoreError::NotFound
    );
    match s
        .append("t1", "nope", event(EventKind::Message, "x"), None)
        .await
    {
        Err(StoreError::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}
