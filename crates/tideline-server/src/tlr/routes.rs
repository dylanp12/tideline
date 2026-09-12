//! The `/v1` HTTP binding for TLR/1 (`spec/tlr-1.md` §9).

use super::approvals::{self, Decision};
use super::auth::{resolve_read, resolve_write, Access, AuthMode};
use super::keys::SigningKeyring;
use super::metrics::{incr, metrics};
use super::sqlite::now_ms;
use super::store::*;
use crate::backend::Backend;
use crate::token::Signal;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;

/// Resolves a Cloud-issued key to the tenant namespace that owns it.
///
/// Behind a trait so the record routes never reach into the control-plane
/// client directly, and so tests can deny or grant without a network.
#[async_trait::async_trait]
pub trait TenantResolver: Send + Sync + 'static {
    async fn resolve(&self, token: &str) -> Option<String>;
}

/// No control plane configured: no token resolves to a tenant.
pub struct NoTenants;

#[async_trait::async_trait]
impl TenantResolver for NoTenants {
    async fn resolve(&self, _token: &str) -> Option<String> {
        None
    }
}

#[derive(Clone)]
pub struct TlrState {
    pub store: Arc<dyn TlrStore>,
    pub keys: Arc<SigningKeyring>,
    pub mode: AuthMode,
    pub tenants: Arc<dyn TenantResolver>,
    /// The streaming spine, used for the live tail of a record.
    pub backend: Arc<dyn Backend>,
    /// Write a checkpoint every N appends. A run is also checkpointed on seal.
    pub checkpoint_every: u64,
}

impl TlrState {
    pub fn new(
        store: Arc<dyn TlrStore>,
        keys: Arc<SigningKeyring>,
        mode: AuthMode,
        backend: Arc<dyn Backend>,
    ) -> Self {
        Self {
            store,
            keys,
            mode,
            tenants: Arc::new(NoTenants),
            backend,
            checkpoint_every: 50,
        }
    }

    pub fn with_tenants(mut self, tenants: Arc<dyn TenantResolver>) -> Self {
        self.tenants = tenants;
        self
    }
}

// --- errors ---------------------------------------------------------------

/// Map a storage failure to a status. `Backend` detail is logged, not returned:
/// a database message is for the operator, not the caller.
fn err(e: StoreError) -> Response {
    match e {
        StoreError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
        StoreError::Conflict(m) => (StatusCode::CONFLICT, m).into_response(),
        StoreError::Invalid(m) => (StatusCode::BAD_REQUEST, m).into_response(),
        StoreError::Backend(m) => {
            tracing::error!(error = %m, "record store failure");
            (StatusCode::INTERNAL_SERVER_ERROR, "storage failure").into_response()
        }
    }
}

fn bad(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, msg.to_string()).into_response()
}

/// Run ids are constrained so they are safe in a path and in a stream name.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|h| h.strip_prefix("Bearer ").unwrap_or(h).to_string())
}

/// Authorize and resolve the namespace. The namespace comes from the verified
/// credential, never from a request parameter (audit finding 1).
async fn authorize(st: &TlrState, headers: &HeaderMap, write: bool) -> Result<String, Response> {
    let presented = bearer(headers);
    let tenant = match &presented {
        Some(tok) if st.mode.cloud_verify => st.tenants.resolve(tok).await,
        _ => None,
    };
    let access = if write {
        match tenant {
            Some(ns) => Access::Granted { ns },
            None => resolve_write(&st.mode, presented.as_deref()),
        }
    } else {
        resolve_read(&st.mode, presented.as_deref(), tenant.as_deref())
    };
    match access {
        Access::Granted { ns } => Ok(ns),
        Access::Denied => Err((StatusCode::UNAUTHORIZED, "unauthorized").into_response()),
    }
}

// --- router ---------------------------------------------------------------

