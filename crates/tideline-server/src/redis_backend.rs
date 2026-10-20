//! Redis-backed storage for horizontal scale.
//!
//! The whole point: with the in-memory backend, a publish on instance A is
//! invisible to a subscriber on instance B. This backend puts every stream in a
//! shared **Redis Stream** (an append-only, ordered, trimmable log), so any
//! instance can serve any subscriber with full resume/replay/fan-out.
//!
//! Per stream `id` there are two keys (hash-tagged on `id` so they share a slot
//! under Redis Cluster):
//!   - `<prefix>:s:{id}` — the Redis Stream. Each entry is either a token
//!     (`o`=offset, `d`=data) or a terminal marker (`t`=done | `t`=error,`m`=msg).
//!   - `<prefix>:o:{id}` — an INCR counter assigning monotonic offsets.
//!
//! A publish is one atomic Lua script: INCR the offset, XADD the token with that
//! offset (trimming to `max_buffer` via MAXLEN), and refresh both keys' TTL —
//! so offsets and stream order can never disagree even under concurrent writers.
//!
//! A subscriber does the same subscribe-before-snapshot dance as the in-memory
//! engine, but against Redis: XRANGE for the replay/history (detecting a gap if
//! the resume point was trimmed), then a blocking XREAD loop from the last seen
//! entry id for the live tail. The entry-id cursor makes the seam exact — no
//! lost or duplicated token — without any cross-instance coordination.

use crate::backend::Backend;
use crate::manager::MetricsSnapshot;
use crate::token::{Signal, Token};
use futures::stream::BoxStream;
use futures::StreamExt;
use redis::streams::{StreamId, StreamRangeReply, StreamReadOptions, StreamReadReply};
use redis::AsyncCommands;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Atomic publish: assign the next offset and append the token in one step, so
/// stream order always matches offset order. KEYS = [stream, offset-counter,
/// privacy]; ARGV = [data, maxlen, ttl_ms]. Returns the assigned offset. The
/// privacy-key PEXPIRE is a no-op when the stream isn't private (key absent), and
/// otherwise keeps the private flag's TTL in lockstep with the stream's.
const PUBLISH_LUA: &str = r#"
local o = redis.call('INCR', KEYS[2]) - 1
redis.call('XADD', KEYS[1], 'MAXLEN', '~', ARGV[2], '*', 'o', o, 'd', ARGV[1])
redis.call('PEXPIRE', KEYS[1], ARGV[3])
redis.call('PEXPIRE', KEYS[2], ARGV[3])
redis.call('PEXPIRE', KEYS[3], ARGV[3])
return o
"#;

#[derive(Clone)]
pub struct RedisConfig {
    pub key_prefix: String,
    /// Approximate retained tokens per stream (MAXLEN ~). Older tokens are
    /// trimmed; resuming past them yields a `Gap`.
    pub max_buffer: usize,
    /// TTL refreshed on every publish — idle streams expire (Redis handles
    /// lifecycle; no reaper needed).
    pub idle_ttl_ms: i64,
    /// Shorter TTL applied once a stream is terminal.
    pub completed_ttl_ms: i64,
    pub max_subscribers_per_stream: u64,
    /// How long each live XREAD blocks before looping.
    pub block_ms: usize,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            key_prefix: "tideline".into(),
            max_buffer: 65_536,
            idle_ttl_ms: 30 * 60 * 1000,
            completed_ttl_ms: 5 * 60 * 1000,
            max_subscribers_per_stream: 50_000,
            block_ms: 5_000,
        }
    }
}

/// Per-instance bookkeeping. Cluster-wide metrics aren't cheaply knowable, so
/// these are local to this process (scrape every instance and sum).
#[derive(Default)]
struct LocalState {
    subs: Mutex<HashMap<String, u64>>,
    published: AtomicU64,
    gaps: AtomicU64,
}

impl LocalState {
    fn incr(&self, id: &str) {
        *self.subs.lock().unwrap().entry(id.to_string()).or_insert(0) += 1;
    }
    fn decr(&self, id: &str) {
        let mut m = self.subs.lock().unwrap();
        if let Some(c) = m.get_mut(id) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                m.remove(id);
            }
        }
    }
    fn count(&self, id: &str) -> u64 {
        self.subs.lock().unwrap().get(id).copied().unwrap_or(0)
    }
    fn total(&self) -> u64 {
        self.subs.lock().unwrap().values().copied().sum()
    }
    fn streams(&self) -> u64 {
        self.subs.lock().unwrap().len() as u64
    }
}

