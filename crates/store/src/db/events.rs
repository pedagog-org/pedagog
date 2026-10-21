//! Live updates: a trigger (migration 0015) announces every grading run's and
//! build's status change on [`CHANNEL`] as JSON: `{"kind":"run", run,
//! submission, assignment, user, email, status, score, total}` or `{"kind":"build",
//! build, assignment, build_status, validation_status}`.

/// The Postgres `NOTIFY` channel the api listens on.
pub const CHANNEL: &str = "pedagog_events";

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sqlx::PgPool;
    use sqlx::postgres::PgListener;

    use super::*;
    use crate::db::assignments::finish_build;
    use crate::db::submissions::tests::{current_build, submit};
    use crate::db::submissions::{RunOutcome, finish_run, start_grading};

    async fn next(listener: &mut PgListener) -> String {
        tokio::time::timeout(Duration::from_secs(5), listener.recv())
            .await
            .expect("an event")
            .expect("a notification")
            .payload()
            .to_owned()
    }

    #[sqlx::test]
    async fn status_changes_are_announced(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let mut listener = PgListener::connect_with(&pool).await?;
        listener.listen(CHANNEL).await?;

        let acc = submit(&pool, "s1", build).await?;
        let queued = next(&mut listener).await;
        assert!(
            queued.contains(r#""kind" : "run""#) && queued.contains(r#""status" : "queued""#),
            "{queued}"
        );
        assert!(
            queued.contains(&format!(r#""submission" : {}"#, acc.submission_id)),
            "{queued}"
        );

        let mut conn = pool.acquire().await?;
        start_grading(
            &mut conn,
            acc.run_id,
            "job",
            pedagog_core::recipe::assignment::Isolation::Process,
        )
        .await?;
        assert!(
            next(&mut listener)
                .await
                .contains(r#""status" : "grading""#)
        );
        finish_run(
            &pool,
            acc.run_id,
            &RunOutcome {
                score: Some((9.0, 10.0)),
                ..Default::default()
            },
        )
        .await?;
        let finished = next(&mut listener).await;
        // No results here, so it finishes as an error; the score still rides along.
        assert!(
            finished.contains(r#""status" : "error""#) && finished.contains(r#""score" : 9"#),
            "{finished}"
        );

        // A change that isn't a status (here a no-op re-finish) says nothing.
        sqlx::query("UPDATE grading_runs SET log_tail = 'x' WHERE id = $1")
            .bind(acc.run_id)
            .execute(&pool)
            .await?;
        assert!(
            tokio::time::timeout(Duration::from_millis(300), listener.recv())
                .await
                .is_err()
        );

        let other = current_build(&pool, "ll2", "img-b").await?;
        let _ = finish_build(&pool, other, true, None).await?;
        let mut kinds = Vec::new();
        while let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(500), listener.recv()).await
        {
            kinds.push(n.payload().to_owned());
        }
        assert!(
            kinds.iter().any(|k| k.contains(r#""kind" : "build""#)),
            "{kinds:?}"
        );
        Ok(())
    }
}
