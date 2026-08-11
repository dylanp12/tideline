use crate::stream::Stream;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Lifecycle + resource limits for the in-memory stream registry.
#[derive(Clone)]
pub struct ManagerConfig {
    pub max_streams: usize,
    pub idle_ttl: Duration,
    pub completed_ttl: Duration,
    pub max_buffer: usize,
    pub max_subscribers_per_stream: u64,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            max_streams: 50_000,
            idle_ttl: Duration::from_secs(30 * 60),
            completed_ttl: Duration::from_secs(5 * 60),
            max_buffer: 65_536,
            max_subscribers_per_stream: 50_000,
        }
    }
}

#[derive(Default)]
struct Metrics {
    streams_created: AtomicU64,
    tokens_published: AtomicU64,
    gaps_served: AtomicU64,
    reaped: AtomicU64,
}

/// Snapshot of live metrics for the `/metrics` endpoint.
pub struct MetricsSnapshot {
    pub active_streams: u64,
    pub active_subscribers: u64,
    pub buffered_tokens: u64,
    pub streams_created: u64,
    pub tokens_published: u64,
    pub gaps_served: u64,
    pub reaped: u64,
}

#[derive(Clone)]
pub struct StreamManager {
    streams: Arc<Mutex<HashMap<String, Arc<Stream>>>>,
    cfg: Arc<ManagerConfig>,
    metrics: Arc<Metrics>,
}

impl Default for StreamManager {
    fn default() -> Self {
        Self::with_config(ManagerConfig::default())
    }
}

impl StreamManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(cfg: ManagerConfig) -> Self {
        Self {
            streams: Arc::new(Mutex::new(HashMap::new())),
            cfg: Arc::new(cfg),
            metrics: Arc::new(Metrics::default()),
        }
    }

    pub fn config(&self) -> &ManagerConfig {
        &self.cfg
    }

    /// Get the stream for `id`, creating it on first use. Bumps activity, and
    /// evicts the least-recently-active stream if the registry is at capacity.
    pub fn get_or_create(&self, id: &str) -> Arc<Stream> {
        let mut map = self.streams.lock().unwrap();
        if let Some(s) = map.get(id) {
            s.touch();
            return s.clone();
        }
        if map.len() >= self.cfg.max_streams {
            self.evict_lru_locked(&mut map);
        }
        let s = Arc::new(Stream::with_capacity(self.cfg.max_buffer));
        map.insert(id.to_string(), s.clone());
        self.metrics.streams_created.fetch_add(1, Ordering::Relaxed);
        s
    }

    /// Get an existing stream without creating it.
    pub fn get(&self, id: &str) -> Option<Arc<Stream>> {
        self.streams.lock().unwrap().get(id).cloned()
    }

    /// Remove a stream. Returns true if it existed.
    pub fn delete(&self, id: &str) -> bool {
        self.streams.lock().unwrap().remove(id).is_some()
    }

    pub fn len(&self) -> usize {
        self.streams.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn incr_published(&self) {
        self.metrics
            .tokens_published
            .fetch_add(1, Ordering::Relaxed);
    }
    pub fn incr_gaps(&self) {
        self.metrics.gaps_served.fetch_add(1, Ordering::Relaxed);
    }

    fn evict_lru_locked(&self, map: &mut HashMap<String, Arc<Stream>>) {
        if let Some(oldest) = map
            .iter()
            .min_by_key(|(_, s)| s.last_activity())
            .map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
            self.metrics.reaped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Remove streams idle past `idle_ttl` or terminal past `completed_ttl`.
    /// Returns the number removed.
    pub fn reap(&self) -> usize {
        let now = Instant::now();
        let mut map = self.streams.lock().unwrap();
        let dead: Vec<String> = map
            .iter()
            .filter(|(_, s)| {
                let idle = now.duration_since(s.last_activity()) > self.cfg.idle_ttl;
                let done = s
                    .completed_at()
                    .map(|t| now.duration_since(t) > self.cfg.completed_ttl)
                    .unwrap_or(false);
                idle || done
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in &dead {
            map.remove(k);
        }
        let n = dead.len();
        self.metrics.reaped.fetch_add(n as u64, Ordering::Relaxed);
        n
    }

    /// Spawn a background task that reaps expired streams every `every`.
    pub fn spawn_reaper(&self, every: Duration) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                me.reap();
            }
        });
    }

    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        let map = self.streams.lock().unwrap();
        let mut subs = 0u64;
        let mut buffered = 0u64;
        for s in map.values() {
            subs += s.subscriber_count();
            buffered += s.buffered_len() as u64;
        }
        MetricsSnapshot {
            active_streams: map.len() as u64,
            active_subscribers: subs,
            buffered_tokens: buffered,
            streams_created: self.metrics.streams_created.load(Ordering::Relaxed),
            tokens_published: self.metrics.tokens_published.load(Ordering::Relaxed),
            gaps_served: self.metrics.gaps_served.load(Ordering::Relaxed),
            reaped: self.metrics.reaped.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_or_create_is_stable_per_id() {
        let m = StreamManager::new();
        let a1 = m.get_or_create("x");
        let a2 = m.get_or_create("x");
        assert!(Arc::ptr_eq(&a1, &a2));
        a1.publish("hello".into());
        let snap = m.get("x").unwrap().snapshot_from(0);
        assert_eq!(snap.tokens[0].data, "hello");
    }

    #[test]
    fn get_missing_returns_none() {
        let m = StreamManager::new();
        assert!(m.get("nope").is_none());
    }

    #[test]
    fn delete_removes_stream() {
        let m = StreamManager::new();
        m.get_or_create("d");
        assert!(m.delete("d"));
        assert!(!m.delete("d"));
        assert!(m.get("d").is_none());
    }

    #[test]
    fn reap_removes_completed_past_ttl_keeps_active() {
        let cfg = ManagerConfig {
            completed_ttl: Duration::ZERO,
            idle_ttl: Duration::from_secs(3600),
            ..Default::default()
        };
        let m = StreamManager::with_config(cfg);
        m.get_or_create("done").complete();
        m.get_or_create("live").publish("x".into());
        assert_eq!(m.len(), 2);
        let n = m.reap();
        assert_eq!(n, 1);
        assert!(m.get("done").is_none());
        assert!(m.get("live").is_some());
    }

    #[test]
    fn evicts_lru_at_capacity() {
        let cfg = ManagerConfig {
            max_streams: 2,
            ..Default::default()
        };
        let m = StreamManager::with_config(cfg);
        m.get_or_create("a");
        std::thread::sleep(Duration::from_millis(3));
        m.get_or_create("b");
        std::thread::sleep(Duration::from_millis(3));
        m.get_or_create("a"); // touch a -> b is now the LRU
        std::thread::sleep(Duration::from_millis(3));
        m.get_or_create("c"); // at cap -> evict b
        assert_eq!(m.len(), 2);
        assert!(m.get("a").is_some());
        assert!(m.get("c").is_some());
        assert!(m.get("b").is_none());
    }

    #[test]
    fn metrics_snapshot_counts() {
        let m = StreamManager::new();
        m.get_or_create("a").publish("x".into());
        m.get_or_create("b");
        let snap = m.metrics_snapshot();
        assert_eq!(snap.active_streams, 2);
        assert_eq!(snap.streams_created, 2);
        assert_eq!(snap.buffered_tokens, 1);
    }
}
