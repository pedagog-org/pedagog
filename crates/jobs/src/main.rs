//! Long-lived jobs service: builds OS base images in-cluster via Kaniko.
//!
//! Startup (prod) runs one dispatch pass over the pinned recipes, then idles
//! serving the rebuild endpoint. Dev waits for the endpoint (live checkout).

mod config;
mod dispatch;
mod http;
mod state;

use std::sync::Arc;

use pedagog_core::env::Env;
use pedagog_k8s::KubeClient;
use pedagog_k8s::build::Builder;
use pedagog_store::db;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    tracing::info!(env = ?config.env, namespace = %config.namespace, "starting pedagog-jobs");

    let pool = db::connect(&config.database_url).await?;
    db::run_migrations(&pool).await?;

    let kube = KubeClient::connect(&config.namespace).await?;
    let builder = Arc::new(Builder::new(kube, config.build_env()));

    let state = AppState {
        pool,
        builder,
        config: Arc::new(config),
        dispatch_lock: Arc::new(Mutex::new(())),
    };

    // prod builds the pinned recipes at startup; dev waits for the endpoint
    // (recipes are a live checkout being edited).
    if state.config.env == Env::Prod {
        match dispatch::dispatch(&state, false, None).await {
            Ok(d) => tracing::info!(dispatched = ?d.dispatched, "startup dispatch complete"),
            Err(e) => tracing::error!(error = %e, "startup dispatch failed"),
        }
    }

    let addr = state.config.listen_addr.clone();
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, http::router(state)).await?;
    Ok(())
}
