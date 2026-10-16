//! Postgres access (`feature = "db"`): generic pool/migration infra here, the
//! per-domain queries in submodules (`builds`, `assignments`, `submissions`,
//! `queue`).

use sqlx::PgPool;
use sqlx::migrate::MigrateError;
use sqlx::postgres::PgPoolOptions;

pub mod assignments;
pub mod builds;
pub mod queue;
pub mod submissions;

/// Max pooled connections for a single service replica.
const MAX_CONNECTIONS: u32 = 5;

/// Connect to Postgres and build a bounded pool.
pub async fn connect(url: &str) -> sqlx::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect(url)
        .await
}

/// Apply the embedded migrations (idempotent — safe to run at every startup).
pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

/// Run settings for builds made in tests.
#[cfg(test)]
pub(crate) fn test_settings() -> crate::BuildSettings {
    crate::BuildSettings {
        platform: "upload".into(),
        isolation: "process".into(),
        egress: r#"{"mode":"deny"}"#.into(),
        submission: "{}".into(),
    }
}
