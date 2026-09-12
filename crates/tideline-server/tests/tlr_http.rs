//! The `/v1` binding end to end, over HTTP.
//!
//! The last test is the one that matters: a record built entirely through the
//! public API verifies with the standalone verifier, with no server involved.

use base64::Engine as _;
use serde_json::{json, Value};
use tideline::manager::StreamManager;
use tideline_proto::{verify_chain, Checkpoint, RunEvent};

async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn base() -> String {
    serve(tideline::api::router_with(StreamManager::new())).await
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn create_run(base: &str, id: &str) -> reqwest::Response {
    client()
        .post(format!("{base}/v1/runs"))
        .json(&json!({ "run_id": id, "agent": { "name": "underwriter", "version": "2.1.0" } }))
        .send()
        .await
        .unwrap()
}

async fn append(base: &str, id: &str, body: Value) -> reqwest::Response {
    client()
        .post(format!("{base}/v1/runs/{id}/events"))
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn every_response_states_the_protocol_version() {
    let base = base().await;
    let r = client()
        .get(format!("{base}/v1/.well-known/tideline"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers().get("tideline-protocol").unwrap(), "1");
    let doc: Value = r.json().await.unwrap();
    assert_eq!(doc["protocol_versions"][0], 1);
    assert!(doc["keys"][0]["public_key"].as_str().unwrap().len() > 16);
}

#[tokio::test]
async fn creating_a_run_returns_its_envelope() {
    let base = base().await;
    let r = create_run(&base, "r1").await;
    assert_eq!(r.status(), 200);
    let env: Value = r.json().await.unwrap();
    assert_eq!(env["run_id"], "r1");
    assert_eq!(env["head_seq"], 0);
    assert_eq!(env["agent"]["name"], "underwriter");
    assert_ne!(env["head_hash"].as_str().unwrap(), &"0".repeat(64));
}

#[tokio::test]
async fn a_duplicate_run_id_conflicts() {
    let base = base().await;
    create_run(&base, "r1").await;
    assert_eq!(create_run(&base, "r1").await.status(), 409);
}

#[tokio::test]
async fn an_invalid_run_id_is_rejected() {
    let base = base().await;
    let r = create_run(&base, "bad!id").await;
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn a_missing_run_is_404() {
    let base = base().await;
    let r = client()
        .get(format!("{base}/v1/runs/nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn appending_returns_the_assigned_position_and_hashes() {
    let base = base().await;
    create_run(&base, "r1").await;
    let r = append(
        &base,
        "r1",
        json!({ "kind": "tool_call", "name": "pull_credit_report", "content": "score=690" }),
    )
    .await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["seq"], 1);
    assert!(v["ts"].as_u64().unwrap() > 0);
    assert_eq!(v["hash"].as_str().unwrap().len(), 64);
    assert_eq!(v["prev_hash"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn an_unrecognised_kind_is_rejected() {
    // Substituting a default would silently relabel the event.
    let base = base().await;
    create_run(&base, "r1").await;
    let r = append(&base, "r1", json!({ "kind": "wire_transfer" })).await;
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn a_repeated_idempotency_key_appends_once() {
    let base = base().await;
    create_run(&base, "r1").await;
    let mut seqs = Vec::new();
    for _ in 0..2 {
        let r = client()
            .post(format!("{base}/v1/runs/r1/events"))
            .header("idempotency-key", "retry-1")
            .json(&json!({ "kind": "message", "content": "once" }))
            .send()
            .await
            .unwrap();
        let v: Value = r.json().await.unwrap();
        seqs.push(v["seq"].as_u64().unwrap());
    }
    assert_eq!(seqs[0], seqs[1]);

    let events: Vec<RunEvent> = client()
        .get(format!("{base}/v1/runs/r1/events"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(events.len(), 2, "run_started plus one append");
}

#[tokio::test]
async fn events_paginate() {
    let base = base().await;
    create_run(&base, "r1").await;
    for i in 0..6 {
        append(
            &base,
            "r1",
            json!({ "kind": "message", "content": i.to_string() }),
        )
        .await;
    }
    let events: Vec<RunEvent> = client()
        .get(format!("{base}/v1/runs/r1/events?from=2&limit=3"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].seq, 2);
}

#[tokio::test]
async fn runs_can_be_listed() {
    let base = base().await;
    create_run(&base, "r1").await;
    let page: Value = client()
        .get(format!("{base}/v1/runs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["runs"].as_array().unwrap().len(), 1);
    assert_eq!(page["runs"][0]["run_id"], "r1");
}

#[tokio::test]
async fn a_gate_opens_lists_and_resolves_once() {
    let base = base().await;
    create_run(&base, "r1").await;

    let opened: Value = client()
        .post(format!("{base}/v1/runs/r1/approvals"))
        .json(&json!({ "action": "Approve EUR40,000 loan", "expires_in": 7200 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let seq = opened["seq"].as_u64().unwrap();
    assert!(opened["expires_at"].as_u64().unwrap() > 0);

    let pending: Value = client()
        .get(format!("{base}/v1/runs/r1/approvals"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["action"], "Approve EUR40,000 loan");
    assert_eq!(pending[0]["state"], "pending");

    let r = client()
        .post(format!("{base}/v1/runs/r1/approvals/{seq}/resolve"))
        .json(&json!({ "decision": "approved", "reviewer": "Jane Okafor (Credit Risk)" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let resolved: Value = r.json().await.unwrap();
    // No control plane configured, so the server cannot vouch for the reviewer.
    assert_eq!(resolved["attested"], false);

    let again = client()
        .post(format!("{base}/v1/runs/r1/approvals/{seq}/resolve"))
        .json(&json!({ "decision": "rejected" }))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 409);

    let one: Value = client()
        .get(format!("{base}/v1/runs/r1/approvals/{seq}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(one["decision"], "approved");
}

#[tokio::test]
async fn a_reviewer_cannot_write_an_expiry_decision() {
    let base = base().await;
    create_run(&base, "r1").await;
    let opened: Value = client()
        .post(format!("{base}/v1/runs/r1/approvals"))
        .json(&json!({ "action": "act" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let seq = opened["seq"].as_u64().unwrap();
    let r = client()
        .post(format!("{base}/v1/runs/r1/approvals/{seq}/resolve"))
        .json(&json!({ "decision": "expired" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn sealing_blocks_further_appends() {
    let base = base().await;
    create_run(&base, "r1").await;
    append(&base, "r1", json!({ "kind": "message", "content": "a" })).await;
    let r = client()
        .post(format!("{base}/v1/runs/r1/complete"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let late = append(&base, "r1", json!({ "kind": "message", "content": "late" })).await;
    assert_eq!(late.status(), 409);
}

#[tokio::test]
async fn redaction_requires_a_stated_authority() {
    // An erasure with no stated authority is not auditable.
    let base = base().await;
    create_run(&base, "r1").await;
    append(
        &base,
        "r1",
        json!({ "kind": "message", "content": "personal" }),
    )
    .await;
    let r = client()
        .post(format!("{base}/v1/runs/r1/redactions"))
        .json(&json!({ "target_seq": 1, "fields": ["content"], "authority": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn a_checkpoint_is_written_on_seal_and_verifies_against_the_published_key() {
    let base = base().await;
    create_run(&base, "r1").await;
    append(&base, "r1", json!({ "kind": "message", "content": "a" })).await;

    assert_eq!(
        client()
            .get(format!("{base}/v1/runs/r1/checkpoint"))
            .send()
            .await
            .unwrap()
            .status(),
        404,
        "no checkpoint before the run is sealed"
    );

    client()
        .post(format!("{base}/v1/runs/r1/complete"))
        .send()
        .await
        .unwrap();

    let cp: Checkpoint = client()
        .get(format!("{base}/v1/runs/r1/checkpoint"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let doc: Value = client()
        .get(format!("{base}/v1/.well-known/tideline"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(doc["keys"][0]["public_key"].as_str().unwrap())
        .unwrap();
    let key = ed25519_dalek::VerifyingKey::from_bytes(&raw.try_into().unwrap()).unwrap();

    assert_eq!(cp.key_id, doc["keys"][0]["id"].as_str().unwrap());
    cp.verify(&key)
        .expect("checkpoint verifies against the published key");
}

#[tokio::test]
async fn a_record_built_over_http_verifies_offline() {
    // The end-to-end guarantee: write a whole run through the public API, fetch
    // it back, and verify it with no server in the loop.
    let base = base().await;
    create_run(&base, "audit-1").await;
    append(
        &base,
        "audit-1",
        json!({ "kind": "message", "role": "user",
                "content": "Applicant #4821 requests EUR40,000" }),
    )
    .await;
    append(
        &base,
        "audit-1",
        json!({ "kind": "tool_call", "role": "tool", "name": "pull_credit_report",
                "content": "score=690", "metadata": { "bureau": "experian", "ms": 410 } }),
    )
    .await;

    let opened: Value = client()
        .post(format!("{base}/v1/runs/audit-1/approvals"))
        .json(&json!({ "action": "Approve EUR40,000 loan" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    client()
        .post(format!(
            "{base}/v1/runs/audit-1/approvals/{}/resolve",
            opened["seq"].as_u64().unwrap()
        ))
        .json(&json!({ "decision": "approved", "reviewer": "Jane Okafor" }))
        .send()
        .await
        .unwrap();

    append(
        &base,
        "audit-1",
        json!({ "kind": "decision", "name": "loan_approved", "content": "EUR40,000" }),
    )
    .await;
    client()
        .post(format!("{base}/v1/runs/audit-1/complete"))
        .send()
        .await
        .unwrap();

    let events: Vec<RunEvent> = client()
        .get(format!("{base}/v1/runs/audit-1/events"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let ok = verify_chain(&events).expect("the fetched record verifies");
    // run_started, message, tool_call, approval_requested, approval_resolved,
    // decision, run_finished.
    assert_eq!(ok.len, 7);
    assert!(ok.sealed);
}

#[tokio::test]
async fn watch_survives_the_spine_being_reset() {
    // The spine's offsets are its own. A reaped in-memory stream or an expired
    // Redis stream restarts them at zero while the record is at a much higher
    // sequence, so a watcher that treated one as the other would silently skip
    // events. Deleting the stream reproduces exactly that.
    let base = base().await;
    create_run(&base, "reset").await;
    for i in 0..3 {
        append(
            &base,
            "reset",
            json!({ "kind": "message", "content": format!("m{i}") }),
        )
        .await;
    }

    // Drop the transient stream, leaving the durable record untouched.
    let dropped = client()
        .delete(format!("{base}/streams/tlr.reset"))
        .send()
        .await
        .unwrap();
    assert!(dropped.status().is_success() || dropped.status() == 404);

    append(
        &base,
        "reset",
        json!({ "kind": "message", "content": "after the reset" }),
    )
    .await;

    // Everything is still delivered, in order, from sequence zero.
    let body = client()
        .get(format!("{base}/v1/runs/reset/watch?from=0"))
        .header("accept", "text/event-stream")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .unwrap();

    let text = read_sse_until(body, 5).await;
    let events: Vec<RunEvent> = text
        .iter()
        .map(|frame| serde_json::from_str(frame).expect("an event per frame"))
        .collect();

    assert_eq!(events.len(), 5, "run_started plus four appends");
    assert_eq!(events[4].content.as_deref(), Some("after the reset"));
    // The ids carry record sequences, not spine offsets.
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
    }
    verify_chain(&events).expect("what the watcher saw is the record");
}

/// Collect `want` SSE `data:` payloads from a streaming response.
async fn read_sse_until(res: reqwest::Response, want: usize) -> Vec<String> {
    use futures::StreamExt;
    let mut stream = res.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut out: Vec<String> = Vec::new();

    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk.unwrap());
        while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = buf.drain(..end + 2).collect();
            let text = String::from_utf8_lossy(&frame[..end]).to_string();
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("data:") {
                    let data = rest.trim_start();
                    if !data.is_empty() {
                        out.push(data.to_string());
                    }
                }
            }
        }
        if out.len() >= want {
            return out;
        }
    }
    out
}
