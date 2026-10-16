//! Shared, cheaply-clonable service state (handed to axum + the dispatch pass).

use std::sync::Arc;

use pedagog_k8s::build::Builder;
use pedagog_store::objects::Objects;
use sqlx::PgPool;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub builder: Arc<Builder>,
    pub objects: Objects,
    pub config: Arc<Config>,
}
