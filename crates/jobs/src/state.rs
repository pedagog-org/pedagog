//! Shared, cheaply-clonable service state (handed to axum + the dispatch pass).

use std::collections::HashMap;
use std::sync::Arc;

use pedagog_k8s::build::Builder;
use pedagog_store::objects::Objects;
use sqlx::PgPool;
use tokio::sync::Mutex;

use crate::assignments::Plan;
use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub builder: Arc<Builder>,
    pub objects: Objects,
    pub config: Arc<Config>,
    /// Serializes the *dispatch decision* (detect → ensure → start_run); released
    /// before builds are awaited, so overlapping triggers don't block on builds.
    pub dispatch_lock: Arc<Mutex<()>>,
    /// Plans by build id, so admission doesn't re-derive one per run (filled
    /// at dispatch; re-derived from the stored archive after a restart).
    pub plans: Arc<std::sync::Mutex<HashMap<i64, Plan>>>,
}
