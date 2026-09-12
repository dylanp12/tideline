//! The SDK against a live reference server.

use std::time::Duration;
use tideline_proto::{verify_chain, ChainError, EventKind};
use tideline_sdk::{Agent, Decision, NewEvent, Tideline};

async fn serve() -> String {
    let app = tideline::api::router_with(tideline::manager::StreamManager::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn client() -> Tideline {
    Tideline::new(&serve().await, None)
}

#[tokio::test]
async fn records_a_run_and_verifies_it() {
    let tl = client().await;
    let run = tl
        .start_run("loan-1", Agent::new("underwriter", "2.1.0"))
        .subject_ref("applicant-4821")
        .label("product", "personal-loan")
        .send()
        .await
        .unwrap();

    run.record(
        NewEvent::tool_call("pull_credit_report")
            .role("tool")
            .content("score=690")
            .metadata(&serde_json::json!({ "bureau": "experian", "ms": 410 })),
    )
    .await
    .unwrap();
    run.complete().await.unwrap();

    let events = run.verified_events().await.unwrap();
    let ok = verify_chain(&events).unwrap();
    assert!(ok.sealed);
    assert_eq!(events[1].kind, EventKind::ToolCall);
    // The metadata survived the round trip byte for byte, which is the whole
    // reason the record verifies at all.
    assert_eq!(
        events[1].metadata_raw(),
        Some(r#"{"bureau":"experian","ms":410}"#)
    );
}

#[tokio::test]
async fn the_envelope_reports_the_head() {
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();
    run.record(NewEvent::message().content("x")).await.unwrap();
    let env = run.envelope().await.unwrap();
    assert_eq!(env.head_seq, 1);
    assert_eq!(env.agent.name, "a");
}

#[tokio::test]
async fn a_retry_with_the_same_key_appends_once() {
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();

    let a = run
        .record_idempotent(NewEvent::message().content("once"), "k-1")
        .await
        .unwrap();
    let b = run
        .record_idempotent(NewEvent::message().content("once"), "k-1")
        .await
        .unwrap();
    assert_eq!(a.seq, b.seq);
    assert_eq!(a.hash, b.hash);
    assert_eq!(run.events().await.unwrap().len(), 2);
}

#[tokio::test]
async fn gate_blocks_until_a_reviewer_resolves_it() {
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();

    // A reviewer arrives a moment later, as one does.
    let reviewer = tl.run("r1");
    tokio::spawn(async move {
        for _ in 0..100 {
            if let Ok(pending) = reviewer.approvals().await {
                if let Some(g) = pending.first() {
                    let _ = reviewer
                        .resolve(
                            g.seq,
                            Decision::Approved,
                            Some("Jane Okafor"),
                            Some("within policy"),
                        )
                        .await;
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let resolution = run
        .gate("Approve EUR40,000 loan", Duration::from_secs(60))
        .await
        .unwrap();
    assert!(resolution.is_approved());
    assert_eq!(resolution.reviewer.as_deref(), Some("Jane Okafor"));
    // No control plane configured, so the server cannot vouch for who that was.
    assert!(!resolution.attested);

    // Both halves of the gate are in the chain by the time gate() returns.
    verify_chain(&run.events().await.unwrap()).unwrap();
}

#[tokio::test]
async fn a_rejection_is_a_value_not_an_error() {
    // An agent must be able to branch on a refusal. Raising would force every
    // caller into a catch block for the normal case.
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();
    let opened = run
        .open_gate("Approve EUR900,000 loan", Duration::from_secs(60))
        .await
        .unwrap();

    run.resolve(
        opened.seq,
        Decision::Rejected,
        Some("Risk"),
        Some("exceeds mandate"),
    )
    .await
    .unwrap();

    let resolution = run.await_gate(opened.seq).await.unwrap();
    assert_eq!(resolution.decision, Decision::Rejected);
    assert!(!resolution.is_approved());
}

#[tokio::test]
async fn a_second_resolution_is_a_conflict_the_caller_can_recognise() {
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();
    let opened = run.open_gate("act", Duration::from_secs(60)).await.unwrap();
    run.resolve(opened.seq, Decision::Approved, None, None)
        .await
        .unwrap();

    let err = run
        .resolve(opened.seq, Decision::Rejected, None, None)
        .await
        .unwrap_err();
    assert!(err.is_conflict(), "got {err}");
}

#[tokio::test]
async fn a_tampered_record_fails_verification_and_names_the_event() {
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();
    for i in 0..4 {
        run.record(NewEvent::message().content(format!("m{i}")))
            .await
            .unwrap();
    }

    let mut events = run.events().await.unwrap();
    events[3].content = Some("altered after the fact".into());

    match verify_chain(&events).unwrap_err() {
        ChainError::HashMismatch { seq, .. } => assert_eq!(seq, 3),
        other => panic!("expected HashMismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn redaction_keeps_the_record_verifiable() {
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();
    run.record(NewEvent::tool_call("kyc").content("applicant dossier"))
        .await
        .unwrap();

    run.redact(1, &["content"], "GDPR Art 17 request #55")
        .await
        .unwrap();

    let events = run.verified_events().await.unwrap();
    assert!(events[1].content.is_none(), "plaintext erased");
    assert!(
        events[1].redacted.is_some(),
        "digest retained so the chain still holds"
    );
}

#[tokio::test]
async fn checkpoints_appear_on_seal_and_verify() {
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();
    assert!(run.checkpoint().await.unwrap().is_none());

    run.complete().await.unwrap();
    let cp = run
        .checkpoint()
        .await
        .unwrap()
        .expect("sealed run is checkpointed");
    assert_eq!(cp.run_id, "r1");

    let doc = tl.well_known().await.unwrap();
    assert_eq!(cp.key_id, doc["keys"][0]["id"].as_str().unwrap());
}

#[tokio::test]
async fn watch_streams_the_record() {
    use futures::StreamExt;

    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();
    run.record(NewEvent::message().content("first"))
        .await
        .unwrap();

    let mut seen = Vec::new();
    let mut stream = Box::pin(run.watch(0).await.unwrap());
    // History replays immediately; take it and stop.
    while let Some(Ok(e)) = stream.next().await {
        seen.push(e);
        if seen.len() == 2 {
            break;
        }
    }
    assert_eq!(seen[0].kind, EventKind::RunStarted);
    assert_eq!(seen[1].content.as_deref(), Some("first"));
}

#[tokio::test]
async fn runs_can_be_listed_and_filtered() {
    use tideline_sdk::RunQuery;
    let tl = client().await;
    tl.start_run("r1", Agent::new("underwriter", "1"))
        .label("product", "loan")
        .send()
        .await
        .unwrap();
    tl.start_run("r2", Agent::new("pricing", "1"))
        .send()
        .await
        .unwrap();

    let all = tl.list_runs(&RunQuery::default()).await.unwrap();
    assert_eq!(all.runs.len(), 2);

    let filtered = tl
        .list_runs(&RunQuery {
            agent: Some("pricing".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(filtered.runs.len(), 1);
    assert_eq!(filtered.runs[0].run_id, "r2");
}

#[tokio::test]
async fn events_pages_through_the_whole_record() {
    // A record longer than one page used to come back as a valid prefix, which
    // verifies and is missing evidence — the worst possible pair. Tested with a
    // small page rather than a thousand appends: the loop is what breaks, not
    // the constant.
    let tl = client().await;
    let run = tl
        .start_run("long", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();
    for i in 0..9 {
        run.record(NewEvent::message().content(format!("m{i}")))
            .await
            .unwrap();
    }

    for page in [1, 2, 3, 5, 10, 100] {
        let events = run.events_paged(page).await.unwrap();
        assert_eq!(events.len(), 10, "page size {page}");
        assert_eq!(events.last().unwrap().seq, 9, "page size {page}");
        verify_chain(&events).expect("the whole record verifies");
    }

    // An exact multiple of the page size, where the loop needs one more request
    // to learn that it is finished.
    run.record(NewEvent::message().content("m9")).await.unwrap();
    assert_eq!(run.events_paged(1).await.unwrap().len(), 11);
}

#[tokio::test]
async fn lifecycle_kinds_cannot_be_appended() {
    // Appending these would put a second envelope at a nonzero seq, or a
    // terminal event that does not seal the run: a record that is invalid by
    // shape but which the chain verifier still accepts.
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();

    for kind in [
        EventKind::RunStarted,
        EventKind::RunFinished,
        EventKind::Redaction,
    ] {
        let err = run
            .record(NewEvent::new(kind).content("x"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, tideline_sdk::Error::Status { code: 400, .. }),
            "{} should be refused, got {err}",
            kind.as_str()
        );
    }

    // And the record is still exactly what it was.
    let events = run.events().await.unwrap();
    assert_eq!(events.len(), 1);
    verify_chain(&events).unwrap();
}

#[tokio::test]
async fn a_lapsed_gate_expires_when_anyone_looks() {
    // Nothing used to expire a gate, so `gate()` polled forever and the
    // required `expired` resolution never reached the record.
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();

    // Already lapsed.
    let opened = run
        .open_gate("Approve something", Duration::from_secs(0))
        .await
        .unwrap();

    let resolution = run.await_gate(opened.seq).await.unwrap();
    assert_eq!(resolution.decision, Decision::Expired);
    assert!(!resolution.is_approved());

    // And it is in the chain, not merely reported.
    let events = run.verified_events().await.unwrap();
    let resolved = events
        .iter()
        .any(|e| e.name.as_deref() == Some("approval_resolved"));
    assert!(resolved, "the expiry must be written into the record");
    assert!(run.approvals().await.unwrap().is_empty(), "queue is clear");
}

#[tokio::test]
async fn completing_twice_is_a_conflict_once_checkpointed() {
    let tl = client().await;
    let run = tl
        .start_run("r1", Agent::new("a", "1"))
        .send()
        .await
        .unwrap();
    run.complete().await.unwrap();
    assert!(run.checkpoint().await.unwrap().is_some());

    let err = run.complete().await.unwrap_err();
    assert!(err.is_conflict(), "got {err}");
}
