//! Human-approval gates (EU AI Act Article 14), as events in the chain.
//!
//! A gate is a `decision` event named `approval_requested` and, later, one named
//! `approval_resolved` pointing back at it. Gate state is a fold over the record
//! rather than a separate table, so the oversight trail cannot drift out of step
//! with the record it is supposed to describe.

use super::store::{NewEvent, StoreError, StoreResult, TlrStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue};
use tideline_proto::{EventKind, RunEvent};

pub const REQUESTED: &str = "approval_requested";
pub const RESOLVED: &str = "approval_resolved";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approved,
    Rejected,
    /// Nobody answered before `expires_at`. Recorded like any other outcome:
    /// a record that simply stops is indistinguishable from a truncated one.
    Expired,
}

impl Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::Approved => "approved",
            Decision::Rejected => "rejected",
            Decision::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Option<Decision> {
        match s {
            "approved" | "approve" => Some(Decision::Approved),
            "rejected" | "reject" => Some(Decision::Rejected),
            "expired" => Some(Decision::Expired),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "decision")]
pub enum ApprovalState {
    Pending,
    Resolved(Decision),
}

/// One gate, as projected from the chain.
#[derive(Clone, Debug, Serialize)]
pub struct Gate {
    pub seq: u64,
    pub action: String,
    pub expires_at: u64,
    #[serde(flatten)]
    pub state: ApprovalState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewer_principal: Option<String>,
    /// Whether the server authenticated the reviewer. Never `true` on the
    /// strength of a caller-supplied name.
    pub attested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn meta(e: &RunEvent) -> serde_json::Value {
    e.metadata_raw()
        .and_then(|m| serde_json::from_str(m).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Fold the chain into gate states, in request order.
///
/// Only the first resolution of a gate counts. A racing duplicate is still part
/// of the record — an attempt to decide twice is itself worth seeing — but it
/// cannot overwrite the decision that landed first.
pub fn project(events: &[RunEvent]) -> Vec<Gate> {
    let mut gates: Vec<Gate> = Vec::new();

    for e in events {
        if e.kind != EventKind::Decision {
            continue;
        }
        match e.name.as_deref() {
            Some(REQUESTED) => {
                let m = meta(e);
                gates.push(Gate {
                    seq: e.seq,
                    action: m["action"].as_str().unwrap_or_default().to_string(),
                    expires_at: m["expires_at"].as_u64().unwrap_or(0),
                    state: ApprovalState::Pending,
                    reviewer: None,
                    reviewer_principal: None,
                    attested: false,
                    note: None,
                });
            }
            Some(RESOLVED) => {
                let m = meta(e);
                let Some(target) = m["target_seq"].as_u64() else {
                    continue;
                };
                let Some(decision) = m["decision"].as_str().and_then(Decision::parse) else {
                    continue;
                };
                if let Some(g) = gates
                    .iter_mut()
                    .find(|g| g.seq == target && g.state == ApprovalState::Pending)
                {
                    g.state = ApprovalState::Resolved(decision);
                    g.reviewer = m["reviewer"].as_str().map(str::to_string);
                    g.reviewer_principal = m["reviewer_principal"].as_str().map(str::to_string);
                    g.attested = m["attested"].as_bool().unwrap_or(false);
                    g.note = m["note"].as_str().map(str::to_string);
                }
            }
            _ => {}
        }
    }
    gates
}

fn decision_event(name: &str, metadata: serde_json::Value) -> StoreResult<NewEvent> {
    Ok(NewEvent {
        kind: EventKind::Decision,
        role: None,
        name: Some(name.to_string()),
        content: None,
        metadata: Some(RawValue::from_string(metadata.to_string())?),
    })
}

/// Open a gate. Returns the `seq` the agent waits on.
pub async fn request(
    store: &dyn TlrStore,
    ns: &str,
    run_id: &str,
    action: &str,
    expires_at: u64,
) -> StoreResult<u64> {
    let ev = decision_event(
        REQUESTED,
        json!({ "action": action, "expires_at": expires_at }),
    )?;
    Ok(store.append(ns, run_id, ev, None).await?.seq)
}

/// Record a human's decision on a gate.
///
/// `attested` says whether the server authenticated the reviewer. An
/// implementation must not set it from a caller-supplied name: an honest
/// weakness in the record is worth more than a claim the record cannot support.
#[allow(clippy::too_many_arguments)]
pub async fn resolve(
    store: &dyn TlrStore,
    ns: &str,
    run_id: &str,
    target_seq: u64,
    decision: Decision,
    reviewer: Option<&str>,
    reviewer_principal: Option<&str>,
    attested: bool,
    note: Option<&str>,
) -> StoreResult<u64> {
    let events = store.events(ns, run_id, 0, 5000).await?;
    let gate = project(&events)
        .into_iter()
        .find(|g| g.seq == target_seq)
        .ok_or(StoreError::NotFound)?;
    if gate.state != ApprovalState::Pending {
        return Err(StoreError::Conflict(format!(
            "gate {target_seq} is already resolved"
        )));
    }

    let ev = decision_event(
        RESOLVED,
        json!({
            "target_seq": target_seq,
            "decision": decision.as_str(),
            "reviewer": reviewer,
            "reviewer_principal": reviewer_principal,
            "attested": attested,
            "note": note,
        }),
    )?;
    Ok(store.append(ns, run_id, ev, None).await?.seq)
}

/// Resolve every gate past its expiry as `Expired`. Returns how many lapsed.
pub async fn expire_lapsed(
    store: &dyn TlrStore,
    ns: &str,
    run_id: &str,
    now_ms: u64,
) -> StoreResult<usize> {
    let events = store.events(ns, run_id, 0, 5000).await?;
    expire_lapsed_in(store, ns, run_id, &events, now_ms).await
}

/// As `expire_lapsed`, over events the caller has already read.
///
/// A read path that is about to report gate states can expire the lapsed ones
/// without paying for a second query, which is what makes doing this on read
/// cheap enough to be the only mechanism.
pub async fn expire_lapsed_in(
    store: &dyn TlrStore,
    ns: &str,
    run_id: &str,
    events: &[RunEvent],
    now_ms: u64,
) -> StoreResult<usize> {
    let lapsed: Vec<u64> = project(events)
        .into_iter()
        .filter(|g| g.state == ApprovalState::Pending && g.expires_at <= now_ms)
        .map(|g| g.seq)
        .collect();
    if lapsed.is_empty() {
        return Ok(0);
    }

    let mut n = 0;
    for seq in lapsed {
        resolve(
            store,
            ns,
            run_id,
            seq,
            Decision::Expired,
            None,
            None,
            false,
            Some("no decision before expiry"),
        )
        .await?;
        n += 1;
    }
    Ok(n)
}