pub fn router(state: TlrState) -> Router {
    Router::new()
        .route("/v1/.well-known/tideline", get(well_known))
        .route("/v1/runs", post(create_run).get(list_runs))
        .route("/v1/runs/:id", get(get_run))
        .route("/v1/runs/:id/events", post(append_event).get(list_events))
        .route("/v1/runs/:id/watch", get(watch))
        .route("/v1/runs/:id/complete", post(complete_run))
        .route("/v1/runs/:id/checkpoint", get(get_checkpoint))
        .route(
            "/v1/runs/:id/approvals",
            post(create_approval).get(list_approvals),
        )
        .route("/v1/runs/:id/approvals/:seq", get(get_approval))
        .route(
            "/v1/runs/:id/approvals/:seq/resolve",
            post(resolve_approval),
        )
        .route("/v1/runs/:id/redactions", post(redact))
        .layer(axum::middleware::map_response(stamp_protocol))
        .with_state(state)
}

/// Every response says which protocol version produced it.
async fn stamp_protocol(mut res: Response) -> Response {
    res.headers_mut()
        .insert("tideline-protocol", HeaderValue::from_static("1"));
    res
}

// --- handlers -------------------------------------------------------------

async fn well_known(State(st): State<TlrState>) -> Response {
    Json(st.keys.well_known()).into_response()
}

async fn create_run(State(st): State<TlrState>, headers: HeaderMap, body: String) -> Response {
    let ns = match authorize(&st, &headers, true).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    let new: NewRun = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return bad(&format!("invalid run: {e}")),
    };
    if !valid_id(&new.run_id) {
        return bad("invalid run id");
    }
    match st.store.create_run(&ns, new).await {
        Ok(env) => {
            incr(&metrics().runs_created);
            publish_head(&st, &ns, &env.run_id, 0).await;
            Json(env).into_response()
        }
        Err(e) => err(e),
    }
}

async fn list_runs(
    State(st): State<TlrState>,
    Query(q): Query<RunQuery>,
    headers: HeaderMap,
) -> Response {
    let ns = match authorize(&st, &headers, false).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    match st.store.list_runs(&ns, q).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => err(e),
    }
}

async fn get_run(
    State(st): State<TlrState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let ns = match authorize(&st, &headers, false).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    match st.store.get_run(&ns, &id).await {
        Ok(env) => Json(env).into_response(),
        Err(e) => err(e),
    }
}

async fn append_event(
    State(st): State<TlrState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let ns = match authorize(&st, &headers, true).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    let event: NewEvent = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return bad(&format!("invalid event: {e}")),
    };
    let idem = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    match st.store.append(&ns, &id, event, idem.as_deref()).await {
        Ok(res) => {
            incr(&metrics().events_appended);
            publish_head(&st, &ns, &id, res.seq).await;
            maybe_checkpoint(&st, &ns, &id, res.seq, res.hash).await;
            Json(res).into_response()
        }
        Err(e) => {
            incr(&metrics().append_failures);
            err(e)
        }
    }
}

#[derive(Deserialize)]
struct EventsQuery {
    #[serde(default)]
    from: Option<u64>,
    #[serde(default)]
    limit: Option<u32>,
}

async fn list_events(
    State(st): State<TlrState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
    headers: HeaderMap,
) -> Response {
    let ns = match authorize(&st, &headers, false).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    match st
        .store
        .events(&ns, &id, q.from.unwrap_or(0), q.limit.unwrap_or(1000))
        .await
    {
        Ok(events) => Json(events).into_response(),
        Err(e) => err(e),
    }
}

