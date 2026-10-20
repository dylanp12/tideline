//! An approval gate is two events in the chain and a fold over them. Nothing
//! about a gate lives outside the record.

use tideline::tlr::approvals::{expire_lapsed, project, request, resolve, ApprovalState, Decision};
use tideline::tlr::sqlite::SqliteTlrStore;
use tideline::tlr::store::{NewRun, StoreError, TlrStore};
use tideline_proto::{verify_chain, Agent};

async fn empty_run() -> SqliteTlrStore {
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
    s
}

async fn run_with_gate() -> (SqliteTlrStore, u64) {
    let s = empty_run().await;
    let seq = request(&s, "t1", "r1", "Approve EUR40,000 loan", 1_900_000_000_000)
        .await
        .unwrap();
    (s, seq)
}

#[tokio::test]
async fn a_request_is_a_decision_event_in_the_chain() {
    let (s, seq) = run_with_gate().await;
    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    verify_chain(&events).expect("chain still verifies");

    let e = &events[seq as usize];
    assert_eq!(e.name.as_deref(), Some("approval_requested"));
    let meta = e.metadata_raw().unwrap();
    assert!(meta.contains("Approve EUR40,000 loan"), "got {meta}");
    assert!(meta.contains("expires_at"), "got {meta}");
}

#[tokio::test]
async fn a_pending_gate_projects_as_pending() {
    let (s, seq) = run_with_gate().await;
    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    let gates = project(&events);
    assert_eq!(gates.len(), 1);
    assert_eq!(gates[0].seq, seq);
    assert_eq!(gates[0].state, ApprovalState::Pending);
    assert_eq!(gates[0].action, "Approve EUR40,000 loan");
}

#[tokio::test]
async fn resolving_writes_the_decision_into_the_chain() {
    let (s, seq) = run_with_gate().await;
    resolve(
        &s,
        "t1",
        "r1",
        seq,
        Decision::Approved,
        Some("Jane Okafor (Credit Risk)"),
        Some("key:jane@bank.example"),
        true,
        Some("Income verified"),
    )
    .await
    .unwrap();

    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    verify_chain(&events).expect("chain still verifies");

    let gates = project(&events);
    assert_eq!(gates[0].state, ApprovalState::Resolved(Decision::Approved));
    assert_eq!(
        gates[0].reviewer.as_deref(),
        Some("Jane Okafor (Credit Risk)")
    );
    assert!(gates[0].attested, "an authenticated reviewer is attested");
}

#[tokio::test]
async fn an_unauthenticated_reviewer_is_recorded_as_unattested() {
    // Self-declared identity is worth recording, but claiming it was verified
    // would be a lie the record cannot support.
    let (s, seq) = run_with_gate().await;
    resolve(
        &s,
        "t1",
        "r1",
        seq,
        Decision::Approved,
        Some("anon"),
        None,
        false,
        None,
    )
    .await
    .unwrap();
    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    assert!(!project(&events)[0].attested);
}

#[tokio::test]
async fn a_second_resolution_is_refused() {
    let (s, seq) = run_with_gate().await;
    resolve(
        &s,
        "t1",
        "r1",
        seq,
        Decision::Approved,
        None,
        None,
        false,
        None,
    )
    .await
    .unwrap();
    let err = resolve(
        &s,
        "t1",
        "r1",
        seq,
        Decision::Rejected,
        None,
        None,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));

    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    assert_eq!(
        project(&events)[0].state,
        ApprovalState::Resolved(Decision::Approved)
    );
}

#[tokio::test]
async fn a_lapsed_gate_expires_into_the_chain() {
    // Nobody answering is itself evidence. A record that simply stops is
    // indistinguishable from one that was truncated.
    let s = empty_run().await;
    request(&s, "t1", "r1", "act", 1).await.unwrap();

    let n = expire_lapsed(&s, "t1", "r1", 2).await.unwrap();
    assert_eq!(n, 1);

    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    verify_chain(&events).expect("chain still verifies");
    assert_eq!(
        project(&events)[0].state,
        ApprovalState::Resolved(Decision::Expired)
    );
}

#[tokio::test]
async fn expiry_leaves_a_live_gate_alone() {
    let (s, _) = run_with_gate().await;
    assert_eq!(expire_lapsed(&s, "t1", "r1", 2).await.unwrap(), 0);
    let events = s.events("t1", "r1", 0, 100).await.unwrap();
    assert_eq!(project(&events)[0].state, ApprovalState::Pending);
}

#[tokio::test]
async fn resolving_an_unknown_gate_is_not_found() {
    let (s, _) = run_with_gate().await;
    let err = resolve(
        &s,
        "t1",
        "r1",
        99,
        Decision::Approved,
        None,
        None,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err, StoreError::NotFound);
}
