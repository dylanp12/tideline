use futures::StreamExt;
use tideline::api::AuthConfig;
use tideline::manager::StreamManager;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TMsg;

// Spawn a server with the given router on an ephemeral port; return its base URL.
async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn open_app() -> axum::Router {
    tideline::api::router_with(StreamManager::new())
}

#[tokio::test]
async fn publish_then_subscribe_replays_and_completes() {
    let base = serve(open_app()).await;
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/streams/s1"))
        .body("hello ")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/streams/s1"))
        .body("world")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/streams/s1/complete"))
        .send()
        .await
        .unwrap();

    let resp = client
        .get(format!("{base}/streams/s1"))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert!(body.contains("id: 0"), "body was: {body}");
    assert!(body.contains("data: hello "), "body was: {body}");
    assert!(body.contains("data: world"), "body was: {body}");
    assert!(body.contains("event: done"), "body was: {body}");
}

#[tokio::test]
async fn resume_via_last_event_id_skips_seen() {
    let base = serve(open_app()).await;
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/streams/s2"))
        .body("a")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/streams/s2"))
        .body("b")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/streams/s2/complete"))
        .send()
        .await
        .unwrap();

    let resp = client
        .get(format!("{base}/streams/s2"))
        .header("Last-Event-ID", "0")
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert!(!body.contains("id: 0"), "body was: {body}");
    assert!(body.contains("id: 1"), "body was: {body}");
    assert!(body.contains("data: b"), "body was: {body}");
}

#[tokio::test]
async fn invalid_stream_id_is_rejected() {
    let base = serve(open_app()).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/streams/bad!id"))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn publish_requires_token_when_set() {
    let auth = AuthConfig {
        publish_token: Some("secret".into()),
        ..Default::default()
    };
    let base = serve(tideline::api::router_with_config(
        StreamManager::new(),
        auth,
    ))
    .await;
    let client = reqwest::Client::new();

    // No token -> 401
    let r = client
        .post(format!("{base}/streams/a"))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // Correct token -> 200
    let r = client
        .post(format!("{base}/streams/a"))
        .bearer_auth("secret")
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Subscribe is open (no subscribe token) -> 200
    client
        .post(format!("{base}/streams/a/complete"))
        .bearer_auth("secret")
        .send()
        .await
        .unwrap();
    let r = client
        .get(format!("{base}/streams/a"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn delete_endpoint_works() {
    let base = serve(open_app()).await;
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/streams/d"))
        .body("x")
        .send()
        .await
        .unwrap();
    let r = client
        .delete(format!("{base}/streams/d"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = client
        .delete(format!("{base}/streams/d"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn error_endpoint_emits_error_event() {
    let base = serve(open_app()).await;
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/streams/e"))
        .body("partial")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/streams/e/error"))
        .body("boom")
        .send()
        .await
        .unwrap();
    let body = client
        .get(format!("{base}/streams/e"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("data: partial"), "body: {body}");
    assert!(body.contains("event: stream_error"), "body: {body}");
    assert!(body.contains("data: boom"), "body: {body}");
}

#[tokio::test]
async fn metrics_endpoint_serves_prometheus() {
    let base = serve(open_app()).await;
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/streams/m"))
        .body("x")
        .send()
        .await
        .unwrap();
    let resp = client.get(format!("{base}/metrics")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("tideline_active_streams"), "body: {body}");
    assert!(
        body.contains("tideline_tokens_published_total"),
        "body: {body}"
    );
}

#[tokio::test]
async fn body_over_limit_is_rejected() {
    let base = serve(open_app()).await;
    let client = reqwest::Client::new();
    let big = "x".repeat(300 * 1024); // > 256KB
    let r = client
        .post(format!("{base}/streams/big"))
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
}

// Read a WebSocket stream to its terminal; returns (token datas, saw_done).
async fn read_ws(ws_url: String) -> (Vec<String>, bool) {
    let (mut socket, _) = connect_async(ws_url).await.unwrap();
    let mut datas = Vec::new();
    let mut done = false;
    while let Some(Ok(msg)) = socket.next().await {
        match msg {
            TMsg::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str() {
                    Some("token") => datas.push(v["data"].as_str().unwrap_or("").to_string()),
                    Some("done") => {
                        done = true;
                        break;
                    }
                    Some("error") => break,
                    _ => {}
                }
            }
            TMsg::Close(_) => break,
            _ => {}
        }
    }
    (datas, done)
}

#[tokio::test]
async fn websocket_streams_tokens_and_completes() {
    let base = serve(open_app()).await;
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/streams/wsa"))
        .body("hello ")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/streams/wsa"))
        .body("world")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/streams/wsa/complete"))
        .send()
        .await
        .unwrap();

    let ws_url = format!("{}/streams/wsa/ws?from=0", base.replace("http://", "ws://"));
    let (datas, done) = read_ws(ws_url).await;
    assert_eq!(datas, vec!["hello ", "world"]);
    assert!(done);
}

#[tokio::test]
async fn websocket_resumes_from_offset() {
    let base = serve(open_app()).await;
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/streams/wsb"))
        .body("a")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/streams/wsb"))
        .body("b")
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/streams/wsb/complete"))
        .send()
        .await
        .unwrap();

    let ws_url = format!("{}/streams/wsb/ws?from=1", base.replace("http://", "ws://"));
    let (datas, done) = read_ws(ws_url).await;
    assert_eq!(datas, vec!["b"]);
    assert!(done);
}
