//! The engine reports tenant-attributed stream events to the control plane.
//! We stand up a mock control plane (verify -> tenant, events -> capture) and
//! prove a publish/complete produces created + tokens + completed, attributed to
//! the right user and stream.

use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tideline::api::{router_with_events, AuthConfig};
use tideline::backend::MemoryBackend;
use tideline::events::Events;
use tideline::manager::StreamManager;

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn engine_emits_tenant_events() {
    let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));

    // mock control plane: /verify returns a tenant; /events captures the batch.
    let app = Router::new()
        .route(
            "/verify",
            post(|| async {
                Json(serde_json::json!({ "valid": true, "userId": "user_test", "orgId": null }))
            }),
        )
        .route(
            "/events",
            post({
                let cap = captured.clone();
                move |body: Json<Value>| {
                    let cap = cap.clone();
                    async move {
                        if let Some(arr) = body.0.get("events").and_then(|e| e.as_array()) {
                            cap.lock().unwrap().extend(arr.iter().cloned());
                        }
                        axum::http::StatusCode::OK
                    }
                }
            }),
        );
    let base = serve(app).await;

    let auth = AuthConfig {
        verify_url: Some(format!("{base}/verify")),
        verify_secret: Some("s".into()),
        ..Default::default()
    };
    let events = Arc::new(Events::new(
        Some(format!("{base}/events")),
        Some("s".into()),
    ));
    let engine = serve(router_with_events(
        Arc::new(MemoryBackend::new(StreamManager::new())),
        auth,
        events,
    ))
    .await;

    let client = reqwest::Client::new();
    client
        .post(format!("{engine}/streams/s1"))
        .bearer_auth("tl_live_x")
        .body("a")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{engine}/streams/s1"))
        .bearer_auth("tl_live_x")
        .body("b")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{engine}/streams/s1/complete"))
        .bearer_auth("tl_live_x")
        .send()
        .await
        .unwrap();

    // wait for the reporter's batched flush
    tokio::time::sleep(Duration::from_millis(1400)).await;

    let evs = captured.lock().unwrap().clone();
    let types: Vec<&str> = evs
        .iter()
        .filter_map(|e| e.get("type").and_then(|t| t.as_str()))
        .collect();
    assert!(types.contains(&"stream.created"), "types: {types:?}");
    assert!(
        types.iter().filter(|t| **t == "tokens").count() >= 1,
        "types: {types:?}"
    );
    assert!(types.contains(&"stream.completed"), "types: {types:?}");

    let created = evs
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("stream.created"))
        .expect("a created event");
    assert_eq!(
        created.get("userId").and_then(|u| u.as_str()),
        Some("user_test")
    );
    assert_eq!(created.get("streamId").and_then(|u| u.as_str()), Some("s1"));
}
