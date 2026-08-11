use crate::token::{Signal, Token};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::broadcast;

/// Default per-stream retained-token cap. Bounds memory; older tokens are
/// evicted and resuming past them yields a `Gap` signal.
const DEFAULT_MAX_BUFFER: usize = 65_536;

struct Inner {
    tokens: VecDeque<Token>,
    next_offset: u64,
    complete: bool,
    error: Option<String>,
    completed_at: Option<Instant>,
    last_activity: Instant,
    max_buffer: usize,
}

pub struct Stream {
    inner: Mutex<Inner>,
    tx: broadcast::Sender<Signal>,
    subscribers: AtomicU64,
    created_at: Instant,
    /// Sticky privacy flag: once set, reads require a signed ticket. Lives and
    /// dies with the stream (reaped/evicted with it), so a reused id starts fresh.
    private: AtomicBool,
}

/// A point-in-time view of a stream for a subscriber's initial replay.
pub struct Snapshot {
    pub tokens: Vec<Token>,
    pub complete: bool,
    pub error: Option<String>,
    /// The dedup boundary: the next offset to be assigned at snapshot time.
    pub total: u64,
    /// True if `from` was below the earliest retained offset (already evicted).
    pub gap: bool,
}

/// Decrements the live-subscriber count when a `read_stream` ends or is dropped.
struct SubGuard(Arc<Stream>);
impl Drop for SubGuard {
    fn drop(&mut self) {
        self.0.subscribers.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Stream {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_BUFFER)
    }

    pub fn with_capacity(max_buffer: usize) -> Self {
        let (tx, _rx) = broadcast::channel(1024);
        let now = Instant::now();
        Stream {
            inner: Mutex::new(Inner {
                tokens: VecDeque::new(),
                next_offset: 0,
                complete: false,
                error: None,
                completed_at: None,
                last_activity: now,
                max_buffer: max_buffer.max(1),
            }),
            tx,
            subscribers: AtomicU64::new(0),
            created_at: now,
            private: AtomicBool::new(false),
        }
    }