/// Decrements the local subscriber count when an SSE stream ends or is dropped.
struct SubGuard {
    local: Arc<LocalState>,
    id: String,
}
impl Drop for SubGuard {
    fn drop(&mut self) {
        self.local.decr(&self.id);
    }
}

pub struct RedisBackend {
    client: redis::Client,
    conn: redis::aio::MultiplexedConnection,
    cfg: RedisConfig,
    local: Arc<LocalState>,
}

impl RedisBackend {
    /// Connect to Redis (e.g. `redis://127.0.0.1:6379`). Errors if unreachable.
    pub async fn connect(url: &str, cfg: RedisConfig) -> redis::RedisResult<Self> {
        let client = redis::Client::open(url)?;
        let conn = client.get_multiplexed_async_connection().await?;
        Ok(Self {
            client,
            conn,
            cfg,
            local: Arc::new(LocalState::default()),
        })
    }

    // Hash-tag on {id} keeps a stream's two keys in the same Redis Cluster slot.
    fn skey(&self, id: &str) -> String {
        format!("{}:s:{{{}}}", self.cfg.key_prefix, id)
    }
    fn okey(&self, id: &str) -> String {
        format!("{}:o:{{{}}}", self.cfg.key_prefix, id)
    }
    // Privacy marker: present (= "1") iff the stream was published private.
    fn pkey(&self, id: &str) -> String {
        format!("{}:p:{{{}}}", self.cfg.key_prefix, id)
    }

    /// Append a terminal marker entry and apply the (shorter) completed TTL.
    async fn mark_terminal(&self, id: &str, items: &[(&str, &str)]) {
        let mut conn = self.conn.clone();
        let skey = self.skey(id);
        let okey = self.okey(id);
        let pkey = self.pkey(id);
        let added: redis::RedisResult<String> = conn.xadd(&skey, "*", items).await;
        if added.is_ok() {
            // Privacy outlives completion: a finished private stream is still
            // replayable, so its flag rides the same (shorter) completed TTL.
            let _: redis::RedisResult<i64> = conn.pexpire(&skey, self.cfg.completed_ttl_ms).await;
            let _: redis::RedisResult<i64> = conn.pexpire(&okey, self.cfg.completed_ttl_ms).await;
            let _: redis::RedisResult<i64> = conn.pexpire(&pkey, self.cfg.completed_ttl_ms).await;
        }
    }
}

/// A decoded Redis Stream entry.
enum Entry {
    Token(u64, String),
    Done,
    Error(String),
    Other,
}

fn parse(sid: &StreamId) -> Entry {
    if let Some(t) = sid.get::<String>("t") {
        return match t.as_str() {
            "done" => Entry::Done,
            "error" => Entry::Error(sid.get::<String>("m").unwrap_or_default()),
            _ => Entry::Other,
        };
    }
    match (sid.get::<u64>("o"), sid.get::<String>("d")) {
        (Some(o), Some(d)) => Entry::Token(o, d),
        _ => Entry::Other,
    }
}

#[async_trait::async_trait]
impl Backend for RedisBackend {
    async fn publish(&self, id: &str, data: String) -> u64 {
        let mut conn = self.conn.clone();
        let offset: redis::RedisResult<i64> = redis::Script::new(PUBLISH_LUA)
            .key(self.skey(id))
            .key(self.okey(id))
            .key(self.pkey(id))
            .arg(data)
            .arg(self.cfg.max_buffer)
            .arg(self.cfg.idle_ttl_ms)
            .invoke_async(&mut conn)
            .await;
        self.local.published.fetch_add(1, Ordering::Relaxed);
        offset.map(|o| o as u64).unwrap_or(0)
    }

    async fn complete(&self, id: &str) {
        self.mark_terminal(id, &[("t", "done")]).await;
    }

    async fn fail(&self, id: &str, message: String) {
        self.mark_terminal(id, &[("t", "error"), ("m", message.as_str())])
            .await;
    }

    async fn delete(&self, id: &str) -> bool {
        let mut conn = self.conn.clone();
        let n: redis::RedisResult<i64> = conn
            .del(&[self.skey(id), self.okey(id), self.pkey(id)])
            .await;
        n.map(|c| c > 0).unwrap_or(false)
    }

    async fn subscriber_count(&self, id: &str) -> u64 {
        self.local.count(id)
    }

