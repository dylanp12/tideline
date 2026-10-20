//! Audit finding 1: on a Cloud deployment with no subscribe token set — the
//! documented default — record reads were unauthenticated and the tenant came
//! from a caller-supplied `?ns=`. These tests are the fix's specification.

use tideline::tlr::auth::{resolve_read, resolve_write, Access, AuthMode};

fn granted(ns: &str) -> Access {
    Access::Granted { ns: ns.to_string() }
}

#[test]
fn wide_open_only_when_nothing_at_all_is_configured() {
    // Local development, and nothing else.
    let mode = AuthMode::default();
    assert_eq!(resolve_read(&mode, None, None), granted(""));
    assert_eq!(resolve_write(&mode, None), granted(""));
}

#[test]
fn a_cloud_deployment_without_a_subscribe_token_still_denies_reads() {
    // The exact configuration the audit found: TIDELINE_AUTH_URL set,
    // TIDELINE_SUBSCRIBE_TOKEN unset. Previously this read as "open".
    let mode = AuthMode {
        cloud_verify: true,
        ..Default::default()
    };
    assert_eq!(resolve_read(&mode, None, None), Access::Denied);
}

#[test]
fn a_publish_token_alone_also_closes_reads() {
    let mode = AuthMode {
        publish_token: Some("secret".into()),
        ..Default::default()
    };
    assert_eq!(resolve_read(&mode, None, None), Access::Denied);
    assert_eq!(resolve_read(&mode, Some("secret"), None), granted(""));
}

#[test]
fn the_subscribe_token_grants_reads() {
    let mode = AuthMode {
        subscribe_token: Some("ro".into()),
        ..Default::default()
    };
    assert_eq!(resolve_read(&mode, Some("ro"), None), granted(""));
    assert_eq!(resolve_read(&mode, Some("wrong"), None), Access::Denied);
}

#[test]
fn a_verified_tenant_supplies_the_namespace() {
    let mode = AuthMode {
        cloud_verify: true,
        ..Default::default()
    };
    assert_eq!(resolve_read(&mode, None, Some("org_42")), granted("org_42"));
}

#[test]
fn a_caller_cannot_choose_its_own_namespace() {
    // `?ns=` is gone from the record routes. This test exists so it cannot come
    // back: the namespace is a function of the credential alone.
    let mode = AuthMode {
        cloud_verify: true,
        ..Default::default()
    };
    let a = resolve_read(&mode, None, Some("org_a"));
    let b = resolve_read(&mode, None, Some("org_b"));
    assert_ne!(a, b);
    assert_eq!(a, granted("org_a"));
}

#[test]
fn writes_deny_without_a_credential_when_any_auth_is_configured() {
    let mode = AuthMode {
        publish_token: Some("secret".into()),
        ..Default::default()
    };
    assert_eq!(resolve_write(&mode, None), Access::Denied);
    assert_eq!(resolve_write(&mode, Some("secret")), granted(""));
}

#[test]
fn a_subscribe_token_does_not_grant_writes() {
    let mode = AuthMode {
        subscribe_token: Some("ro".into()),
        ..Default::default()
    };
    assert_eq!(resolve_write(&mode, Some("ro")), Access::Denied);
}

// --- the finding, end to end over HTTP -------------------------------------

use tideline::api::AuthConfig;
use tideline::manager::StreamManager;

async fn serve(auth: AuthConfig) -> String {
    let app = tideline::api::router_with_config(StreamManager::new(), auth);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// The configuration the audit found: a Cloud deployment authenticating through
/// TIDELINE_AUTH_URL with TIDELINE_SUBSCRIBE_TOKEN unset, which the README
/// documents as optional. Every record read used to be served to anyone.
fn cloud_shaped() -> AuthConfig {
    AuthConfig {
        publish_token: None,
        subscribe_token: None,
        // Unreachable on purpose: the unauthenticated paths under test must be
        // refused before any call to the control plane.
        verify_url: Some("http://127.0.0.1:1/verify".into()),
        verify_secret: None,
    }
}

#[tokio::test]
async fn a_cloud_deployment_serves_no_record_to_an_anonymous_caller() {
    let base = serve(cloud_shaped()).await;
    let client = reqwest::Client::new();

    for path in [
        "/v1/runs",
        "/v1/runs/any-run",
        "/v1/runs/any-run/events",
        "/v1/runs/any-run/approvals",
        "/v1/runs/any-run/approvals/0",
        "/v1/runs/any-run/checkpoint",
        "/v1/runs/any-run/watch",
    ] {
        let r = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(
            r.status(),
            401,
            "{path} must not be readable without a credential"
        );
    }
}

#[tokio::test]
async fn a_cloud_deployment_refuses_anonymous_writes_too() {
    let base = serve(cloud_shaped()).await;
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{base}/v1/runs"))
        .json(&serde_json::json!({
            "run_id": "r1", "agent": { "name": "a", "version": "1" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn there_is_no_ns_parameter_to_abuse() {
    // The old routes took the tenant from `?ns=`. Supplying one now changes
    // nothing: it is not a credential, so the request is still refused.
    let base = serve(cloud_shaped()).await;
    let client = reqwest::Client::new();
    let r = client
        .get(format!(
            "{base}/v1/runs/any-run/events?ns=some-other-tenant"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}
