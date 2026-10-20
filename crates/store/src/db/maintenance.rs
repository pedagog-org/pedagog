//! Scheduled maintenance: claiming a day's run once across replicas, and what
//! it needs to know (pushes in progress, how big the databases are).

use sqlx::PgPool;

/// Claim `task`'s run for `day` (UTC, `YYYY-MM-DD`); false if a replica already did.
pub async fn claim(pool: &PgPool, task: &str, day: &str) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "INSERT INTO maintenance_runs (task, day) VALUES ($1, $2::text::date) ON CONFLICT DO NOTHING",
        task,
        day
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Image builds (assignment and OS) whose Kaniko push may be in progress.
pub async fn pushes_in_progress(pool: &PgPool) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT (SELECT count(*) FROM assignment_builds WHERE build_status = 'running')
                + (SELECT count(*) FROM build_runs WHERE status = 'running') AS "n!""#
    )
    .fetch_one(pool)
    .await
}

/// Every database's size in this Postgres, in bytes (its data, not WAL).
pub async fn database_bytes(pool: &PgPool) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT COALESCE(sum(pg_database_size(datname)), 0)::bigint AS "bytes!" FROM pg_database"#
    )
    .fetch_one(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::assignments::{finish_build, start_build};

    #[sqlx::test]
    async fn a_day_is_claimed_once(pool: PgPool) -> sqlx::Result<()> {
        assert!(claim(&pool, "registry-cleanup", "2026-10-21").await?);
        assert!(!claim(&pool, "registry-cleanup", "2026-10-21").await?);
        assert!(claim(&pool, "registry-cleanup", "2026-10-22").await?);
        assert!(claim(&pool, "other", "2026-10-21").await?);
        Ok(())
    }

    #[sqlx::test]
    async fn pushes_are_running_builds(pool: PgPool) -> sqlx::Result<()> {
        assert_eq!(pushes_in_progress(&pool).await?, 0);
        let a = crate::db::test_assignment(&pool, "ll").await?;
        let up = crate::db::assignments::record_upload(&pool, a.id, "t", "k", 1).await?;
        let b = start_build(&pool, &crate::db::test_build(&up, "img", "reg/a:1")).await?;
        assert_eq!(pushes_in_progress(&pool).await?, 1);
        finish_build(&pool, b, true, None).await?;
        assert_eq!(
            pushes_in_progress(&pool).await?,
            0,
            "validating doesn't push"
        );
        assert!(database_bytes(&pool).await? > 0);
        Ok(())
    }
}