    fn max_subscribers_per_stream(&self) -> u64 {
        self.cfg.max_subscribers_per_stream
    }

    fn incr_gaps(&self) {
        self.local.gaps.fetch_add(1, Ordering::Relaxed);
    }

    async fn subscribe(&self, id: &str, from: u64) -> BoxStream<'static, Signal> {
        let client = self.client.clone();
        let local = self.local.clone();
        let skey = self.skey(id);
        let id_owned = id.to_string();
        let block_ms = self.cfg.block_ms;

        local.incr(&id_owned);
        let guard = SubGuard {
            local: local.clone(),
            id: id_owned,
        };

        async_stream::stream! {
            let _guard = guard; // decrements the live count when this stream is dropped

            // A dedicated connection: the live tail uses a blocking XREAD, which
            // would otherwise stall a shared multiplexed connection.
            let mut conn = match client.get_multiplexed_async_connection().await {
                Ok(c) => c,
                Err(_) => { yield Signal::Error("redis unavailable".into()); yield Signal::Done; return; }
            };

            // --- snapshot / replay (subscribe semantics live in the entry ids) ---
            let snap: StreamRangeReply = match conn.xrange(&skey, "-", "+").await {
                Ok(r) => r,
                Err(_) => { yield Signal::Error("redis read failed".into()); yield Signal::Done; return; }
            };
            let earliest = snap.ids.iter().find_map(|s| match parse(s) {
                Entry::Token(o, _) => Some(o),
                _ => None,
            });
            if let Some(e) = earliest {
                if from < e {
                    yield Signal::Gap;
                }
            }
            let lower = earliest.map_or(from, |e| from.max(e));
            for sid in &snap.ids {
                match parse(sid) {
                    Entry::Token(o, d) => {
                        if o >= lower {
                            yield Signal::Token(Token { offset: o, data: d });
                        }
                    }
                    Entry::Done => { yield Signal::Done; return; }
                    Entry::Error(m) => { yield Signal::Error(m); yield Signal::Done; return; }
                    Entry::Other => {}
                }
            }

            // --- live tail: XREAD strictly after the last entry we replayed ---
            // "0" means "from the beginning" and is the right cursor for an empty
            // snapshot (nothing to duplicate); otherwise resume after the last id.
            let mut cursor = snap.ids.last().map(|s| s.id.clone()).unwrap_or_else(|| "0".to_string());
            let opts = StreamReadOptions::default().block(block_ms).count(512);
            loop {
                let reply: StreamReadReply = match conn.xread_options(&[&skey], &[&cursor], &opts).await {
                    Ok(r) => r,
                    Err(_) => { yield Signal::Error("redis read failed".into()); yield Signal::Done; return; }
                };
                let Some(k) = reply.keys.into_iter().next() else { continue; }; // block timeout
                for sid in k.ids {
                    cursor = sid.id.clone();
                    match parse(&sid) {
                        Entry::Token(o, d) => yield Signal::Token(Token { offset: o, data: d }),
                        Entry::Done => { yield Signal::Done; return; }
                        Entry::Error(m) => { yield Signal::Error(m); yield Signal::Done; return; }
                        Entry::Other => {}
                    }
                }
            }
        }
        .boxed()
    }

    async fn set_private(&self, id: &str) {
        let mut conn = self.conn.clone();
        // Shared across instances (that's the point): any instance serving a read
        // sees the flag. TTL matches the stream's; each publish's Lua refreshes it.
        let _: redis::RedisResult<()> = redis::cmd("SET")
            .arg(self.pkey(id))
            .arg("1")
            .arg("PX")
            .arg(self.cfg.idle_ttl_ms)
            .query_async(&mut conn)
            .await;
    }

    async fn is_private(&self, id: &str) -> bool {
        let mut conn = self.conn.clone();
        let r: redis::RedisResult<bool> = conn.exists(self.pkey(id)).await;
        r.unwrap_or(false)
    }

    async fn metrics(&self) -> MetricsSnapshot {
        // Per-instance. active_streams here = streams this instance is serving.
        MetricsSnapshot {
            active_streams: self.local.streams(),
            active_subscribers: self.local.total(),
            buffered_tokens: 0,
            streams_created: 0,
            tokens_published: self.local.published.load(Ordering::Relaxed),
            gaps_served: self.local.gaps.load(Ordering::Relaxed),
            reaped: 0,
        }
    }
}
