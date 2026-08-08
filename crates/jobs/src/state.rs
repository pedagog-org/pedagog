//! Shared, cheaply-clonable service state (handed to axum + the dispatch pass).

use std::sync::Arc;

use pedagog_k8s::build::Builder;
use sqlx::PgPool;
use tokio::sync::Mutex;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub builder: Arc<Builder>,
    pub config: Arc<Config>,
    /// Serializes the *dispatch decision* (detect → ensure → start_run); released
    /// before builds are awaited, so overlapping triggers don't block on builds.
    pub dispatch_lock: Arc<Mutex<()>>,
}
