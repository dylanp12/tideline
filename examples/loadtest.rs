//! Load test: one generation fanned out to N concurrent SSE subscribers.
//!
//! Bounded so it can never hang: each subscriber returns the instant it has
//! received every expected token (it never waits for the connection to close),
//! and every read is wrapped in a hard timeout.
//!
//! Run: `cargo run --release --example loadtest -- [subscribers] [tokens]`
//! For large runs raise the fd limit first, e.g. `ulimit -n 32768`.
//! NOTE: in-process over loopback measures fan-out CPU throughput on one box —
//! treat it as a floor, not a cloud benchmark.

use futures::StreamExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let subs: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let tokens: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    // Engine, in-process on an ephemeral port.
    let app = tideline::api::router_with(tideline::manager::StreamManager::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("http://{addr}/streams/load");
    let client = reqwest::Client::new();
    let connected = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::with_capacity(subs);
    for _ in 0..subs {
        let url = url.clone();
        let client = client.clone();
        let connected = connected.clone();
        handles.push(tokio::spawn(async move {
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(_) => return 0u64,
            };
            connected.fetch_add(1, Ordering::Relaxed);
            // Count "id: " markers (one per token); stop as soon as we've seen
            // them all. Hard 20s ceiling means a stuck task can't hang the run.
            let read = async {
                let mut stream = resp.bytes_stream();
                let mut buf = Vec::new();
                let mut count = 0u64;
                while let Some(chunk) = stream.next().await {
                    let Ok(bytes) = chunk else { break };
                    buf.extend_from_slice(&bytes);
                    count = String::from_utf8_lossy(&buf).matches("id: ").count() as u64;
                    if count >= tokens {
                        break;
                    }
                }
                count
            };
            tokio::time::timeout(Duration::from_secs(20), read)
                .await
                .unwrap_or(0)
        }));
    }

    // Wait (bounded) for everyone to connect, then settle.
    let deadline = Instant::now() + Duration::from_secs(20);
    while connected.load(Ordering::Relaxed) < subs as u64 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let live = connected.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Publish one generation, then complete.
    let start = Instant::now();
    for i in 0..tokens {
        let _ = client.post(&url).body(format!("tok{i} ")).send().await;
    }
    let _ = client.post(format!("{url}/complete")).send().await;

    let mut total = 0u64;
    for h in handles {
        total += h.await.unwrap_or(0);
    }
    let dur = start.elapsed();
    let expected = live * tokens;

    println!("── tideline load test ─────────────────────────────");
    println!("subscribers connected  : {live} / {subs}");
    println!("tokens in generation   : {tokens}");
    println!("events delivered       : {total}  (expected {expected})");
    println!(
        "delivery completeness  : {:.2}%",
        100.0 * total as f64 / expected.max(1) as f64
    );
    println!("publish -> all delivered: {:.3} s", dur.as_secs_f64());
    println!(
        "fan-out throughput     : {:.0} events/sec",
        total as f64 / dur.as_secs_f64().max(1e-9)
    );
}
