//! Postgres access (`feature = "db"`): generic pool/migration infra here, the
//! per-domain queries in submodules (`builds`).

use sqlx::PgPool;
use sqlx::migrate::MigrateError;
use sqlx::postgres::PgPoolOptions;

pub mod builds;

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