/// Live tail of a record.
///
/// History comes from the store and the live tail from the streaming spine.
/// Subscribing from `head + 1` rather than "now" closes the gap between the two
/// reads: anything appended in between is still in the stream's replay buffer.
async fn watch(
    State(st): State<TlrState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
    headers: HeaderMap,
) -> Response {
    let ns = match authorize(&st, &headers, false).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    let from = q.from.unwrap_or(0);
    let history = match st.store.events(&ns, &id, from, 5000).await {
        Ok(e) => e,
        Err(e) => return err(e),
    };
    let next = history.last().map(|e| e.seq + 1).unwrap_or(from);
    let live = st.backend.subscribe(&stream_name(&ns, &id), next).await;

    let stream = async_stream::stream! {
        for e in history {
            let seq = e.seq;
            if let Ok(json) = serde_json::to_string(&e) {
                yield Ok::<Event, Infallible>(Event::default().id(seq.to_string()).data(json));
            }
        }
        let mut live = live;
        while let Some(sig) = live.next().await {
            match sig {
                Signal::Token(t) => {
                    yield Ok(Event::default().id(t.offset.to_string()).data(t.data));
                }
                Signal::Gap => yield Ok(Event::default().event("gap").data("")),
                Signal::Error(m) => {
                    yield Ok(Event::default().event("stream_error").data(m));
                    break;
                }
                Signal::Done => {
                    yield Ok(Event::default().event("done").data(""));
                    break;
                }
            }
        }
    };
    Sse::new(stream).into_response()
}

async fn complete_run(
    State(st): State<TlrState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let ns = match authorize(&st, &headers, true).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    match st.store.seal(&ns, &id).await {
        Ok(res) => {
            incr(&metrics().runs_sealed);
            publish_head(&st, &ns, &id, res.seq).await;
            write_checkpoint(&st, &ns, &id, res.seq, res.hash).await;
            st.backend.complete(&stream_name(&ns, &id)).await;
            Json(res).into_response()
        }
        Err(e) => err(e),
    }
}

