use crate::backend::{Backend, MemoryBackend};
use crate::events::{Events, Tenant};
use crate::manager::StreamManager;
use crate::token::Signal;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use base64::Engine as _;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower_http::cors::CorsLayer;

/// Max request body for a single publish (one token chunk).
const MAX_BODY: usize = 256 * 1024;

/// Access control. A static `publish_token` is the simple/admin path; `verify_url`
/// (the Tideline Cloud control plane) validates per-tenant API keys. `None`
/// everywhere = open (dev).
#[derive(Clone, Default)]
pub struct AuthConfig {
    pub publish_token: Option<String>,
    pub subscribe_token: Option<String>,
    /// Control-plane endpoint that validates Cloud-issued keys (TIDELINE_AUTH_URL).
    pub verify_url: Option<String>,
    /// Shared secret sent to the verify endpoint (TIDELINE_AUTH_SECRET).
    pub verify_secret: Option<String>,
}

impl AuthConfig {
    pub fn from_env() -> Self {
        let norm = |v: Result<String, std::env::VarError>| v.ok().filter(|s| !s.is_empty());
        Self {
            publish_token: norm(std::env::var("TIDELINE_PUBLISH_TOKEN")),
            subscribe_token: norm(std::env::var("TIDELINE_SUBSCRIBE_TOKEN")),
            verify_url: norm(std::env::var("TIDELINE_AUTH_URL")),
            verify_secret: norm(std::env::var("TIDELINE_AUTH_SECRET")),
        }
    }
}

#[derive(serde::Deserialize)]
struct VerifyResp {
    #[serde(rename = "userId")]
    user_id: Option<String>,
    #[serde(rename = "orgId")]
    org_id: Option<String>,
}

/// Validates Cloud-issued API keys against the control plane, keeping the owning
/// tenant and caching positive results so it's not a round-trip per request.
struct KeyVerifier {
    client: reqwest::Client,
    cache: Mutex<HashMap<String, (Tenant, Instant)>>,
    ttl: Duration,
}

impl KeyVerifier {
    fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            cache: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(60),
        }
    }

    async fn verify(&self, url: &str, secret: &Option<String>, token: &str) -> Option<Tenant> {
        if let Some((tenant, exp)) = self.cache.lock().unwrap().get(token) {
            if *exp > Instant::now() {
                return Some(tenant.clone());
            }
        }
        let mut req = self.client.post(url).json(&json!({ "key": token }));
        if let Some(s) = secret {
            req = req.header("x-tideline-verify-secret", s);
        }
        match req.send().await {
            Ok(r) if r.status().is_success() => {
                let tenant = match r.json::<VerifyResp>().await {
                    Ok(v) => Tenant {
                        user_id: v.user_id.unwrap_or_default(),
                        org_id: v.org_id,
                    },
                    Err(_) => Tenant {
                        user_id: String::new(),
                        org_id: None,
                    },
                };
                self.cache.lock().unwrap().insert(
                    token.to_string(),
                    (tenant.clone(), Instant::now() + self.ttl),
                );
                Some(tenant)
            }
            _ => None,
        }
    }
}

/// Result of authorizing a write: denied, or allowed with the optional owning tenant.
enum Authz {
    Denied,
    Allowed(Option<Tenant>),
}

#[derive(Clone)]
pub struct AppState {
    backend: Arc<dyn Backend>,
    auth: Arc<AuthConfig>,
    verifier: Arc<KeyVerifier>,
    events: Arc<Events>,
}

pub fn router() -> Router {
    router_with(StreamManager::new())
}

/// Build a router over an in-memory backend (default).
pub fn router_with(manager: StreamManager) -> Router {
    router_with_config(manager, AuthConfig::default())
}

pub fn router_with_config(manager: StreamManager, auth: AuthConfig) -> Router {
    router_with_backend(Arc::new(MemoryBackend::new(manager)), auth)
}

/// Build a router over any backend (in-memory or Redis). Event reporting is
/// configured from the environment (TIDELINE_EVENTS_URL + TIDELINE_AUTH_SECRET);
/// unset = no reporting.
pub fn router_with_backend(backend: Arc<dyn Backend>, auth: AuthConfig) -> Router {
    let norm = |v: Result<String, std::env::VarError>| v.ok().filter(|s| !s.is_empty());
    let events = Arc::new(Events::new(
        norm(std::env::var("TIDELINE_EVENTS_URL")),
        norm(std::env::var("TIDELINE_AUTH_SECRET")),
    ));
    router_with_events(backend, auth, events)
}

