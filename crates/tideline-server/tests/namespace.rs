//! Per-tenant stream isolation: two tenants publishing to the same stream id must
//! get separate streams, and a subscriber in one namespace must never see the
//! other's content.

use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use tideline::api::{router_with_config, AuthConfig};
use tideline::manager::StreamManager;

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

// Mock control plane: key containing "A" -> userA, otherwise -> userB.
async fn mock_verify() -> String {
    let app = Router::new().route(
        "/verify",
        post(|body: Json<Value>| async move {
            let key = body.0.get("key").and_then(|k| k.as_str()).unwrap_or("");
            let user = if key.contains('A') { "userA" } else { "userB" };
            Json(serde_json::json!({ "valid": true, "userId": user, "orgId": null }))
        }),
    );
    serve(app).await
}

#[tokio::test]
async fn streams_are_isolated_per_tenant() {
    let vbase = mock_verify().await;
    let auth = AuthConfig {
        verify_url: Some(format!("{vbase}/verify")),
        verify_secret: Some("s".into()),
        ..Default::default()
    };
    let engine = serve(router_with_config(StreamManager::new(), auth)).await;
    let client = reqwest::Client::new();

    // Both tenants publish to the SAME stream id "s1".
    client
        .post(format!("{engine}/streams/s1"))
        .bearer_auth("keyA")
        .body("from-A")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{engine}/streams/s1"))
        .bearer_auth("keyB")
        .body("from-B")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{engine}/streams/s1/complete"))
        .bearer_auth("keyA")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{engine}/streams/s1/complete"))
        .bearer_auth("keyB")
        .send()
        .await
        .unwrap();

    // Tenant A's namespace sees only A's content.
    let a = client
        .get(format!("{engine}/streams/s1?ns=userA"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(a.contains("from-A"), "A should see its own token: {a}");
    assert!(!a.contains("from-B"), "A leaked B's token: {a}");

    // Tenant B's namespace sees only B's content.
    let b = client
        .get(format!("{engine}/streams/s1?ns=userB"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(b.contains("from-B"), "B should see its own token: {b}");
    assert!(!b.contains("from-A"), "B leaked A's token: {b}");
}
