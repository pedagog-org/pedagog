//! `submissions` and `grading_runs` queries.
//!
//! A grading run moves queued → grading → graded | error. Accepting a
//! submission records it, its first run and the run's queue entry in one
//! transaction. Writes that finish a run only apply to runs still grading, so
//! a late Job can't overwrite a run reconcile already ended.

use std::str::FromStr;

use sqlx::{PgConnection, PgPool};

use crate::{GradingRun, RunReason, RunStatus, Submission, SubmissionSource};

/// Queue priority of a submission's first run (validations share it).
pub const SUBMISSION_PRIORITY: i16 = 0;
/// Queue priority of regrade runs: behind new submissions.
pub const REGRADE_PRIORITY: i16 = 1;

/// A submission intake accepted, pinned to `build_id`.
#[derive(Clone, Debug)]
pub struct NewSubmission<'a> {
    pub assignment_id: &'a str,
    pub student_id: &'a str,
    pub build_id: i64,
    pub source: SubmissionSource,
    pub tree_hash: &'a str,
    pub object_key: &'a str,
    pub size_bytes: i64,
    /// What intake did, as JSON text.
    pub receipt: &'a str,
}

/// Ids of an accepted submission and its first grading run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub submission_id: i64,
    pub run_id: i64,
}

/// Record a submission and queue its first grading run.
pub async fn record_submission(pool: &PgPool, s: &NewSubmission<'_>) -> sqlx::Result<Accepted> {
    let mut tx = pool.begin().await?;
    let submission_id = sqlx::query!(
        "INSERT INTO submissions
           (assignment_id, student_id, build_id, source, tree_hash, object_key, size_bytes, receipt)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::text::jsonb)
         RETURNING id",
        s.assignment_id,
        s.student_id,
        s.build_id,
        s.source.to_string(),
        s.tree_hash,
        s.object_key,
        s.size_bytes,
        s.receipt
    )
    .fetch_one(&mut *tx)
    .await?
    .id;
    let run_id = sqlx::query!(
        "INSERT INTO grading_runs (submission_id, build_id, reason) VALUES ($1, $2, $3) RETURNING id",
        submission_id,
        s.build_id,
        RunReason::Submission.to_string()
    )
    .fetch_one(&mut *tx)
    .await?
    .id;
    sqlx::query!(
        "INSERT INTO run_queue (grading_run_id, priority) VALUES ($1, $2)",
        run_id,
        SUBMISSION_PRIORITY
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Accepted {
        submission_id,
        run_id,
    })
}

/// Mark an admitted run as grading in its Job, recording the isolation it
/// runs with. Use the connection that claimed it from the queue.
pub async fn start_grading(
    conn: &mut PgConnection,
    run: i64,
    job_name: &str,
    isolation: &str,
) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "UPDATE grading_runs
            SET status = $2, job_name = $3, isolation = $4, started_at = now()
          WHERE id = $1 AND status = 'queued'",
        run,
        RunStatus::Grading.to_string(),
        job_name,
        isolation
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// How a run ended.
#[derive(Clone, Debug, Default)]
pub struct RunOutcome<'a> {
    /// `(score, total)` for graded runs.
    pub score: Option<(f64, f64)>,
    /// The parsed results as JSON text; `None` for errors (untrusted).
    pub results: Option<&'a str>,
    pub problems: &'a [String],
    pub error: Option<&'a str>,
    pub log_tail: Option<&'a str>,
    /// Regrades: the difference from the previous graded run, as JSON text.
    pub changes: Option<&'a str>,
    pub notify: bool,
}

