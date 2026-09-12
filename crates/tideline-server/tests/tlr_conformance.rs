//! Runs `conformance/transcript/basic.json` against a live server.
//!
//! The TypeScript, Python, and Go SDKs run this same file. A protocol change
//! that one implementation misses becomes a failing build in all of them rather
//! than a support ticket months later.

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use tideline::manager::StreamManager;
use tideline_proto::{verify_chain, RunEvent};

async fn serve() -> String {
    let app = tideline::api::router_with(StreamManager::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn transcript() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/transcript/basic.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read transcript"))
        .expect("transcript is valid JSON")
}

/// Substitute `$VAR` occurrences in a string.
fn subst_str(s: &str, vars: &HashMap<String, Value>) -> String {
    let mut out = s.to_string();
    for (k, v) in vars {
        let replacement = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out = out.replace(&format!("${k}"), &replacement);
    }
    out
}

/// Substitute recursively. A string that is exactly `$VAR` takes the variable's
/// JSON value, so a captured number stays a number rather than becoming "4".
fn subst(v: &Value, vars: &HashMap<String, Value>) -> Value {
    match v {
        Value::String(s) => {
            if let Some(name) = s.strip_prefix('$') {
                if let Some(val) = vars.get(name) {
                    return val.clone();
                }
            }
            Value::String(subst_str(s, vars))
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, vars)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), subst(x, vars)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

#[tokio::test]
async fn the_transcript_passes_against_a_live_server() {
    let base = serve().await;
    let client = reqwest::Client::new();
    let script = transcript();

    let mut vars: HashMap<String, Value> = HashMap::new();
    vars.insert(
        "RUN".into(),
        json!(format!("conformance-{}", std::process::id())),
    );

    for step in script["steps"].as_array().expect("steps array") {
        let name = step["name"].as_str().unwrap_or("<unnamed>");
        let req = &step["request"];
        let method = req["method"].as_str().unwrap_or("GET");
        let path = subst_str(req["path"].as_str().expect("path"), &vars);
        let repeat = step["repeat"].as_u64().unwrap_or(1);

        let mut last: Option<(reqwest::StatusCode, reqwest::header::HeaderMap, String)> = None;
        for _ in 0..repeat {
            let mut rb = match method {
                "POST" => client.post(format!("{base}{path}")),
                "GET" => client.get(format!("{base}{path}")),
                other => panic!("step {name}: unsupported method {other}"),
            };
            if let Some(headers) = req["headers"].as_object() {
                for (k, v) in headers {
                    rb = rb.header(k.as_str(), v.as_str().unwrap_or_default());
                }
            }
            if let Some(body) = req.get("body") {
                rb = rb.json(&subst(body, &vars));
            }
            let res = rb
                .send()
                .await
                .unwrap_or_else(|e| panic!("step {name}: {e}"));
            let status = res.status();
            let headers = res.headers().clone();
            let text = res.text().await.unwrap_or_default();
            last = Some((status, headers, text));
        }
        let (status, headers, text) = last.expect("at least one request");
        let expect = &step["expect"];

        if let Some(want) = expect["status"].as_u64() {
            assert_eq!(
                status.as_u16() as u64,
                want,
                "step {name}: expected {want}, got {status}. Body: {text}"
            );
        }

        if let Some(want) = expect["header_eq"].as_object() {
            for (k, v) in want {
                let got = headers.get(k).and_then(|h| h.to_str().ok()).unwrap_or("");
                assert_eq!(
                    got,
                    v.as_str().unwrap_or_default(),
                    "step {name}: header {k}"
                );
            }
        }

        // Only parse a body when the step asserts something about it, so error
        // steps can assert a status without needing JSON.
        let needs_body = expect.get("json_has").is_some()
            || expect.get("json_eq").is_some()
            || expect.get("array_len").is_some()
            || expect.get("chain_verifies").is_some()
            || step.get("capture").is_some();
        if !needs_body {
            continue;
        }
        let body: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("step {name}: body is not JSON ({e}): {text}"));

        if let Some(keys) = expect["json_has"].as_array() {
            for k in keys {
                let k = k.as_str().unwrap();
                assert!(
                    body.get(k).is_some(),
                    "step {name}: missing key {k} in {body}"
                );
            }
        }

        if let Some(want) = expect["json_eq"].as_object() {
            for (k, v) in want {
                let expected = subst(v, &vars);
                assert_eq!(
                    body.get(k),
                    Some(&expected),
                    "step {name}: {k} mismatch in {body}"
                );
            }
        }

        if let Some(n) = expect["array_len"].as_u64() {
            let arr = body
                .as_array()
                .unwrap_or_else(|| panic!("step {name}: expected an array, got {body}"));
            assert_eq!(arr.len() as u64, n, "step {name}: array length");
        }

        if expect["chain_verifies"].as_bool() == Some(true) {
            // From the response text, never from `body`. Going through a
            // generic JSON value re-serialises metadata with its keys sorted,
            // which changes the digest and fails a record that is perfectly
            // sound. This is the mistake an SDK author makes first.
            let events: Vec<RunEvent> = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("step {name}: not a record ({e})"));
            let ok = verify_chain(&events)
                .unwrap_or_else(|e| panic!("step {name}: chain does not verify: {e:?}"));
            assert!(ok.sealed, "step {name}: expected a sealed record");
        }

        if let Some(capture) = step["capture"].as_object() {
            for (var, field) in capture {
                let field = field.as_str().unwrap();
                let value = body
                    .get(field)
                    .unwrap_or_else(|| panic!("step {name}: cannot capture missing {field}"));
                vars.insert(var.clone(), value.clone());
            }
        }
    }
}