    /// Append a chunk, assigning the next offset. Returns the new offset.
    pub fn publish(&self, data: String) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        inner.last_activity = Instant::now();
        let offset = inner.next_offset;
        inner.next_offset += 1;
        let token = Token { offset, data };
        inner.tokens.push_back(token.clone());
        let max = inner.max_buffer;
        while inner.tokens.len() > max {
            inner.tokens.pop_front();
        }
        let _ = self.tx.send(Signal::Token(token));
        offset
    }

    /// Mark the stream finished successfully. Idempotent; no-op if already terminal.
    pub fn complete(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.last_activity = Instant::now();
        if !inner.complete && inner.error.is_none() {
            inner.complete = true;
            inner.completed_at = Some(Instant::now());
            let _ = self.tx.send(Signal::Done);
        }
    }

    /// Terminate the stream with an error. Idempotent; no-op if already terminal.
    pub fn fail(&self, message: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.last_activity = Instant::now();
        if !inner.complete && inner.error.is_none() {
            inner.error = Some(message.clone());
            inner.completed_at = Some(Instant::now());
            let _ = self.tx.send(Signal::Error(message));
        }
    }

    /// Bump the activity clock (called when a new subscriber connects).
    pub fn touch(&self) {
        self.inner.lock().unwrap().last_activity = Instant::now();
    }

    fn earliest_retained(inner: &Inner) -> u64 {
        inner
            .tokens
            .front()
            .map(|t| t.offset)
            .unwrap_or(inner.next_offset)
    }

    /// Snapshot tokens with offset >= `from` (or the earliest retained, flagging
    /// a gap), plus terminal state and the dedup boundary.
    pub fn snapshot_from(&self, from: u64) -> Snapshot {
        let inner = self.inner.lock().unwrap();
        let earliest = Self::earliest_retained(&inner);
        let gap = from < earliest;
        let eff = from.max(earliest);
        let tokens: Vec<Token> = inner
            .tokens
            .iter()
            .filter(|t| t.offset >= eff)
            .cloned()
            .collect();
        Snapshot {
            tokens,
            complete: inner.complete,
            error: inner.error.clone(),
            total: inner.next_offset,
            gap,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.tx.subscribe()
    }

    // --- accessors (lifecycle + metrics) ---
    pub fn subscriber_count(&self) -> u64 {
        self.subscribers.load(Ordering::Relaxed)
    }
    pub fn last_activity(&self) -> Instant {
        self.inner.lock().unwrap().last_activity
    }
    pub fn completed_at(&self) -> Option<Instant> {
        self.inner.lock().unwrap().completed_at
    }
    pub fn is_terminal(&self) -> bool {
        let i = self.inner.lock().unwrap();
        i.complete || i.error.is_some()
    }
    pub fn buffered_len(&self) -> usize {
        self.inner.lock().unwrap().tokens.len()
    }
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Mark this stream private — reads then require a signed ticket. Sticky.
    pub fn set_private(&self) {
        self.private.store(true, Ordering::Relaxed);
    }
    pub fn is_private(&self) -> bool {
        self.private.load(Ordering::Relaxed)
    }

    /// Yields a `Gap` (if `from` was evicted), the retained tokens, then live
    /// tokens deduped by offset, ending with `Error`+`Done` or `Done`.
    pub fn read_stream(self: Arc<Self>, from: u64) -> impl futures::Stream<Item = Signal> {
        async_stream::stream! {
            self.subscribers.fetch_add(1, Ordering::Relaxed);
            let _guard = SubGuard(self.clone());

            let mut rx = self.subscribe();
            let snap = self.snapshot_from(from);
            if snap.gap {
                yield Signal::Gap;
            }
            for t in snap.tokens {
                yield Signal::Token(t);
            }
            if let Some(e) = snap.error {
                yield Signal::Error(e);
                yield Signal::Done;
                return;
            }
            if snap.complete {
                yield Signal::Done;
                return;
            }
            loop {
                match rx.recv().await {
                    Ok(Signal::Token(t)) => {
                        if t.offset >= snap.total {
                            yield Signal::Token(t);
                        }
                    }
                    Ok(Signal::Error(e)) => {
                        yield Signal::Error(e);
                        yield Signal::Done;
                        return;
                    }
                    Ok(Signal::Done) => {
                        yield Signal::Done;
                        return;
                    }
                    Ok(Signal::Gap) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => return,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

impl Default for Stream {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::time::Duration;

    #[test]
    fn publish_assigns_monotonic_offsets() {
        let s = Stream::new();
        assert_eq!(s.publish("a".into()), 0);
        assert_eq!(s.publish("b".into()), 1);
        assert_eq!(s.publish("c".into()), 2);
    }

    #[test]
    fn snapshot_from_returns_tail_and_boundary() {
        let s = Stream::new();
        s.publish("a".into());
        s.publish("b".into());
        s.publish("c".into());
        let snap = s.snapshot_from(1);
        assert_eq!(
            snap.tokens
                .iter()
                .map(|t| t.data.clone())
                .collect::<Vec<_>>(),
            vec!["b", "c"]
        );
        assert!(!snap.complete);
        assert_eq!(snap.total, 3);
        assert!(!snap.gap);
        assert!(snap.error.is_none());
    }

    #[test]
    fn complete_sets_flag_and_is_idempotent() {
        let s = Stream::new();
        s.complete();
        s.complete();
        let snap = s.snapshot_from(0);
        assert!(snap.complete);
        assert!(s.completed_at().is_some());
        assert!(s.is_terminal());
    }

    #[test]
    fn buffer_caps_and_signals_gap_on_evicted_offset() {
        let s = Stream::with_capacity(3);
        for c in ["a", "b", "c", "d", "e"] {
            s.publish(c.into());
        }
        let snap = s.snapshot_from(0);
        assert!(snap.gap);
        assert_eq!(snap.total, 5);
        assert_eq!(
            snap.tokens
                .iter()
                .map(|t| t.data.clone())
                .collect::<Vec<_>>(),
            vec!["c", "d", "e"]
        );
        let snap2 = s.snapshot_from(4);
        assert!(!snap2.gap);
        assert_eq!(
            snap2
                .tokens
                .iter()
                .map(|t| t.data.clone())
                .collect::<Vec<_>>(),
            vec!["e"]
        );
    }

    async fn drain(s: Arc<Stream>, from: u64) -> Vec<String> {
        let mut out = Vec::new();
        let mut rx = Box::pin(s.read_stream(from));
        while let Some(sig) = rx.next().await {
            match sig {
                Signal::Token(t) => out.push(t.data),
                Signal::Gap => {}
                Signal::Error(_) => break,
                Signal::Done => break,
            }
        }
        out
    }

    #[tokio::test]
    async fn late_joiner_replays_then_stops_on_done() {
        let s = Arc::new(Stream::new());
        s.publish("a".into());
        s.publish("b".into());
        s.complete();
        assert_eq!(drain(s, 0).await, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn resume_from_offset_skips_already_seen() {
        let s = Arc::new(Stream::new());
        s.publish("a".into());
        s.publish("b".into());
        s.complete();
        assert_eq!(drain(s, 1).await, vec!["b"]);
    }

    #[tokio::test]
    async fn no_gap_no_dup_across_seam() {
        let s = Arc::new(Stream::new());
        s.publish("a".into());
        let reader = tokio::spawn(drain(s.clone(), 0));
        for c in ["b", "c", "d"] {
            s.publish(c.into());
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        s.complete();
        assert_eq!(reader.await.unwrap(), vec!["a", "b", "c", "d"]);
    }

    #[tokio::test]
    async fn fan_out_all_subscribers_get_everything() {
        let s = Arc::new(Stream::new());
        let r1 = tokio::spawn(drain(s.clone(), 0));
        let r2 = tokio::spawn(drain(s.clone(), 0));
        tokio::time::sleep(Duration::from_millis(5)).await;
        for c in ["x", "y", "z"] {
            s.publish(c.into());
        }
        s.complete();
        assert_eq!(r1.await.unwrap(), vec!["x", "y", "z"]);
        assert_eq!(r2.await.unwrap(), vec!["x", "y", "z"]);
    }

    #[tokio::test]
    async fn read_stream_emits_gap_when_resuming_past_eviction() {
        let s = Arc::new(Stream::with_capacity(2));
        for c in ["a", "b", "c", "d"] {
            s.publish(c.into());
        }
        s.complete();
        let mut rx = Box::pin(s.read_stream(0));
        let mut saw_gap = false;
        let mut data = Vec::new();
        while let Some(sig) = rx.next().await {
            match sig {
                Signal::Gap => saw_gap = true,
                Signal::Token(t) => data.push(t.data),
                Signal::Error(_) => break,
                Signal::Done => break,
            }
        }
        assert!(saw_gap);
        assert_eq!(data, vec!["c", "d"]);
    }

    #[tokio::test]
    async fn fail_emits_error_then_done() {
        let s = Arc::new(Stream::new());
        s.publish("a".into());
        s.fail("boom".into());
        let mut rx = Box::pin(s.clone().read_stream(0));
        let mut got = Vec::new();
        let mut err = None;
        while let Some(sig) = rx.next().await {
            match sig {
                Signal::Token(t) => got.push(t.data),
                Signal::Error(e) => err = Some(e),
                Signal::Gap => {}
                Signal::Done => break,
            }
        }
        assert_eq!(got, vec!["a"]);
        assert_eq!(err.as_deref(), Some("boom"));
        assert!(s.is_terminal());
    }

    #[tokio::test]
    async fn complete_after_fail_is_noop() {
        let s = Arc::new(Stream::new());
        s.fail("x".into());
        s.complete();
        assert_eq!(s.snapshot_from(0).error.as_deref(), Some("x"));
        assert!(!s.snapshot_from(0).complete);
    }

    #[tokio::test]
    async fn subscriber_count_tracks_live_readers() {
        let s = Arc::new(Stream::new());
        assert_eq!(s.subscriber_count(), 0);
        let h = tokio::spawn(drain(s.clone(), 0));
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(s.subscriber_count(), 1);
        s.complete();
        let _ = h.await;
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(s.subscriber_count(), 0);
    }
}
