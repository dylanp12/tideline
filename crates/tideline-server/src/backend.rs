//! Storage backend abstraction.
//!
//! The HTTP layer talks to a `Backend`, not directly to the in-memory
//! `StreamManager`. That indirection is what lets Tideline run either as a
//! single in-memory instance (`MemoryBackend`, the default) or as a fleet of
//! instances sharing one Redis (`RedisBackend`) — same SSE semantics, same
//! resume/fan-out/replay guarantees, just durable and cross-process.

use crate::manager::{MetricsSnapshot, StreamManager};
use crate::token::Signal;
use futures::stream::BoxStream;
use futures::StreamExt;

/// Everything the API needs from a stream store. Implementations must preserve
/// the core contract: monotonic offsets, resume-from-offset with a `Gap` when
/// the resume point was evicted, fan-out to every subscriber, and terminal
/// `Done`/`Error`.
#[async_trait::async_trait]
pub trait Backend: Send + Sync + 'static {
    /// Append a chunk, returning its offset.
    async fn publish(&self, id: &str, data: String) -> u64;
    /// Finish the stream successfully (terminal).
    async fn complete(&self, id: &str);
    /// Terminate the stream with an error message (terminal).
    async fn fail(&self, id: &str, message: String);
    /// Drop the stream. Returns true if it existed.
    async fn delete(&self, id: &str) -> bool;
    /// Live subscriber count for `id` (for the per-stream cap).
    async fn subscriber_count(&self, id: &str) -> u64;
    /// The per-stream subscriber cap.
    fn max_subscribers_per_stream(&self) -> u64;
    /// Record that a `Gap` was served (metrics).
    fn incr_gaps(&self);
    /// Subscribe over the lifetime of one SSE connection: an optional `Gap`,
    /// the replayed history from `from`, then live tokens, then a terminal.
    async fn subscribe(&self, id: &str, from: u64) -> BoxStream<'static, Signal>;
    /// Mark a stream private: reads then require a signed ticket. Sticky for the
    /// stream's lifetime; cleared when the stream is deleted or reaped.
    async fn set_private(&self, id: &str);
    /// Whether `id` is currently marked private.
    async fn is_private(&self, id: &str) -> bool;
    /// A point-in-time metrics snapshot.
    async fn metrics(&self) -> MetricsSnapshot;
}

/// Single-instance, in-memory backend — the original engine. Default unless a
/// `RedisBackend` is configured.
pub struct MemoryBackend {
    manager: StreamManager,
}

impl MemoryBackend {
    pub fn new(manager: StreamManager) -> Self {
        Self { manager }
    }
    pub fn manager(&self) -> &StreamManager {
        &self.manager
    }
}

#[async_trait::async_trait]
impl Backend for MemoryBackend {
    async fn publish(&self, id: &str, data: String) -> u64 {
        let offset = self.manager.get_or_create(id).publish(data);
        self.manager.incr_published();
        offset
    }
    async fn complete(&self, id: &str) {
        self.manager.get_or_create(id).complete();
    }
    async fn fail(&self, id: &str, message: String) {
        self.manager.get_or_create(id).fail(message);
    }
    async fn delete(&self, id: &str) -> bool {
        self.manager.delete(id)
    }
    async fn subscriber_count(&self, id: &str) -> u64 {
        self.manager
            .get(id)
            .map(|s| s.subscriber_count())
            .unwrap_or(0)
    }
    fn max_subscribers_per_stream(&self) -> u64 {
        self.manager.config().max_subscribers_per_stream
    }
    fn incr_gaps(&self) {
        self.manager.incr_gaps();
    }
    async fn subscribe(&self, id: &str, from: u64) -> BoxStream<'static, Signal> {
        self.manager.get_or_create(id).read_stream(from).boxed()
    }
    async fn set_private(&self, id: &str) {
        self.manager.get_or_create(id).set_private();
    }
    async fn is_private(&self, id: &str) -> bool {
        self.manager
            .get(id)
            .map(|s| s.is_private())
            .unwrap_or(false)
    }
    async fn metrics(&self) -> MetricsSnapshot {
        self.manager.metrics_snapshot()
    }
}
