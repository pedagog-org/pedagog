//! Long-lived jobs service: builds OS base images in-cluster via Kaniko, and
//! takes in assignment uploads.
//!
//! Startup (prod) runs one dispatch pass over the pinned recipes, then idles
//! serving the rebuild endpoint. Dev waits for the endpoint (live checkout).

mod archive;
mod config;
mod dispatch;
mod http;
mod intake;
mod state;

use std::sync::Arc;

use pedagog_core::env::Env;
use pedagog_k8s::KubeClient;
use pedagog_k8s::build::Builder;
use pedagog_store::db;
use pedagog_store::objects::Objects;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

/// object_store's reqwest compiles in rustls' aws-lc-rs backend while sqlx and
/// kube use ring; with both present rustls can't choose a default and panics on
/// the first TLS client, so pick ring explicitly.
fn install_crypto_provider() {
    // Already installed (e.g. by another test) is fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    install_crypto_provider();
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
    let objects = Objects::connect(&config.objects)?;

    let state = AppState {
        pool,
        builder,
        objects,
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

#[cfg(test)]
mod tests {
    /// kube's rustls connector relies on rustls' default provider, which is
    /// ambiguous in this binary (object_store brings aws-lc-rs, sqlx/kube ring):
    /// without the explicit install, building a kube client panics.
    #[tokio::test]
    async fn kube_client_builds_with_both_crypto_backends_compiled_in() {
        super::install_crypto_provider();
        let config = kube::Config::new("https://127.0.0.1:6443".parse().unwrap());
        kube::Client::try_from(config).unwrap();
    }
}
