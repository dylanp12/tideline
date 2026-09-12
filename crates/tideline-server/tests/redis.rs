//! Cross-instance integration tests for the Redis backend.
//!
//! Each test spins up *two* `RedisBackend`s sharing one Redis — standing in for
//! two server processes behind a load balancer — and proves that a publish on
//! instance A is fully visible (replay, resume, live tail, errors) to a
//! subscriber on instance B. That cross-process visibility is the whole reason
//! the adapter exists.
//!
//! These self-skip if no Redis is reachable, so `cargo test` stays green on a
//! machine without one. Point them at a server with TIDELINE_REDIS_URL
//! (default redis://127.0.0.1:6379).

use futures::stream::BoxStream;
use futures::StreamExt;
use std::time::Duration;
use tideline::backend::Backend;
use tideline::redis_backend::{RedisBackend, RedisConfig};
use tideline::token::Signal;

fn redis_url() -> String {
    std::env::var("TIDELINE_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into())
}

/// Connect a backend with a test-scoped key prefix, or None if Redis is down.
async fn backend(prefix: &str) -> Option<RedisBackend> {
    let cfg = RedisConfig {
        key_prefix: prefix.into(),
        block_ms: 1000,
        ..Default::default()
    };
    RedisBackend::connect(&redis_url(), cfg).await.ok()
}

/// Drain an SSE signal stream to its terminal (with a safety timeout).
async fn collect(mut s: BoxStream<'static, Signal>) -> (Vec<String>, bool, Option<String>) {
    let mut tokens = Vec::new();
    let mut done = false;
    let mut err = None;
    let fut = async {
        while let Some(sig) = s.next().await {
            match sig {
                Signal::Token(t) => tokens.push(t.data),
                Signal::Gap => {}
                Signal::Error(e) => err = Some(e),
                Signal::Done => {
                    done = true;
                    break;
                }
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(8), fut).await;
    (tokens, done, err)
}

#[tokio::test]
async fn cross_instance_fanout_and_replay() {
    let Some(a) = backend("tl_t_fanout").await else {
        eprintln!(
            "SKIP cross_instance_fanout_and_replay: no redis at {}",
            redis_url()
        );
        return;
    };
    let b = backend("tl_t_fanout").await.unwrap();
    a.delete("s").await;

    // Publish on instance A...
    a.publish("s", "hello ".into()).await;
    a.publish("s", "world".into()).await;
    a.complete("s").await;

    // ...read it back on instance B.
    let (tokens, done, err) = collect(b.subscribe("s", 0).await).await;
    assert_eq!(tokens, vec!["hello ", "world"], "err={err:?}");
    assert!(done);
    a.delete("s").await;
}

#[tokio::test]
async fn cross_instance_resume_skips_seen() {
    let Some(a) = backend("tl_t_resume").await else {
        return;
    };
    let b = backend("tl_t_resume").await.unwrap();
    a.delete("s").await;

    a.publish("s", "a".into()).await;
    a.publish("s", "b".into()).await;
    a.complete("s").await;

    // Resume from offset 1 on the other instance -> only "b".
    let (tokens, done, _) = collect(b.subscribe("s", 1).await).await;
    assert_eq!(tokens, vec!["b"]);
    assert!(done);
    a.delete("s").await;
}

#[tokio::test]
async fn cross_instance_error_propagates() {
    let Some(a) = backend("tl_t_err").await else {
        return;
    };
    let b = backend("tl_t_err").await.unwrap();
    a.delete("s").await;

    a.publish("s", "partial".into()).await;
    a.fail("s", "boom".into()).await;

    let (tokens, _done, err) = collect(b.subscribe("s", 0).await).await;
    assert_eq!(tokens, vec!["partial"]);
    assert_eq!(err.as_deref(), Some("boom"));
    a.delete("s").await;
}

#[tokio::test]
async fn cross_instance_privacy_is_shared() {
    let Some(a) = backend("tl_t_priv").await else {
        return;
    };
    let b = backend("tl_t_priv").await.unwrap();
    a.delete("s").await;

    // A non-private stream is private on neither instance.
    a.publish("s", "x".into()).await;
    assert!(!a.is_private("s").await);
    assert!(!b.is_private("s").await);
    a.delete("s").await;

    // Mark private on instance A; instance B must see it — without that, a read
    // load-balanced to B would bypass the gate. This is the multi-instance hole
    // the in-memory HashSet had.
    a.set_private("s").await;
    a.publish("s", "secret".into()).await;
    assert!(b.is_private("s").await, "B must observe A's privacy mark");

    // Privacy survives completion (a finished private stream is still replayable).
    a.complete("s").await;
    assert!(b.is_private("s").await, "privacy persists past complete");

    // Delete clears it everywhere.
    a.delete("s").await;
    assert!(!b.is_private("s").await, "delete clears privacy");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_instance_live_tail() {
    let Some(a) = backend("tl_t_live").await else {
        return;
    };
    let b = backend("tl_t_live").await.unwrap();
    a.delete("s").await;

    // Subscribe live on B *before* anything is published on A.
    let stream = b.subscribe("s", 0).await;
    let reader = tokio::spawn(collect(stream));

    // Give B's blocking XREAD time to engage, then publish on A.
    tokio::time::sleep(Duration::from_millis(400)).await;
    a.publish("s", "live-".into()).await;
    a.publish("s", "token".into()).await;
    a.complete("s").await;

    let (tokens, done, _) = reader.await.unwrap();
    assert_eq!(tokens, vec!["live-", "token"]);
    assert!(done);
    a.delete("s").await;
}
