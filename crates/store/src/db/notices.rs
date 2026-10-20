//! The notices outbox: emails to students, rendered when queued and sent by
//! the api at a bounded rate. A sender leases due notices (so replicas don't
//! send one twice), then marks each sent, or failed with a backoff, giving up
//! after the last retry.

use std::str::FromStr;

use sqlx::PgPool;

use crate::{Notice, NoticeKind};

/// Waits before each retry; after the last, the notice is given up on.
pub const RETRY_AFTER_SECS: [i64; 3] = [60, 300, 1800];
/// How long a lease keeps other senders off a notice being sent.
const LEASE_SECS: f64 = 300.0;

#[derive(Clone, Debug)]
pub struct NewNotice<'a> {
    pub user_id: i64,
    pub assignment_id: Option<i64>,
    pub kind: NoticeKind,
    pub subject: &'a str,
    pub body: &'a str,
}

/// Queue a notice. Returns its id.
pub async fn queue(pool: &PgPool, n: &NewNotice<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO notices (user_id, assignment_id, kind, subject, body)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
        n.user_id,
        n.assignment_id,
        n.kind.to_string(),
        n.subject,
        n.body
    )
    .fetch_one(pool)
    .await
}

/// A finished grading run whose student may need telling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnnotifiedRun {
    pub run_id: i64,
    pub submission_id: i64,
    pub assignment_id: i64,
    pub user_id: Option<i64>,
    /// Its regrade asked to tell students even when nothing changed.
    pub always: bool,
}

