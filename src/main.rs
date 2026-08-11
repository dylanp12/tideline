use std::sync::Arc;
use std::time::Duration;
use tideline::api::{self, AuthConfig};
use tideline::backend::MemoryBackend;
use tideline::manager::StreamManager;
use tideline::redis_backend::{RedisBackend, RedisConfig};

#[tokio::main]
async fn main() {
    let auth = AuthConfig::from_env();
    if auth.publish_token.is_none() && auth.verify_url.is_none() {
        eprintln!(
            "⚠  No TIDELINE_PUBLISH_TOKEN or TIDELINE_AUTH_URL set — publishing is OPEN (dev mode). \
             Set one before exposing this server publicly."
        );
    }
    if let Some(u) = &auth.verify_url {
        println!("tideline: Cloud key verification enabled ({u})");
    }

    // TIDELINE_REDIS_URL set -> horizontally scalable, multi-instance Redis
    // backend. Unset -> the single-instance in-memory engine.
    let app = match std::env::var("TIDELINE_REDIS_URL") {
        Ok(url) if !url.is_empty() => {
            let backend = RedisBackend::connect(&url, RedisConfig::default())
                .await
                .expect("failed to connect to TIDELINE_REDIS_URL");
            println!("tideline: Redis backend @ {url} (horizontally scalable)");
            api::router_with_backend(Arc::new(backend), auth)
        }
        _ => {
            let manager = StreamManager::new();
            // Reap idle/completed streams so the registry doesn't grow unbounded.
            manager.spawn_reaper(Duration::from_secs(30));
            println!("tideline: in-memory backend (single instance)");
            api::router_with_backend(Arc::new(MemoryBackend::new(manager)), auth)
        }
    };

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    println!("tideline listening on http://0.0.0.0:8080");
    axum::serve(listener, app).await.unwrap();
}
