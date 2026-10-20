//! Private streams require a signed subscribe ticket; open `?ns=` reads of a
//! private stream are rejected, and only a correctly-signed, unexpired ticket
//! grants access.

use axum::routing::post;
use axum::{Json, Router};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
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

async fn mock_verify() -> String {
    let app = Router::new().route(
        "/verify",
        post(|| async {
            Json(serde_json::json!({ "valid": true, "userId": "userT", "orgId": null }))
        }),
    );
    serve(app).await
}

// Mint a ticket exactly as the control plane would.
fn mint(secret: &str, ns: &str, sid: &str, exp: u64) -> String {
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload = serde_json::json!({ "ns": ns, "sid": sid, "exp": exp }).to_string();
    let p = b64.encode(payload.as_bytes());
    let mut mac = <Hmac<Sha256>>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(p.as_bytes());
    let sig = b64.encode(mac.finalize().into_bytes());
    format!("{p}.{sig}")
}

#[tokio::test]
async fn private_streams_require_a_valid_ticket() {
    let vbase = mock_verify().await;
    let auth = AuthConfig {
        verify_url: Some(format!("{vbase}/verify")),
        verify_secret: Some("s".into()),
        ..Default::default()
    };
    let engine = serve(router_with_config(StreamManager::new(), auth)).await;
    let client = reqwest::Client::new();

    // publish a PRIVATE stream (tenant userT), then complete it
    client
        .post(format!("{engine}/streams/s1?private=1"))
        .bearer_auth("k")
        .body("TOP-SECRET")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{engine}/streams/s1/complete"))
        .bearer_auth("k")
        .send()
        .await
        .unwrap();

    // open ?ns= read of a private (even completed) stream -> 401
    let r = client
        .get(format!("{engine}/streams/s1?ns=userT"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        401,
        "open read of a private stream must be rejected"
    );

    // a valid, unexpired ticket -> content
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    let ticket = mint("s", "userT", "s1", exp);
    let body = client
        .get(format!("{engine}/streams/s1?ticket={ticket}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains("TOP-SECRET"),
        "a valid ticket should read the stream: {body}"
    );

    // expired ticket -> 401
    let expired = mint("s", "userT", "s1", 1);
    let r = client
        .get(format!("{engine}/streams/s1?ticket={expired}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "expired ticket must be rejected");

    // forged ticket (wrong secret) -> 401
    let forged = mint("wrong-secret", "userT", "s1", exp);
    let r = client
        .get(format!("{engine}/streams/s1?ticket={forged}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "forged ticket must be rejected");
}