/// Build a router with an explicit event reporter (used in tests).
pub fn router_with_events(
    backend: Arc<dyn Backend>,
    auth: AuthConfig,
    events: Arc<Events>,
) -> Router {
    router_full(backend, auth, events)
}

/// The fully-parameterized builder: streaming backend, auth, and event reporter.
fn router_full(backend: Arc<dyn Backend>, auth: AuthConfig, events: Arc<Events>) -> Router {
    let state = AppState {
        backend,
        auth: Arc::new(auth),
        verifier: Arc::new(KeyVerifier::new()),
        events,
    };
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/metrics", get(metrics))
        .route(
            "/streams/:id",
            post(publish).get(subscribe).delete(delete_stream),
        )
        .route("/streams/:id/complete", post(complete))
        .route("/streams/:id/error", post(fail))
        .route("/streams/:id/ws", get(subscribe_ws))
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(CorsLayer::permissive())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// Namespace a stream id by tenant for Cloud isolation. A `None`/empty namespace
/// (self-host, or a static-token publish) leaves the id untouched. The separator
/// `|` can't appear in a valid stream id, so the mapping is unambiguous, and a key
/// can only ever address its own tenant's namespace.
fn ns_key(namespace: Option<&str>, id: &str) -> String {
    match namespace {
        Some(ns) if !ns.is_empty() => format!("{ns}|{id}"),
        _ => id.to_string(),
    }
}

/// Verify a signed subscribe ticket: `base64url(payload).base64url(hmac_sha256)`,
/// where payload = `{"ns","sid","exp"}`. Returns (namespace, stream id) if the
/// signature is valid and unexpired. The signing secret is shared with the
/// control plane, which mints tickets on a publisher's behalf.
fn verify_ticket(secret: &str, ticket: &str) -> Option<(String, String)> {
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let (payload_b64, sig_b64) = ticket.split_once('.')?;
    let sig = b64.decode(sig_b64).ok()?;
    let mut mac = <Hmac<Sha256>>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload_b64.as_bytes());
    mac.verify_slice(&sig).ok()?; // constant-time
    let payload = b64.decode(payload_b64).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    let ns = v.get("ns")?.as_str()?.to_string();
    let sid = v.get("sid")?.as_str()?.to_string();
    let exp = v.get("exp")?.as_u64()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if now >= exp {
        return None;
    }
    Some((ns, sid))
}

fn authed(token: &Option<String>, headers: &HeaderMap) -> bool {
    match token {
        None => true,
        Some(want) => headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|h| h.strip_prefix("Bearer ").unwrap_or(h) == want)
            .unwrap_or(false),
    }
}

/// Extract the bearer token from the Authorization header.
fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|h| h.strip_prefix("Bearer ").unwrap_or(h).to_string())
}

/// Authorize a write: accept the static publish token, or a Cloud-issued key
/// verified against the control plane (keeping its tenant). Open only when
/// neither a token nor a verify URL is configured.
async fn authorize_write(st: &AppState, headers: &HeaderMap) -> Authz {
    let presented = bearer(headers);
    if let Some(want) = &st.auth.publish_token {
        if presented.as_deref() == Some(want.as_str()) {
            return Authz::Allowed(None);
        }
    }
    if let Some(url) = &st.auth.verify_url {
        if let Some(tok) = &presented {
            if let Some(tenant) = st.verifier.verify(url, &st.auth.verify_secret, tok).await {
                return Authz::Allowed(Some(tenant));
            }
        }
    }
    if st.auth.publish_token.is_none() && st.auth.verify_url.is_none() {
        return Authz::Allowed(None);
    }
    Authz::Denied
}

#[derive(serde::Deserialize)]
struct PublishParams {
    /// `?private=1` marks the stream private — reads then require a signed ticket.
    private: Option<String>,
}

async fn publish(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<PublishParams>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if !valid_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid stream id").into_response();
    }
    let tenant = match authorize_write(&st, &headers).await {
        Authz::Denied => return (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        Authz::Allowed(t) => t,
    };
    let ns = tenant.as_ref().map(|t| t.namespace());
    let key = ns_key(ns.as_deref(), &id);
    if matches!(params.private.as_deref(), Some("1") | Some("true")) {
        st.backend.set_private(&key).await;
    }
    let offset = st.backend.publish(&key, body).await;
    if let Some(t) = &tenant {
        if st.events.owner(&key).is_none() {
            st.events.set_owner(&key, t, &id);
            st.events.created(t, &id);
        }
        st.events.tokens(t, &id, 1, offset);
    }
    (StatusCode::OK, offset.to_string()).into_response()
}

