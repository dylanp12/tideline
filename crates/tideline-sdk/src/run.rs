//! A handle to one run.

use crate::client::Tideline;
use crate::error::{Error, Result};
use crate::event::NewEvent;
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tideline_proto::{Checkpoint, Hash, RunEnvelope, RunEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approved,
    Rejected,
    /// Nobody decided before the gate expired.
    Expired,
}

impl Decision {
    pub fn is_approved(&self) -> bool {
        *self == Decision::Approved
    }
}

/// One approval gate, as the server projects it from the chain.
#[derive(Clone, Debug, Deserialize)]
pub struct Gate {
    pub seq: u64,
    pub action: String,
    #[serde(default)]
    pub expires_at: u64,
    pub state: String,
    #[serde(default)]
    pub decision: Option<Decision>,
    #[serde(default)]
    pub reviewer: Option<String>,
    #[serde(default)]
    pub reviewer_principal: Option<String>,
    #[serde(default)]
    pub attested: bool,
    #[serde(default)]
    pub note: Option<String>,
}

impl Gate {
    pub fn is_pending(&self) -> bool {
        self.state == "pending"
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Appended {
    pub seq: u64,
    pub ts: u64,
    pub prev_hash: Hash,
    pub hash: Hash,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OpenedGate {
    pub seq: u64,
    pub expires_at: u64,
}

/// The outcome of a gate, once a human has decided.
#[derive(Clone, Debug)]
pub struct Resolution {
    pub seq: u64,
    pub decision: Decision,
    pub reviewer: Option<String>,
    pub note: Option<String>,
    /// Whether the server authenticated the reviewer, as opposed to taking
    /// their word for who they are.
    pub attested: bool,
}

impl Resolution {
    pub fn is_approved(&self) -> bool {
        self.decision.is_approved()
    }
}

pub struct Run {
    client: Tideline,
    run_id: String,
}

impl Run {
    pub(crate) fn new(client: Tideline, run_id: String) -> Self {
        Run { client, run_id }
    }

    pub fn id(&self) -> &str {
        &self.run_id
    }

    fn path(&self, suffix: &str) -> String {
        format!("/v1/runs/{}{}", self.run_id, suffix)
    }

    /// The run's envelope, head sequence, and head hash.
    pub async fn envelope(&self) -> Result<RunEnvelope> {
        self.client
            .send(self.client.req(reqwest::Method::GET, &self.path("")))
            .await
    }

    /// Append an event.
    pub async fn record(&self, event: NewEvent) -> Result<Appended> {
        self.record_with_key(event, None).await
    }

    /// Append with an idempotency key, so a retry after a timeout does not put a
    /// duplicate into evidence.
    pub async fn record_idempotent(&self, event: NewEvent, key: &str) -> Result<Appended> {
        self.record_with_key(event, Some(key)).await
    }

    async fn record_with_key(&self, event: NewEvent, key: Option<&str>) -> Result<Appended> {
        let mut rb = self
            .client
            .req(reqwest::Method::POST, &self.path("/events"))
            .json(&event);
        if let Some(k) = key {
            rb = rb.header("idempotency-key", k);
        }
        self.client.send(rb).await
    }

    /// Open a gate without waiting on it.
    pub async fn open_gate(&self, action: &str, expires_in: Duration) -> Result<OpenedGate> {
        let rb = self
            .client
            .req(reqwest::Method::POST, &self.path("/approvals"))
            .json(&json!({ "action": action, "expires_in": expires_in.as_secs() }));
        self.client.send(rb).await
    }

    /// Open a gate and block until a human decides, or it expires.
    ///
    /// This is the call that puts oversight in the agent's path rather than
    /// beside it: when it returns, the request and the decision are both
    /// already in the chain.
    ///
    /// It polls rather than holding a stream open. A gate can sit for hours, and
    /// a poll survives a proxy idle timeout, a NAT rebind, and a reconnect,
    /// where a long-lived SSE connection quietly does not. `watch` is the right
    /// tool for a live oversight view; this is the right tool for a gate.
    pub async fn gate(&self, action: &str, expires_in: Duration) -> Result<Resolution> {
        let opened = self.open_gate(action, expires_in).await?;
        self.await_gate(opened.seq).await
    }

    /// Block until gate `seq` is decided.
    pub async fn await_gate(&self, seq: u64) -> Result<Resolution> {
        let mut delay = Duration::from_millis(250);
        loop {
            let gate = self.approval(seq).await?;
            if !gate.is_pending() {
                let decision = gate.decision.ok_or_else(|| {
                    Error::Protocol("resolved gate carries no decision".to_string())
                })?;
                return Ok(Resolution {
                    seq,
                    decision,
                    reviewer: gate.reviewer,
                    note: gate.note,
                    attested: gate.attested,
                });
            }
            tokio::time::sleep(delay).await;
            // Back off to five seconds: a human is not going to answer faster
            // than that, and a tight loop on a multi-hour gate is rude.
            delay = (delay * 2).min(Duration::from_secs(5));
        }
    }

    /// Gates still awaiting a decision — the reviewer's queue.
    pub async fn approvals(&self) -> Result<Vec<Gate>> {
        self.client
            .send(
                self.client
                    .req(reqwest::Method::GET, &self.path("/approvals")),
            )
            .await
    }

    pub async fn approval(&self, seq: u64) -> Result<Gate> {
        self.client
            .send(self.client.req(
                reqwest::Method::GET,
                &self.path(&format!("/approvals/{seq}")),
            ))
            .await
    }

    /// Record a human's decision on a gate.
    pub async fn resolve(
        &self,
        seq: u64,
        decision: Decision,
        reviewer: Option<&str>,
        note: Option<&str>,
    ) -> Result<()> {
        let rb = self
            .client
            .req(
                reqwest::Method::POST,
                &self.path(&format!("/approvals/{seq}/resolve")),
            )
            .json(&json!({
                "decision": match decision {
                    Decision::Approved => "approved",
                    Decision::Rejected => "rejected",
                    Decision::Expired => "expired",
                },
                "reviewer": reviewer,
                "note": note,
            }));
        let _: serde_json::Value = self.client.send(rb).await?;
        Ok(())
    }

    /// The whole record, from the beginning.
    pub async fn events(&self) -> Result<Vec<RunEvent>> {
        self.events_from(0, 5000).await
    }

    pub async fn events_from(&self, from: u64, limit: u32) -> Result<Vec<RunEvent>> {
        self.client
            .send(self.client.req(
                reqwest::Method::GET,
                &self.path(&format!("/events?from={from}&limit={limit}")),
            ))
            .await
    }

    /// Fetch the record and verify it in one step.
    pub async fn verified_events(&self) -> Result<Vec<RunEvent>> {
        let events = self.events().await?;
        tideline_proto::verify_chain(&events)?;
        Ok(events)
    }

    /// Follow the record live. History first, then new events as they land.
    pub async fn watch(&self, from: u64) -> Result<impl Stream<Item = Result<RunEvent>>> {
        let res = self
            .client
            .req(
                reqwest::Method::GET,
                &self.path(&format!("/watch?from={from}")),
            )
            .send()
            .await?;
        let code = res.status().as_u16();
        if !(200..300).contains(&code) {
            return Err(Error::Status {
                code,
                body: res.text().await.unwrap_or_default(),
            });
        }
        Ok(sse_events(res.bytes_stream()))
    }

    pub async fn complete(&self) -> Result<Appended> {
        self.client
            .send(
                self.client
                    .req(reqwest::Method::POST, &self.path("/complete")),
            )
            .await
    }

    /// The latest signed checkpoint, or `None` if the server has issued none.
    pub async fn checkpoint(&self) -> Result<Option<Checkpoint>> {
        match self
            .client
            .send::<Checkpoint>(
                self.client
                    .req(reqwest::Method::GET, &self.path("/checkpoint")),
            )
            .await
        {
            Ok(cp) => Ok(Some(cp)),
            Err(e) if e.is_not_found() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Erase fields of an earlier event, keeping the chain intact.
    pub async fn redact(
        &self,
        target_seq: u64,
        fields: &[&str],
        authority: &str,
    ) -> Result<Appended> {
        let rb = self
            .client
            .req(reqwest::Method::POST, &self.path("/redactions"))
            .json(&json!({
                "target_seq": target_seq,
                "fields": fields,
                "authority": authority,
            }));
        self.client.send(rb).await
    }
}

/// Decode an SSE byte stream into events.
///
/// Small enough to own: the wire format here is `data:` lines terminated by a
/// blank line, and a dependency would cost more than it saves.
fn sse_events(
    bytes: impl Stream<Item = reqwest::Result<bytes::Bytes>>,
) -> impl Stream<Item = Result<RunEvent>> {
    let mut buf = String::new();
    Box::pin(bytes).flat_map(move |chunk| {
        let mut out: Vec<Result<RunEvent>> = Vec::new();
        match chunk {
            Err(e) => out.push(Err(Error::Transport(e))),
            Ok(b) => {
                buf.push_str(&String::from_utf8_lossy(&b));
                while let Some(end) = buf.find("\n\n") {
                    let frame = buf[..end].to_string();
                    buf.drain(..end + 2);
                    let mut data = String::new();
                    let mut terminal = false;
                    for line in frame.lines() {
                        if let Some(rest) = line.strip_prefix("data:") {
                            data.push_str(rest.trim_start());
                        } else if let Some(name) = line.strip_prefix("event:") {
                            terminal = matches!(name.trim(), "done" | "stream_error");
                        }
                    }
                    if terminal || data.is_empty() {
                        continue;
                    }
                    // From the frame text, so the metadata bytes survive.
                    match serde_json::from_str::<RunEvent>(&data) {
                        Ok(e) => out.push(Ok(e)),
                        Err(e) => out.push(Err(Error::Protocol(format!("bad event frame: {e}")))),
                    }
                }
            }
        }
        futures::stream::iter(out)
    })
}