/// Finished runs marked `notify` that haven't been dealt with yet.
pub async fn unnotified_runs(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<UnnotifiedRun>> {
    sqlx::query_as!(
        UnnotifiedRun,
        r#"SELECT r.id AS "run_id!", s.id AS "submission_id!", s.assignment_id AS "assignment_id!",
                  s.user_id, COALESCE(b.notify = 'always', false) AS "always!"
             FROM grading_runs r JOIN submissions s ON s.id = r.submission_id
             LEFT JOIN regrade_batches b ON b.id = r.regrade_batch_id
          WHERE r.notify AND r.notified_at IS NULL AND r.status IN ('graded', 'error')
          ORDER BY r.id LIMIT $1"#,
        limit
    )
    .fetch_all(pool)
    .await
}

/// Deal with a run: queue its notice (if any) and mark it notified, at once.
pub async fn settle_run(
    pool: &PgPool,
    run: i64,
    notice: Option<&NewNotice<'_>>,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    if let Some(n) = notice {
        sqlx::query!(
            "INSERT INTO notices (user_id, assignment_id, kind, grading_run_id, subject, body)
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (grading_run_id) DO NOTHING",
            n.user_id,
            n.assignment_id,
            n.kind.to_string(),
            run,
            n.subject,
            n.body
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        "UPDATE grading_runs SET notified_at = now() WHERE id = $1",
        run
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Lease up to `limit` due notices for sending.
pub async fn lease_due(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<Notice>> {
    sqlx::query_as!(
        Notice,
        r#"UPDATE notices n SET send_after = now() + make_interval(secs => $2)
             FROM users u
            WHERE u.id = n.user_id
              AND n.id IN (SELECT id FROM notices
                            WHERE sent_at IS NULL AND gave_up_at IS NULL AND send_after <= now()
                            ORDER BY send_after, id LIMIT $1 FOR UPDATE SKIP LOCKED)
        RETURNING n.id, u.email, n.subject, n.body, n.attempts"#,
        limit,
        LEASE_SECS
    )
    .fetch_all(pool)
    .await
}

pub async fn sent(pool: &PgPool, id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE notices SET sent_at = now(), attempts = attempts + 1, last_error = NULL WHERE id = $1",
        id
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed send: retry later, or give up after the last retry.
/// Returns whether it was given up on.
pub async fn failed(
    pool: &PgPool,
    id: i64,
    attempts_before: i32,
    error: &str,
) -> sqlx::Result<bool> {
    let retry = usize::try_from(attempts_before)
        .ok()
        .and_then(|n| RETRY_AFTER_SECS.get(n))
        .copied();
    sqlx::query!(
        "UPDATE notices
            SET attempts = attempts + 1, last_error = $2,
                send_after = now() + make_interval(secs => $3),
                gave_up_at = CASE WHEN $4 THEN now() END
          WHERE id = $1",
        id,
        error,
        retry.unwrap_or(0) as f64,
        retry.is_none()
    )
    .execute(pool)
    .await?;
    Ok(retry.is_none())
}

/// A notice's state (for tests and, later, the portal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeState {
    pub kind: NoticeKind,
    pub email: String,
    pub subject: String,
    pub body: String,
    pub sent: bool,
    pub gave_up: bool,
    pub attempts: i32,
}

/// Notices about an assignment, oldest first.
pub async fn for_assignment(pool: &PgPool, assignment_id: i64) -> sqlx::Result<Vec<NoticeState>> {
    sqlx::query!(
        r#"SELECT n.kind, u.email, n.subject, n.body, n.sent_at IS NOT NULL AS "sent!",
                  n.gave_up_at IS NOT NULL AS "gave_up!", n.attempts
             FROM notices n JOIN users u ON u.id = n.user_id
            WHERE n.assignment_id = $1 ORDER BY n.id"#,
        assignment_id
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| {
        Ok(NoticeState {
            kind: NoticeKind::from_str(&r.kind).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
            email: r.email,
            subject: r.subject,
            body: r.body,
            sent: r.sent,
            gave_up: r.gave_up,
            attempts: r.attempts,
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::accounts::upsert_user;

    #[sqlx::test]
    async fn notices_are_leased_sent_retried_and_given_up(pool: PgPool) -> sqlx::Result<()> {
        let a = crate::db::test_assignment(&pool, "ll").await?;
        let alice = upsert_user(&pool, "alice@ufl.edu", "").await?;
        let n = |subject: &'static str| NewNotice {
            user_id: alice.id,
            assignment_id: Some(a.id),
            kind: NoticeKind::Manual,
            subject,
            body: "hi",
        };
        let first = queue(&pool, &n("one")).await?;
        let second = queue(&pool, &n("two")).await?;
        let leased = lease_due(&pool, 10).await?;
        assert_eq!(
            leased.iter().map(|l| l.id).collect::<Vec<_>>(),
            [first, second]
        );
        assert_eq!(leased[0].email, "alice@ufl.edu");
        assert!(
            lease_due(&pool, 10).await?.is_empty(),
            "leased notices aren't due"
        );

        sent(&pool, first).await?;
        assert!(!failed(&pool, second, 0, "451 try later").await?);
        assert!(lease_due(&pool, 10).await?.is_empty(), "waiting to retry");
        // The last retry fails: given up on.
        assert!(
            failed(
                &pool,
                second,
                RETRY_AFTER_SECS.len() as i32,
                "550 no such user"
            )
            .await?
        );
        let states = for_assignment(&pool, a.id).await?;
        assert_eq!(
            states
                .iter()
                .map(|s| (s.sent, s.gave_up))
                .collect::<Vec<_>>(),
            [(true, false), (false, true)]
        );
        Ok(())
    }

    #[sqlx::test]
    async fn runs_to_tell_are_settled_once(pool: PgPool) -> sqlx::Result<()> {
        use crate::db::submissions::tests::{current_build, submit};
        use crate::db::submissions::{RunOutcome, finish_run, start_grading};
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "s1", build).await?;
        let mut conn = pool.acquire().await?;
        start_grading(&mut conn, acc.run_id, "g", crate::Isolation::Process).await?;
        finish_run(
            &pool,
            acc.run_id,
            &RunOutcome {
                score: Some((1.0, 1.0)),
                results: Some("{}"),
                notify: true,
                ..Default::default()
            },
        )
        .await?;
        let runs = unnotified_runs(&pool, 10).await?;
        assert_eq!(runs.len(), 1);
        let alice = upsert_user(&pool, "alice@ufl.edu", "").await?;
        let notice = NewNotice {
            user_id: alice.id,
            assignment_id: Some(runs[0].assignment_id),
            kind: NoticeKind::Regrade,
            subject: "updated",
            body: "b",
        };
        settle_run(&pool, acc.run_id, Some(&notice)).await?;
        settle_run(&pool, acc.run_id, Some(&notice)).await?;
        assert!(unnotified_runs(&pool, 10).await?.is_empty());
        assert_eq!(
            for_assignment(&pool, runs[0].assignment_id).await?.len(),
            1,
            "once per run"
        );
        Ok(())
    }
}