async fn complete(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !valid_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid stream id").into_response();
    }
    let tenant = match authorize_write(&st, &headers).await {
        Authz::Denied => return (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        Authz::Allowed(t) => t,
    };
    let key = ns_key(tenant.as_ref().map(|t| t.namespace()).as_deref(), &id);
    st.backend.complete(&key).await;
    st.events.completed(&key);
    (StatusCode::OK, "ok").into_response()
}

async fn fail(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if !valid_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid stream id").into_response();
    }
    let tenant = match authorize_write(&st, &headers).await {
        Authz::Denied => return (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        Authz::Allowed(t) => t,
    };
    let key = ns_key(tenant.as_ref().map(|t| t.namespace()).as_deref(), &id);
    let msg = if body.is_empty() {
        "error".to_string()
    } else {
        body
    };
    st.backend.fail(&key, msg.clone()).await;
    st.events.errored(&key, &msg);
    (StatusCode::OK, "ok").into_response()
}

async fn delete_stream(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !valid_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid stream id").into_response();
    }
    let tenant = match authorize_write(&st, &headers).await {
        Authz::Denied => return (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        Authz::Allowed(t) => t,
    };
    let key = ns_key(tenant.as_ref().map(|t| t.namespace()).as_deref(), &id);
    if st.backend.delete(&key).await {
        (StatusCode::OK, "deleted").into_response()
    } else {
        (StatusCode::NOT_FOUND, "no such stream").into_response()
    }
}

#[derive(serde::Deserialize)]
struct SubParams {
    from: Option<u64>,
    /// Tenant namespace (Cloud). Non-secret; the publisher's account id.
    ns: Option<String>,
    /// Signed subscribe ticket (required to read a private stream).
    ticket: Option<String>,
}

/// Emits `subscriber.disconnected` when an SSE/WS subscription ends or is dropped.
struct SubGuard {
    events: Arc<Events>,
    id: String,
}
impl Drop for SubGuard {
    fn drop(&mut self) {
        self.events.sub_disconnected(&self.id);
    }
}

async fn subscribe(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<SubParams>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !valid_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid stream id").into_response();
    }
    if !authed(&st.auth.subscribe_token, &headers) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let key = match params.ticket.as_deref() {
        Some(tk) => match st
            .auth
            .verify_secret
            .as_deref()
            .and_then(|s| verify_ticket(s, tk))
        {
            Some((ns, sid)) => ns_key(Some(&ns), &sid),
            None => return (StatusCode::UNAUTHORIZED, "invalid or expired ticket").into_response(),
        },
        None => {
            let k = ns_key(params.ns.as_deref(), &id);
            if st.backend.is_private(&k).await {
                return (
                    StatusCode::UNAUTHORIZED,
                    "private stream — a signed ticket is required",
                )
                    .into_response();
            }
            k
        }
    };
    if st.backend.subscriber_count(&key).await >= st.backend.max_subscribers_per_stream() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "stream subscriber limit reached",
        )
            .into_response();
    }
    // Resume point: Last-Event-ID header (browser auto-reconnect) -> +1, else ?from=.
    let from = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(|last| last + 1)
        .or(params.from)
        .unwrap_or(0);

    st.events.sub_connected(&key, from > 0, from, "sse");
    let guard = SubGuard {
        events: st.events.clone(),
        id: key.clone(),
    };
    let backend = st.backend.clone();
    let events = st.events.clone();
    let gap_id = key.clone();
    let sse = st
        .backend
        .subscribe(&key, from)
        .await
        .map(move |sig| -> Result<Event, Infallible> {
            let _keep = &guard; // owned by this closure; fires sub_disconnected when the stream drops
            match sig {
                Signal::Token(t) => Ok(Event::default().id(t.offset.to_string()).data(t.data)),
                Signal::Gap => {
                    backend.incr_gaps();
                    events.gap(&gap_id, from);
                    Ok(Event::default().event("gap").data(""))
                }
                // Named "stream_error" (not "error") so it doesn't collide with
                // EventSource's built-in connection-error event on the client.
                Signal::Error(e) => Ok(Event::default().event("stream_error").data(e)),
                Signal::Done => Ok(Event::default().event("done").data("")),
            }
        });
    Sse::new(sse).into_response()
}

#[derive(serde::Deserialize)]
struct WsParams {
    from: Option<u64>,
    token: Option<String>,
    ns: Option<String>,
    ticket: Option<String>,
}