async fn get_checkpoint(
    State(st): State<TlrState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let ns = match authorize(&st, &headers, false).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    match st.store.latest_checkpoint(&ns, &id).await {
        Ok(Some(cp)) => Json(cp).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no checkpoint yet").into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
struct CreateApproval {
    action: String,
    /// Absolute expiry in milliseconds since the epoch.
    #[serde(default)]
    expires_at: Option<u64>,
    /// Relative expiry in seconds, as a convenience for clients.
    #[serde(default)]
    expires_in: Option<u64>,
}

async fn create_approval(
    State(st): State<TlrState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let ns = match authorize(&st, &headers, true).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    let req: CreateApproval = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return bad("expected {\"action\": ...}"),
    };
    // Default: a day. A gate with no expiry blocks an agent forever and leaves
    // a record that stops without saying why.
    let expires_at = req
        .expires_at
        .or_else(|| req.expires_in.map(|s| now_ms() + s * 1000))
        .unwrap_or_else(|| now_ms() + 86_400_000);

    match approvals::request(st.store.as_ref(), &ns, &id, &req.action, expires_at).await {
        Ok(seq) => {
            incr(&metrics().gates_opened);
            publish_head(&st, &ns, &id, seq).await;
            Json(json!({ "seq": seq, "expires_at": expires_at })).into_response()
        }
        Err(e) => err(e),
    }
}

async fn list_approvals(
    State(st): State<TlrState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let ns = match authorize(&st, &headers, false).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    match st.store.events(&ns, &id, 0, 5000).await {
        Ok(events) => {
            let pending: Vec<_> = approvals::project(&events)
                .into_iter()
                .filter(|g| g.state == approvals::ApprovalState::Pending)
                .collect();
            Json(pending).into_response()
        }
        Err(e) => err(e),
    }
}

async fn get_approval(
    State(st): State<TlrState>,
    Path((id, seq)): Path<(String, u64)>,
    headers: HeaderMap,
) -> Response {
    let ns = match authorize(&st, &headers, false).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    match st.store.events(&ns, &id, 0, 5000).await {
        Ok(events) => match approvals::project(&events)
            .into_iter()
            .find(|g| g.seq == seq)
        {
            Some(g) => Json(g).into_response(),
            None => (StatusCode::NOT_FOUND, "no such approval").into_response(),
        },
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
struct ResolveApproval {
    decision: String,
    #[serde(default)]
    reviewer: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

async fn resolve_approval(
    State(st): State<TlrState>,
    Path((id, seq)): Path<(String, u64)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let ns = match authorize(&st, &headers, true).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    let req: ResolveApproval = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return bad("invalid resolve body"),
    };
    let Some(decision) = Decision::parse(&req.decision) else {
        return bad("decision must be 'approved' or 'rejected'");
    };
    if decision == Decision::Expired {
        return bad("'expired' is written by the server, not a reviewer");
    }

    // The authenticated principal, where the server has one. `attested` is
    // never set from the caller-supplied `reviewer` name.
    let principal = match (st.mode.cloud_verify, bearer(&headers)) {
        (true, Some(tok)) => st
            .tenants
            .resolve(&tok)
            .await
            .map(|ns| format!("tenant:{ns}")),
        _ => None,
    };
    let attested = principal.is_some();

    match approvals::resolve(
        st.store.as_ref(),
        &ns,
        &id,
        seq,
        decision,
        req.reviewer.as_deref(),
        principal.as_deref(),
        attested,
        req.note.as_deref(),
    )
    .await
    {
        Ok(written) => {
            incr(match decision {
                Decision::Approved => &metrics().gates_approved,
                Decision::Rejected => &metrics().gates_rejected,
                Decision::Expired => &metrics().gates_expired,
            });
            publish_head(&st, &ns, &id, written).await;
            Json(json!({
                "seq": written,
                "target_seq": seq,
                "decision": decision.as_str(),
                "attested": attested,
            }))
            .into_response()
        }
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
struct RedactBody {
    target_seq: u64,
    fields: Vec<String>,
    authority: String,
}

async fn redact(
    State(st): State<TlrState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let ns = match authorize(&st, &headers, true).await {
        Ok(ns) => ns,
        Err(r) => return r,
    };
    if !valid_id(&id) {
        return bad("invalid run id");
    }
    let req: RedactBody = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return bad("expected {target_seq, fields, authority}"),
    };
    if req.authority.trim().is_empty() {
        // An erasure with no stated authority is not auditable.
        return bad("authority is required");
    }
    match st
        .store
        .redact(&ns, &id, req.target_seq, &req.fields, &req.authority)
        .await
    {
        Ok(res) => {
            incr(&metrics().redactions);
            publish_head(&st, &ns, &id, res.seq).await;
            Json(res).into_response()
        }
        Err(e) => err(e),
    }
}

// --- live tail plumbing ---------------------------------------------------

fn stream_name(ns: &str, run_id: &str) -> String {
    if ns.is_empty() {
        format!("tlr.{run_id}")
    } else {
        format!("tlr.{ns}.{run_id}")
    }
}

/// Publish the newly written event to the stream spine so watchers see it live.
///
/// Best-effort: a watcher missing a frame is a degraded view, while a failed
/// append would be a missing record. The store is the source of truth.
async fn publish_head(st: &TlrState, ns: &str, run_id: &str, seq: u64) {
    let Ok(events) = st.store.events(ns, run_id, seq, 1).await else {
        return;
    };
    let Some(event) = events.first() else { return };
    if let Ok(json) = serde_json::to_string(event) {
        st.backend.publish(&stream_name(ns, run_id), json).await;
    }
}

async fn maybe_checkpoint(
    st: &TlrState,
    ns: &str,
    run_id: &str,
    seq: u64,
    head: tideline_proto::Hash,
) {
    if st.checkpoint_every > 0 && seq % st.checkpoint_every == 0 {
        write_checkpoint(st, ns, run_id, seq, head).await;
    }
}

async fn write_checkpoint(
    st: &TlrState,
    ns: &str,
    run_id: &str,
    seq: u64,
    head: tideline_proto::Hash,
) {
    let cp = st.keys.sign_head(run_id, seq, head, now_ms());
    match st.store.put_checkpoint(ns, &cp).await {
        Ok(()) => {
            incr(&metrics().checkpoints_written);
            metrics()
                .last_checkpoint_ms
                .store(cp.ts, std::sync::atomic::Ordering::Relaxed);
        }
        Err(e) => tracing::warn!(error = %e, run_id, seq, "failed to write checkpoint"),
    }
}
