//! Tenant-attributed stream events, reported to the Tideline Cloud control plane.
//!
//! The engine is the only place that sees streams, so it's the only place that can
//! attribute reliability events (created, tokens, subscriber connect/disconnect/
//! resume, gap, completed, errored) to the owning tenant. Events are buffered and
//! POSTed in batches to `TIDELINE_EVENTS_URL` — best-effort, never on the hot path,
//! and a no-op when no ingest URL is configured (self-host / dev).
//!
//! Streams are namespaced per tenant internally (`{namespace}|{id}`), so the owner
//! map is keyed by the namespaced key but remembers the plain id the customer sees.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

/// The owner of a stream — who gets attributed, and whose namespace it lives in.
#[derive(Clone)]
pub struct Tenant {
    pub user_id: String,
    pub org_id: Option<String>,
}

impl Tenant {
    /// The stream namespace for this tenant — the org if present, else the user.
    pub fn namespace(&self) -> String {
        self.org_id.clone().unwrap_or_else(|| self.user_id.clone())
    }
}

enum Kind {
    Created,
    Tokens {
        count: u64,
        last_offset: u64,
    },
    SubConnected {
        resumed: bool,
        from: u64,
        transport: &'static str,
    },
    SubDisconnected,
    Gap {
        from: u64,
    },
    Completed,
    Errored {
        message: String,
    },
}

struct Msg {
    tenant: Tenant,
    stream_id: String, // the plain (customer-facing) id
    kind: Kind,
}

/// Records and ships stream events. Cheap to clone-wrap in an Arc.
pub struct Events {
    tx: Option<mpsc::Sender<Msg>>,
    owners: Mutex<HashMap<String, (Tenant, String)>>, // namespaced key -> (tenant, plain id)
}

impl Events {
    pub fn disabled() -> Self {
        Self {
            tx: None,
            owners: Mutex::new(HashMap::new()),
        }
    }

    /// Spawn the background reporter if an ingest URL is configured.
    pub fn new(url: Option<String>, secret: Option<String>) -> Self {
        match url {
            Some(url) if !url.is_empty() => {
                let (tx, rx) = mpsc::channel(10_000);
                tokio::spawn(reporter(rx, url, secret));
                Self {
                    tx: Some(tx),
                    owners: Mutex::new(HashMap::new()),
                }
            }
            _ => Self::disabled(),
        }
    }

    fn enabled(&self) -> bool {
        self.tx.is_some()
    }

    fn send(&self, tenant: Tenant, plain_id: &str, kind: Kind) {
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(Msg {
                tenant,
                stream_id: plain_id.to_string(),
                kind,
            }); // best-effort
        }
    }

    /// Remember the tenant + plain id behind a namespaced key (first authorized publish).
    pub fn set_owner(&self, key: &str, tenant: &Tenant, plain_id: &str) {
        if self.enabled() {
            self.owners
                .lock()
                .unwrap()
                .entry(key.to_string())
                .or_insert_with(|| (tenant.clone(), plain_id.to_string()));
        }
    }
    pub fn owner(&self, key: &str) -> Option<(Tenant, String)> {
        if !self.enabled() {
            return None;
        }
        self.owners.lock().unwrap().get(key).cloned()
    }
    fn forget(&self, key: &str) {
        self.owners.lock().unwrap().remove(key);
    }

    // --- publish path carries the tenant + plain id explicitly ---
    pub fn created(&self, tenant: &Tenant, plain_id: &str) {
        self.send(tenant.clone(), plain_id, Kind::Created);
    }
    pub fn tokens(&self, tenant: &Tenant, plain_id: &str, count: u64, last_offset: u64) {
        self.send(
            tenant.clone(),
            plain_id,
            Kind::Tokens { count, last_offset },
        );
    }

    // --- subscriber / terminal paths look up the owner by the namespaced key ---
    pub fn sub_connected(&self, key: &str, resumed: bool, from: u64, transport: &'static str) {
        if let Some((t, plain)) = self.owner(key) {
            self.send(
                t,
                &plain,
                Kind::SubConnected {
                    resumed,
                    from,
                    transport,
                },
            );
        }
    }
    pub fn sub_disconnected(&self, key: &str) {
        if let Some((t, plain)) = self.owner(key) {
            self.send(t, &plain, Kind::SubDisconnected);
        }
    }
    pub fn gap(&self, key: &str, from: u64) {
        if let Some((t, plain)) = self.owner(key) {
            self.send(t, &plain, Kind::Gap { from });
        }
    }
    pub fn completed(&self, key: &str) {
        if let Some((t, plain)) = self.owner(key) {
            self.send(t, &plain, Kind::Completed);
        }
        self.forget(key);
    }
    pub fn errored(&self, key: &str, message: &str) {
        if let Some((t, plain)) = self.owner(key) {
            self.send(
                t,
                &plain,
                Kind::Errored {
                    message: message.to_string(),
                },
            );
        }
        self.forget(key);
    }
}

fn to_json(m: &Msg) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("userId".into(), m.tenant.user_id.clone().into());
    if let Some(org) = &m.tenant.org_id {
        o.insert("orgId".into(), org.clone().into());
    }
    o.insert("streamId".into(), m.stream_id.clone().into());
    match &m.kind {
        Kind::Created => {
            o.insert("type".into(), "stream.created".into());
        }
        Kind::Tokens { count, last_offset } => {
            o.insert("type".into(), "tokens".into());
            o.insert("count".into(), (*count).into());
            o.insert("lastOffset".into(), (*last_offset).into());
        }
        Kind::SubConnected {
            resumed,
            from,
            transport,
        } => {
            o.insert("type".into(), "subscriber.connected".into());
            o.insert("resumed".into(), (*resumed).into());
            o.insert("from".into(), (*from).into());
            o.insert("transport".into(), (*transport).into());
        }
        Kind::SubDisconnected => {
            o.insert("type".into(), "subscriber.disconnected".into());
        }
        Kind::Gap { from } => {
            o.insert("type".into(), "gap".into());
            o.insert("from".into(), (*from).into());
        }
        Kind::Completed => {
            o.insert("type".into(), "stream.completed".into());
        }
        Kind::Errored { message } => {
            o.insert("type".into(), "stream.errored".into());
            o.insert("message".into(), message.clone().into());
        }
    }
    Value::Object(o)
}

async fn reporter(mut rx: mpsc::Receiver<Msg>, url: String, secret: Option<String>) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut buf: Vec<Msg> = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_millis(1000));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Some(m) => {
                    buf.push(m);
                    if buf.len() >= 256 {
                        flush(&client, &url, &secret, &mut buf).await;
                    }
                }
                None => {
                    flush(&client, &url, &secret, &mut buf).await;
                    return;
                }
            },
            _ = tick.tick() => flush(&client, &url, &secret, &mut buf).await,
        }
    }
}

async fn flush(client: &reqwest::Client, url: &str, secret: &Option<String>, buf: &mut Vec<Msg>) {
    if buf.is_empty() {
        return;
    }
    let events: Vec<Value> = buf.iter().map(to_json).collect();
    buf.clear();
    let mut req = client.post(url).json(&json!({ "events": events }));
    if let Some(s) = secret {
        req = req.header("x-tideline-verify-secret", s);
    }
    let _ = req.send().await; // best-effort observability
}
