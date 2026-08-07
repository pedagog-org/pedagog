//! `os_builds` (current build per OS) + `build_runs` (attempt history) queries.

use sqlx::PgPool;

use crate::{BuildRun, BuildStatus};

/// Last successfully-built Containerfile hash for an OS — what change detection reads.
pub async fn last_hash(pool: &PgPool, os_id: &str) -> sqlx::Result<Option<String>> {
    let row = sqlx::query!(
        "SELECT containerfile_hash FROM os_builds WHERE os_id = $1",
        os_id
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.containerfile_hash))
}

/// Upsert the current successful build for an OS. Written only on success.
pub async fn record_build(
    pool: &PgPool,
    os_id: &str,
    hash: &str,
    image_ref: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO os_builds (os_id, containerfile_hash, image_ref, built_at)
         VALUES ($1, $2, $3, now())
         ON CONFLICT (os_id) DO UPDATE
           SET containerfile_hash = EXCLUDED.containerfile_hash,
               image_ref          = EXCLUDED.image_ref,
               built_at           = EXCLUDED.built_at",
        os_id,
        hash,
        image_ref
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Open a build attempt (`status = running`); returns the new run id.
///
/// Fails with a unique violation if the OS already has a running attempt (the
/// partial index) — the DB backstop for dispatch dedup.
pub async fn start_run(
    pool: &PgPool,
    os_id: &str,
    hash: &str,
    image_ref: &str,
) -> sqlx::Result<i64> {
    let row = sqlx::query!(
        "INSERT INTO build_runs (os_id, hash, image_ref, status)
         VALUES ($1, $2, $3, $4)
         RETURNING id",
        os_id,
        hash,
        image_ref,
        BuildStatus::Running.to_string()
    )
    .fetch_one(pool)
    .await?;
    Ok(row.id)
}

/// Finalize a build attempt, recording Kaniko logs in `error` on failure.
pub async fn finish_run(
    pool: &PgPool,
    run: i64,
    status: BuildStatus,
    error: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE build_runs
            SET status = $2, error = $3, finished_at = now()
          WHERE id = $1",
        run,
        status.to_string(),
        error
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Attempts still marked `running` — orphans to reconcile against the cluster.
pub async fn running(pool: &PgPool) -> sqlx::Result<Vec<BuildRun>> {
    let rows = sqlx::query!(
        "SELECT id, os_id, hash, image_ref
           FROM build_runs
          WHERE status = $1
          ORDER BY started_at",
        BuildStatus::Running.to_string()
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| BuildRun {
            id: r.id,
            os_id: r.os_id,
            hash: r.hash,
            image_ref: r.image_ref,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn os_build_upsert_and_read(pool: PgPool) -> sqlx::Result<()> {
        assert_eq!(last_hash(&pool, "ubuntu-22").await?, None);
        record_build(&pool, "ubuntu-22", "hash-a", "reg/ubuntu:22").await?;
        assert_eq!(last_hash(&pool, "ubuntu-22").await?, Some("hash-a".into()));
        // upsert overwrites
        record_build(&pool, "ubuntu-22", "hash-b", "reg/ubuntu:22").await?;
        assert_eq!(last_hash(&pool, "ubuntu-22").await?, Some("hash-b".into()));
        Ok(())
    }

    #[sqlx::test]
    async fn run_lifecycle_and_running(pool: PgPool) -> sqlx::Result<()> {
        let run = start_run(&pool, "ubuntu-22", "hash-a", "reg/ubuntu:22").await?;

        let orphans = running(&pool).await?;
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].id, run);
        assert_eq!(orphans[0].os_id, "ubuntu-22");
        assert_eq!(orphans[0].image_ref, "reg/ubuntu:22");

        finish_run(&pool, run, BuildStatus::Succeeded, None).await?;
        assert!(running(&pool).await?.is_empty());
        Ok(())
    }

    #[sqlx::test]
    async fn at_most_one_running_per_os(pool: PgPool) -> sqlx::Result<()> {
        start_run(&pool, "ubuntu-22", "h1", "reg/ubuntu:22").await?;
        // a second in-flight attempt for the same OS violates the partial index
        assert!(
            start_run(&pool, "ubuntu-22", "h2", "reg/ubuntu:22")
                .await
                .is_err()
        );
        Ok(())
    }
}
