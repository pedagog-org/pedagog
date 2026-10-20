//! Shared, cheaply-clonable service state.

use std::sync::Arc;

use sqlx::PgPool;

use crate::auth::Authenticator;
use crate::config::Config;
use crate::mail::Mailer;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub mailer: Mailer,
    pub auth: Authenticator,
    /// For jobs' internal endpoints.
    pub jobs: reqwest::Client,
    /// Live updates for the open event streams.
    pub events: crate::events::Hub,
}