/// WebSocket transport — identical stream semantics to SSE, over one
/// bidirectional socket (binary-capable, friendly to non-browser clients).
/// Resume is via `?from=`; the subscribe token (if set) via `?token=`, since
/// browsers can't set headers when opening a WebSocket.
async fn subscribe_ws(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<WsParams>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if !valid_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid stream id").into_response();
    }
    if let Some(want) = st.auth.subscribe_token.as_deref() {
        if params.token.as_deref() != Some(want) {
            return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    }
    let key = match params.ticket.as_deref() {
        Some(tk) => match st
            .auth
            .verify_secret
            .as_deref()
            .and_then(|s| verify_ticket(s, tk))
        {
            Some((ns, sid)) => ns_key(Some(&ns), &sid),
            None => return (StatusCode::UNAUTHORIZED, "invalid or expired ticket").into_response(),
        },
        None => {
            let k = ns_key(params.ns.as_deref(), &id);
            if st.backend.is_private(&k).await {
                return (
                    StatusCode::UNAUTHORIZED,
                    "private stream — a signed ticket is required",
                )
                    .into_response();
            }
            k
        }
    };
    if st.backend.subscriber_count(&key).await >= st.backend.max_subscribers_per_stream() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "stream subscriber limit reached",
        )
            .into_response();
    }
    let from = params.from.unwrap_or(0);
    let backend = st.backend.clone();
    let events = st.events.clone();
    events.sub_connected(&key, from > 0, from, "ws");
    ws.on_upgrade(move |socket| pump_ws(socket, backend, events, key, from))
}

/// Encode a signal as a JSON WS frame; the bool marks a terminal frame.
fn encode_ws(sig: &Signal) -> (String, bool) {
    match sig {
        Signal::Token(t) => (
            json!({ "type": "token", "offset": t.offset, "data": t.data }).to_string(),
            false,
        ),
        Signal::Gap => (json!({ "type": "gap" }).to_string(), false),
        Signal::Error(e) => (json!({ "type": "error", "message": e }).to_string(), true),
        Signal::Done => (json!({ "type": "done" }).to_string(), true),
    }
}

async fn pump_ws(
    mut socket: WebSocket,
    backend: Arc<dyn Backend>,
    events: Arc<Events>,
    id: String,
    from: u64,
) {
    let _guard = SubGuard {
        events: events.clone(),
        id: id.clone(),
    };
    let mut stream = backend.subscribe(&id, from).await;
    loop {
        tokio::select! {
            sig = stream.next() => match sig {
                Some(sig) => {
                    if matches!(sig, Signal::Gap) {
                        events.gap(&id, from);
                    }
                    let (text, terminal) = encode_ws(&sig);
                    if socket.send(Message::Text(text)).await.is_err() {
                        return; // client disconnected
                    }
                    if terminal {
                        break;
                    }
                }
                None => break,
            },
            // Drives ping/pong and notices the client closing the socket.
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => {} // ignore client->server frames in v1
            },
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}

async fn metrics(State(st): State<AppState>) -> String {
    let m = st.backend.metrics().await;
    format!(
        "# HELP tideline_active_streams Currently retained streams\n\
         # TYPE tideline_active_streams gauge\n\
         tideline_active_streams {}\n\
         # HELP tideline_active_subscribers Currently connected subscribers\n\
         # TYPE tideline_active_subscribers gauge\n\
         tideline_active_subscribers {}\n\
         # HELP tideline_buffered_tokens Retained tokens across all streams\n\
         # TYPE tideline_buffered_tokens gauge\n\
         tideline_buffered_tokens {}\n\
         # HELP tideline_streams_created_total Streams created since start\n\
         # TYPE tideline_streams_created_total counter\n\
         tideline_streams_created_total {}\n\
         # HELP tideline_tokens_published_total Tokens published since start\n\
         # TYPE tideline_tokens_published_total counter\n\
         tideline_tokens_published_total {}\n\
         # HELP tideline_gaps_served_total Gap signals served since start\n\
         # TYPE tideline_gaps_served_total counter\n\
         tideline_gaps_served_total {}\n\
         # HELP tideline_streams_reaped_total Streams evicted/reaped since start\n\
         # TYPE tideline_streams_reaped_total counter\n\
         tideline_streams_reaped_total {}\n",
        m.active_streams,
        m.active_subscribers,
        m.buffered_tokens,
        m.streams_created,
        m.tokens_published,
        m.gaps_served,
        m.reaped,
    )
}