/// End a grading run as graded (`results` present) or error. Returns false if
/// the run isn't grading (e.g. reconcile already ended it).
pub async fn finish_run(pool: &PgPool, run: i64, outcome: &RunOutcome<'_>) -> sqlx::Result<bool> {
    let status = if outcome.results.is_some() {
        RunStatus::Graded
    } else {
        RunStatus::Error
    };
    let done = sqlx::query!(
        "UPDATE grading_runs
            SET status = $2, score = $3, total = $4, results = $5::text::jsonb, problems = $6,
                error = $7, log_tail = $8, changes = $9::text::jsonb, notify = $10,
                finished_at = now()
          WHERE id = $1 AND status = 'grading'",
        run,
        status.to_string(),
        outcome.score.map(|s| s.0),
        outcome.score.map(|s| s.1),
        outcome.results,
        outcome.problems,
        outcome.error,
        outcome.log_tail,
        outcome.changes,
        outcome.notify
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// One submission by id.
pub async fn get_submission(pool: &PgPool, id: i64) -> sqlx::Result<Option<Submission>> {
    sqlx::query_as!(
        SubmissionRow,
        "SELECT id, assignment_id, student_id, build_id, source, tree_hash, object_key,
                size_bytes, receipt::text AS \"receipt!\"
           FROM submissions WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await?
    .map(Submission::try_from)
    .transpose()
}

/// A student's submissions to an assignment, latest first.
pub async fn submissions_for_student(
    pool: &PgPool,
    assignment_id: &str,
    student_id: &str,
) -> sqlx::Result<Vec<Submission>> {
    sqlx::query_as!(
        SubmissionRow,
        "SELECT id, assignment_id, student_id, build_id, source, tree_hash, object_key,
                size_bytes, receipt::text AS \"receipt!\"
           FROM submissions WHERE assignment_id = $1 AND student_id = $2
          ORDER BY id DESC",
        assignment_id,
        student_id
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(Submission::try_from)
    .collect()
}

/// One grading run by id.
pub async fn get_run(pool: &PgPool, run: i64) -> sqlx::Result<Option<GradingRun>> {
    sqlx::query_as!(
        RunRow,
        r#"SELECT id, submission_id, build_id, reason, regrade_batch_id, status, isolation,
                  job_name, score, total, results::text AS "results?", problems, error,
                  log_tail, changes::text AS "changes?", notify
             FROM grading_runs WHERE id = $1"#,
        run
    )
    .fetch_optional(pool)
    .await?
    .map(GradingRun::try_from)
    .transpose()
}

/// A submission's runs, latest first (the first is the one shown).
pub async fn runs_for_submission(pool: &PgPool, submission: i64) -> sqlx::Result<Vec<GradingRun>> {
    sqlx::query_as!(
        RunRow,
        r#"SELECT id, submission_id, build_id, reason, regrade_batch_id, status, isolation,
                  job_name, score, total, results::text AS "results?", problems, error,
                  log_tail, changes::text AS "changes?", notify
             FROM grading_runs WHERE submission_id = $1
            ORDER BY id DESC"#,
        submission
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(GradingRun::try_from)
    .collect()
}

/// The submission's latest graded run before `run`: what a regrade compares against.
pub async fn previous_graded_run(
    pool: &PgPool,
    submission: i64,
    run: i64,
) -> sqlx::Result<Option<GradingRun>> {
    sqlx::query_as!(
        RunRow,
        r#"SELECT id, submission_id, build_id, reason, regrade_batch_id, status, isolation,
                  job_name, score, total, results::text AS "results?", problems, error,
                  log_tail, changes::text AS "changes?", notify
             FROM grading_runs
            WHERE submission_id = $1 AND id < $2 AND status = 'graded'
            ORDER BY id DESC
            LIMIT 1"#,
        submission,
        run
    )
    .fetch_optional(pool)
    .await?
    .map(GradingRun::try_from)
    .transpose()
}

/// Every run grading in a Job: what reconcile adopts after a restart.
pub async fn grading_in_flight(pool: &PgPool) -> sqlx::Result<Vec<GradingRun>> {
    sqlx::query_as!(
        RunRow,
        r#"SELECT id, submission_id, build_id, reason, regrade_batch_id, status, isolation,
                  job_name, score, total, results::text AS "results?", problems, error,
                  log_tail, changes::text AS "changes?", notify
             FROM grading_runs WHERE status = 'grading'
            ORDER BY id"#
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(GradingRun::try_from)
    .collect()
}

fn decode(e: strum::ParseError) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(e))
}

struct SubmissionRow {
    id: i64,
    assignment_id: String,
    student_id: String,
    build_id: i64,
    source: String,
    tree_hash: String,
    object_key: String,
    size_bytes: i64,
    receipt: String,
}

impl TryFrom<SubmissionRow> for Submission {
    type Error = sqlx::Error;

    fn try_from(row: SubmissionRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.id,
            assignment_id: row.assignment_id,
            student_id: row.student_id,
            build_id: row.build_id,
            source: SubmissionSource::from_str(&row.source).map_err(decode)?,
            tree_hash: row.tree_hash,
            object_key: row.object_key,
            size_bytes: row.size_bytes,
            receipt: row.receipt,
        })
    }
}

struct RunRow {
    id: i64,
    submission_id: i64,
    build_id: i64,
    reason: String,
    regrade_batch_id: Option<i64>,
    status: String,
    isolation: Option<String>,
    job_name: Option<String>,
    score: Option<f64>,
    total: Option<f64>,
    results: Option<String>,
    problems: Vec<String>,
    error: Option<String>,
    log_tail: Option<String>,
    changes: Option<String>,
    notify: bool,
}

impl TryFrom<RunRow> for GradingRun {
    type Error = sqlx::Error;

    fn try_from(row: RunRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.id,
            submission_id: row.submission_id,
            build_id: row.build_id,
            reason: RunReason::from_str(&row.reason).map_err(decode)?,
            regrade_batch_id: row.regrade_batch_id,
            status: RunStatus::from_str(&row.status).map_err(decode)?,
            isolation: row.isolation,
            job_name: row.job_name,
            score: row.score,
            total: row.total,
            results: row.results,
            problems: row.problems,
            error: row.error,
            log_tail: row.log_tail,
            changes: row.changes,
            notify: row.notify,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db::assignments::{
        finish_build, finish_validation, record_upload, start_build, upsert_assignment,
    };
    use crate::db::queue::claim_next;
    use crate::{QueuedRun, ValidationStatus};

    /// An assignment with a current (validated) build; returns the build.
    pub(crate) async fn current_build(pool: &PgPool, id: &str, image: &str) -> sqlx::Result<i64> {
        upsert_assignment(pool, id, "Linked List").await?;
        let up =
            record_upload(pool, id, image, &format!("assignments/{id}/{image}.tgz"), 1).await?;
        let build = start_build(pool, id, up.id, image, "reg/a:1").await?;
        finish_build(pool, build, true, None).await?;
        finish_validation(pool, build, ValidationStatus::Skipped, None, &[]).await?;
        Ok(build)
    }

    pub(crate) async fn submit(
        pool: &PgPool,
        assignment: &str,
        student: &str,
        build: i64,
    ) -> sqlx::Result<Accepted> {
        record_submission(
            pool,
            &NewSubmission {
                assignment_id: assignment,
                student_id: student,
                build_id: build,
                source: SubmissionSource::Upload,
                tree_hash: "t",
                object_key: "submissions/ll/t.tgz",
                size_bytes: 10,
                receipt: r#"{"files": ["linked_list.py"]}"#,
            },
        )
        .await
    }

    #[sqlx::test]
    async fn a_submission_queues_its_first_run(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "ll", "s1", build).await?;

        let sub = get_submission(&pool, acc.submission_id).await?.unwrap();
        assert_eq!(
            (sub.build_id, sub.source),
            (build, SubmissionSource::Upload)
        );
        assert!(sub.receipt.contains("linked_list.py"));
        let runs = runs_for_submission(&pool, acc.submission_id).await?;
        assert_eq!(runs.len(), 1);
        assert_eq!(
            (runs[0].id, runs[0].status, runs[0].reason),
            (acc.run_id, RunStatus::Queued, RunReason::Submission)
        );
        assert_eq!(submissions_for_student(&pool, "ll", "s1").await?, [sub]);
        assert!(submissions_for_student(&pool, "ll", "s2").await?.is_empty());

        let mut conn = pool.acquire().await?;
        assert_eq!(
            claim_next(&mut conn).await?,
            Some(QueuedRun::Grading(acc.run_id))
        );
        Ok(())
    }

    #[sqlx::test]
    async fn runs_finish_once_and_only_from_grading(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "ll", "s1", build).await?;
        let graded = RunOutcome {
            score: Some((7.5, 10.0)),
            results: Some(r#"{"summary": {"score": 7.5, "total": 10}}"#),
            ..Default::default()
        };
        assert!(
            !finish_run(&pool, acc.run_id, &graded).await?,
            "still queued"
        );

        let mut conn = pool.acquire().await?;
        assert!(start_grading(&mut conn, acc.run_id, "grade-1", "process").await?);
        assert!(!start_grading(&mut conn, acc.run_id, "grade-1", "process").await?);
        assert_eq!(grading_in_flight(&pool).await?.len(), 1);
        assert!(finish_run(&pool, acc.run_id, &graded).await?);
        assert!(!finish_run(&pool, acc.run_id, &RunOutcome::default()).await?);

        let run = get_run(&pool, acc.run_id).await?.unwrap();
        assert_eq!(run.status, RunStatus::Graded);
        assert_eq!((run.score, run.total), (Some(7.5), Some(10.0)));
        assert_eq!(run.isolation.as_deref(), Some("process"));
        assert!(grading_in_flight(&pool).await?.is_empty());
        Ok(())
    }

    #[sqlx::test]
    async fn errors_keep_no_results(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "ll", "s1", build).await?;
        let mut conn = pool.acquire().await?;
        start_grading(&mut conn, acc.run_id, "grade-1", "in-process").await?;
        let problems = vec!["results.yaml is missing".to_string()];
        let outcome = RunOutcome {
            problems: &problems,
            error: Some("timed out"),
            log_tail: Some("Killed"),
            ..Default::default()
        };
        finish_run(&pool, acc.run_id, &outcome).await?;
        let run = get_run(&pool, acc.run_id).await?.unwrap();
        assert_eq!(run.status, RunStatus::Error);
        assert_eq!(run.results, None);
        assert_eq!(run.problems, problems);
        assert_eq!(run.log_tail.as_deref(), Some("Killed"));
        Ok(())
    }

    #[sqlx::test]
    async fn previous_graded_run_skips_errors(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "ll", "s1", build).await?;
        let mut conn = pool.acquire().await?;
        start_grading(&mut conn, acc.run_id, "g", "process").await?;
        let graded = RunOutcome {
            score: Some((5.0, 10.0)),
            results: Some("{}"),
            ..Default::default()
        };
        finish_run(&pool, acc.run_id, &graded).await?;
        assert_eq!(
            previous_graded_run(&pool, acc.submission_id, acc.run_id + 100)
                .await?
                .map(|r| r.id),
            Some(acc.run_id)
        );
        assert_eq!(
            previous_graded_run(&pool, acc.submission_id, acc.run_id).await?,
            None
        );
        Ok(())
    }
}
