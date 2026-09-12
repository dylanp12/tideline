use std::sync::Arc;
use std::time::Duration;
use tideline::api::{self, AuthConfig};
use tideline::backend::MemoryBackend;
use tideline::manager::StreamManager;
use tideline::redis_backend::{RedisBackend, RedisConfig};
use tracing::{error, info, warn};

/// Structured logs, JSON when `TIDELINE_LOG_FORMAT=json`.
///
/// An audit product whose own server logs are unstructured fails its own
/// security review.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if std::env::var("TIDELINE_LOG_FORMAT").as_deref() == Ok("json") {
        fmt().with_env_filter(filter).json().init();
    } else {
        fmt().with_env_filter(filter).init();
    }
}

/// Refuse to run a configuration that would fragment the record.
///
/// With `TIDELINE_REDIS_URL` set, streams span instances but records are held
/// in this process's SQLite file. Behind a load balancer each instance would
/// hold a fragment of every run and `GET /v1/runs/:id` would return whichever
/// piece it happened to own — audit finding 2. Shared record storage is not
/// implemented yet, so the only safe options are one writer or no start.
fn guard_record_storage(redis_configured: bool) {
    if !redis_configured {
        return;
    }
    if std::env::var("TIDELINE_ALLOW_LOCAL_RECORDS").is_ok() {
        warn!(
            "TIDELINE_REDIS_URL is set with local record storage, allowed by \
             TIDELINE_ALLOW_LOCAL_RECORDS. Records live only in this instance — \
             run exactly one writer, or runs will be split across instances."
        );
        return;
    }
    error!(
        "TIDELINE_REDIS_URL is set but records are stored in local SQLite. Behind a \
         load balancer each instance would hold a fragment of every run, and reads \
         would return whichever fragment they landed on. Set \
         TIDELINE_ALLOW_LOCAL_RECORDS=1 if this instance is the only writer."
    );
    std::process::exit(1);
}

/// Resolve when the process is asked to stop, so in-flight appends commit and
/// subscribers get a clean close instead of a connection reset.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                warn!(error = %e, "cannot listen for SIGTERM; Ctrl-C only");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    info!("shutdown signal received — draining");
}

#[tokio::main]
async fn main() {
    init_tracing();

    let auth = AuthConfig::from_env();
    if auth.publish_token.is_none() && auth.verify_url.is_none() {
        warn!(
            "No TIDELINE_PUBLISH_TOKEN or TIDELINE_AUTH_URL set — publishing is OPEN \
             (dev mode). Set one before exposing this server publicly."
        );
    }
    if let Some(u) = &auth.verify_url {
        info!(verify_url = %u, "Cloud key verification enabled");
    }

    let redis_url = std::env::var("TIDELINE_REDIS_URL")
        .ok()
        .filter(|s| !s.is_empty());
    guard_record_storage(redis_url.is_some());

    // Redis backend -> a fleet of stateless instances sharing streams.
    // Unset -> the single-instance in-memory engine.
    let app = match redis_url {
        Some(url) => {
            let backend = RedisBackend::connect(&url, RedisConfig::default())
                .await
                .expect("failed to connect to TIDELINE_REDIS_URL");
            info!(url = %url, "Redis backend (horizontally scalable)");
            api::router_with_backend(Arc::new(backend), auth)
        }
        None => {
            let manager = StreamManager::new();
            // Reap idle/completed streams so the registry doesn't grow unbounded.
            manager.spawn_reaper(Duration::from_secs(30));
            info!("in-memory backend (single instance)");
            api::router_with_backend(Arc::new(MemoryBackend::new(manager)), auth)
        }
    };

    // PORT is what every platform-as-a-service injects; TIDELINE_PORT wins when
    // both are set, so the engine can be pinned independently of a host's
    // convention.
    let port = std::env::var("TIDELINE_PORT")
        .or_else(|_| std::env::var("PORT"))
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8080);
    let addr = format!("0.0.0.0:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            error!(error = %e, addr, "cannot bind");
            std::process::exit(1);
        }
    };
    info!("tideline listening on http://{addr}");
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        error!(error = %e, "server stopped");
    }
}
