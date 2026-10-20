//! The engine enforces Cloud-issued API keys by calling the control plane's
//! verify endpoint. Here we stand up a mock verify endpoint (standing in for
//! Tideline Cloud's /api/v1/verify) and prove the engine accepts keys it
//! approves, rejects the rest, and caches positive results.

use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
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

// Mock control plane: 200 only when the shared secret matches and the key is "goodkey".
async fn mock_verify() -> String {
    let app = Router::new().route(
        "/api/v1/verify",
        post(|headers: HeaderMap, body: String| async move {
            if headers
                .get("x-tideline-verify-secret")
                .and_then(|v| v.to_str().ok())
                != Some("s3cret")
            {
                return StatusCode::UNAUTHORIZED;
            }
            if body.contains("goodkey") {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        }),
    );
    serve(app).await
}

#[tokio::test]
async fn engine_enforces_cloud_issued_keys() {
    let verify_base = mock_verify().await;
    let auth = AuthConfig {
        verify_url: Some(format!("{verify_base}/api/v1/verify")),
        verify_secret: Some("s3cret".into()),
        ..Default::default()
    };
    let base = serve(router_with_config(StreamManager::new(), auth)).await;
    let client = reqwest::Client::new();

    // a key the control plane approves -> 200
    let r = client
        .post(format!("{base}/streams/s"))
        .bearer_auth("goodkey")
        .body("hi")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // an unknown key -> 401
    let r = client
        .post(format!("{base}/streams/s"))
        .bearer_auth("nope")
        .body("hi")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // no key at all -> 401
    let r = client
        .post(format!("{base}/streams/s"))
        .body("hi")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // second use of the good key still works (cache path) -> 200
    let r = client
        .post(format!("{base}/streams/s"))
        .bearer_auth("goodkey")
        .body("more")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}
