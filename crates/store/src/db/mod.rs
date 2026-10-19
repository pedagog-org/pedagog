//! Postgres access (`feature = "db"`): generic pool/migration infra here, the
//! per-domain queries in submodules (`accounts`, `builds`, `assignments`,
//! `submissions`, `queue`).

use sqlx::PgPool;
use sqlx::migrate::MigrateError;
use sqlx::postgres::PgPoolOptions;

pub mod accounts;
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

/// Namespace for [`lock`]'s keys among Postgres advisory locks.
const LOCK_CLASS: i32 = 1;

/// Hold a transaction-level advisory lock on `key` for the rest of the
/// caller's transaction, waiting until no other transaction holds it. Works
/// across replicas; released by commit or rollback.
pub async fn lock(conn: &mut sqlx::PgConnection, key: &str) -> sqlx::Result<()> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock($1, hashtext($2))",
        LOCK_CLASS,
        key
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Apply the embedded migrations (idempotent — safe to run at every startup).
pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

/// Run settings for builds made in tests.
#[cfg(test)]
pub(crate) fn test_settings() -> crate::BuildSettings {
    crate::BuildSettings {
        platform: crate::PlatformKind::Upload,
        isolation: crate::Isolation::Process,
        egress: r#"{"mode":"deny"}"#.into(),
        submission: "{}".into(),
    }
}

/// An assignment of course `c` (created as needed), for tests.
#[cfg(test)]
pub(crate) async fn test_assignment(pool: &PgPool, slug: &str) -> sqlx::Result<crate::Assignment> {
    sqlx::query!("INSERT INTO courses (id, name) VALUES ('c', 'C') ON CONFLICT DO NOTHING")
        .execute(pool)
        .await?;
    assignments::upsert_assignment(pool, "c", slug, "Linked List").await
}

/// A build attempt of an upload, with test settings and the default policy.
#[cfg(test)]
pub(crate) fn test_build<'a>(
    upload: &crate::AssignmentUpload,
    image_hash: &'a str,
    image_ref: &'a str,
) -> assignments::NewBuild<'a> {
    static SETTINGS: std::sync::LazyLock<crate::BuildSettings> =
        std::sync::LazyLock::new(test_settings);
    assignments::NewBuild {
        assignment_id: upload.assignment_id,
        upload_id: upload.id,
        image_hash,
        image_ref,
        settings: &SETTINGS,
        policy: "{}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn a_lock_is_held_until_the_transaction_ends(pool: PgPool) -> sqlx::Result<()> {
        let mut first = pool.begin().await?;
        lock(&mut first, "dispatch:ll").await?;

        let mut other = pool.begin().await?;
        lock(&mut other, "dispatch:hw2").await?; // another key doesn't wait
        other.commit().await?;

        let waiting = {
            let pool = pool.clone();
            tokio::spawn(async move {
                let mut second = pool.begin().await?;
                lock(&mut second, "dispatch:ll").await?;
                second.commit().await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!waiting.is_finished(), "the same key waits");
        first.commit().await?;
        tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
            .await
            .expect("released by the commit")
            .expect("task")?;
        Ok(())
    }
}
