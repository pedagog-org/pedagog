//! The admission queue (`run_queue`): validation and grading runs wait here
//! until the capacity gate releases them, in `(priority, queued_at)` order.
//!
//! Releasing is done in one transaction: take the admission lock (one replica
//! releases at a time), check capacity, `claim_next`, start the run, commit.

use sqlx::{PgConnection, PgPool};

use crate::QueuedRun;
use crate::ValidationStatus;
use crate::db::submissions::SUBMISSION_PRIORITY;

/// Advisory lock key serializing releases across `jobs` replicas.
const ADMISSION_LOCK: i64 = 0x7065_6461_676f_6701;

/// Queue a succeeded build's validation. Returns false if the attempt isn't
/// waiting to validate (finished, superseded, or already queued).
pub async fn enqueue_validation(pool: &PgPool, build: i64) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let done = sqlx::query!(
        "UPDATE assignment_builds SET validation_status = $2
          WHERE id = $1 AND finished_at IS NULL AND build_status = 'succeeded'
            AND validation_status = 'pending'",
        build,
        ValidationStatus::Queued.to_string()
    )
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query!(
        "INSERT INTO run_queue (build_id, priority) VALUES ($1, $2)",
        build,
        SUBMISSION_PRIORITY
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Take the admission lock for the rest of the caller's transaction. Returns
/// false if another replica holds it.
pub async fn try_admission_lock(conn: &mut PgConnection) -> sqlx::Result<bool> {
    let row = sqlx::query!(
        r#"SELECT pg_try_advisory_xact_lock($1) AS "locked!""#,
        ADMISSION_LOCK
    )
    .fetch_one(conn)
    .await?;
    Ok(row.locked)
}

/// Release the next waiting run, if any.
pub async fn claim_next(conn: &mut PgConnection) -> sqlx::Result<Option<QueuedRun>> {
    let row = sqlx::query!(
        "UPDATE run_queue SET released_at = now()
          WHERE id = (SELECT id FROM run_queue
                       WHERE released_at IS NULL
                       ORDER BY priority, queued_at, id
                       LIMIT 1
                       FOR UPDATE SKIP LOCKED)
          RETURNING build_id, grading_run_id"
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.and_then(|r| match (r.build_id, r.grading_run_id) {
        (Some(build), None) => Some(QueuedRun::Validation(build)),
        (None, Some(run)) => Some(QueuedRun::Grading(run)),
        _ => None,
    }))
}

/// 1-based position among waiting runs, or `None` once released (or never queued).
pub async fn position(pool: &PgPool, run: QueuedRun) -> sqlx::Result<Option<i64>> {
    let (build, grading) = match run {
        QueuedRun::Validation(b) => (Some(b), None),
        QueuedRun::Grading(g) => (None, Some(g)),
    };
    let row = sqlx::query!(
        r#"SELECT (SELECT count(*) FROM run_queue o
                    WHERE o.released_at IS NULL
                      AND (o.priority, o.queued_at, o.id) <= (q.priority, q.queued_at, q.id))
                  AS "position!"
             FROM run_queue q
            WHERE q.released_at IS NULL
              AND (q.build_id = $1 OR q.grading_run_id = $2)"#,
        build,
        grading
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.position))
}

/// Released runs that haven't finished: what the capacity gate counts while
/// their pods are being created.
pub async fn released_unfinished(pool: &PgPool) -> sqlx::Result<Vec<QueuedRun>> {
    let rows = sqlx::query!(
        "SELECT q.build_id, q.grading_run_id
           FROM run_queue q
           LEFT JOIN assignment_builds b ON b.id = q.build_id
           LEFT JOIN grading_runs g ON g.id = q.grading_run_id
          WHERE q.released_at IS NOT NULL
            AND (b.finished_at IS NULL AND q.build_id IS NOT NULL
                 OR g.status = 'grading')
          ORDER BY q.released_at"
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| match (r.build_id, r.grading_run_id) {
            (Some(build), None) => Some(QueuedRun::Validation(build)),
            (None, Some(run)) => Some(QueuedRun::Grading(run)),
            _ => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::assignments::{finish_build, record_upload, start_build, upsert_assignment};
    use crate::db::submissions::tests::{current_build, submit};

    async fn built(pool: &PgPool, image: &str) -> sqlx::Result<i64> {
        upsert_assignment(pool, "other", "Other").await?;
        let up = record_upload(pool, "other", image, "k", 1).await?;
        let build = start_build(
            pool,
            "other",
            up.id,
            image,
            "reg/o:1",
            &crate::db::test_settings(),
        )
        .await?;
        finish_build(pool, build, true, None).await?;
        Ok(build)
    }

    #[sqlx::test]
    async fn validation_is_queued_once(pool: PgPool) -> sqlx::Result<()> {
        let build = built(&pool, "img-o").await?;
        assert!(enqueue_validation(&pool, build).await?);
        assert!(!enqueue_validation(&pool, build).await?, "already queued");
        assert_eq!(
            position(&pool, QueuedRun::Validation(build)).await?,
            Some(1)
        );
        let mut conn = pool.acquire().await?;
        assert_eq!(
            claim_next(&mut conn).await?,
            Some(QueuedRun::Validation(build))
        );
        assert_eq!(position(&pool, QueuedRun::Validation(build)).await?, None);
        assert_eq!(
            released_unfinished(&pool).await?,
            [QueuedRun::Validation(build)]
        );
        Ok(())
    }

    #[sqlx::test]
    async fn released_in_priority_then_arrival_order(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let first = submit(&pool, "ll", "s1", build).await?;
        // A regrade queued before the next submission still goes after it.
        let regrade = submit(&pool, "ll", "s2", build).await?;
        sqlx::query!(
            "UPDATE run_queue SET priority = 1 WHERE grading_run_id = $1",
            regrade.run_id
        )
        .execute(&pool)
        .await?;
        let second = submit(&pool, "ll", "s3", build).await?;
        let validation = built(&pool, "img-o").await?;
        enqueue_validation(&pool, validation).await?;

        assert_eq!(
            position(&pool, QueuedRun::Grading(regrade.run_id)).await?,
            Some(4)
        );
        let mut conn = pool.acquire().await?;
        let mut order = vec![];
        while let Some(next) = claim_next(&mut conn).await? {
            order.push(next);
        }
        assert_eq!(
            order,
            [
                QueuedRun::Grading(first.run_id),
                QueuedRun::Grading(second.run_id),
                QueuedRun::Validation(validation),
                QueuedRun::Grading(regrade.run_id),
            ]
        );
        Ok(())
    }

    #[sqlx::test]
    async fn one_replica_holds_the_admission_lock(pool: PgPool) -> sqlx::Result<()> {
        let mut a = pool.begin().await?;
        let mut b = pool.begin().await?;
        assert!(try_admission_lock(&mut a).await?);
        assert!(!try_admission_lock(&mut b).await?);
        a.commit().await?;
        assert!(
            try_admission_lock(&mut b).await?,
            "released with the transaction"
        );
        Ok(())
    }

    #[sqlx::test]
    async fn concurrent_claims_never_share_a_run(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        submit(&pool, "ll", "s1", build).await?;
        submit(&pool, "ll", "s2", build).await?;
        let mut a = pool.begin().await?;
        let mut b = pool.begin().await?;
        let first = claim_next(&mut a).await?;
        let second = claim_next(&mut b).await?;
        assert!(first.is_some() && second.is_some());
        assert_ne!(first, second);
        Ok(())
    }
}
