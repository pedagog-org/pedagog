//! `submissions` and `grading_runs` queries.
//!
//! A grading run moves queued → grading → graded | error. Accepting a
//! submission records it, its first run and the run's queue entry in one
//! transaction. Writes that finish a run only apply to runs still grading, so
//! a late Job can't overwrite a run reconcile already ended.

use std::str::FromStr;

use sqlx::{PgConnection, PgPool};

use crate::{
    ErrorKind, GradingRun, Isolation, Notify, RunReason, RunStatus, Submission, SubmissionSource,
};

/// `grading_runs` as [`RunRow`] reads it: one copy of the column list for
/// every query, still checked at compile time (sqlx joins `+`-separated
/// literals).
macro_rules! select_runs {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        sqlx::query_as!(
            RunRow,
            r#"SELECT id, submission_id, build_id, reason, regrade_batch_id, status, isolation,
                      job_name, score, total, results::text AS "results?", problems, error,
                      error_kind, log_tail, changes::text AS "changes?", notify
                 FROM grading_runs "# + $tail
            $(, $arg)*
        )
    };
}

/// Queue priority of a submission's first run (validations share it).
pub const SUBMISSION_PRIORITY: i16 = 0;
/// Queue priority of regrade runs: behind new submissions.
pub const REGRADE_PRIORITY: i16 = 1;

/// Whose a submission is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner<'a> {
    User(i64),
    /// Operator submissions: an opaque id.
    Student(&'a str),
}

/// A submission intake accepted, pinned to `build_id`.
#[derive(Clone, Debug)]
pub struct NewSubmission<'a> {
    pub assignment_id: i64,
    pub owner: Owner<'a>,
    /// Who sent it, if not the owner.
    pub submitted_by: Option<i64>,
    pub staff: bool,
    /// RFC 3339; `None` counts it now.
    pub counted_at: Option<&'a str>,
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
    let (user_id, student_id) = match s.owner {
        Owner::User(u) => (Some(u), None),
        Owner::Student(id) => (None, Some(id)),
    };
    let submission_id = sqlx::query!(
        "INSERT INTO submissions
           (assignment_id, user_id, student_id, submitted_by, staff, counted_at,
            build_id, source, tree_hash, object_key, size_bytes, receipt)
         VALUES ($1, $2, $3, $4, $5, COALESCE($6::text::timestamptz, now()),
                 $7, $8, $9, $10, $11, $12::text::jsonb)
         RETURNING id",
        s.assignment_id,
        user_id,
        student_id,
        s.submitted_by,
        s.staff,
        s.counted_at,
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
    crate::db::queue::wake(&mut *tx).await?;
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
    isolation: Isolation,
) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "UPDATE grading_runs
            SET status = $2, job_name = $3, isolation = $4, started_at = now()
          WHERE id = $1 AND status = 'queued'",
        run,
        RunStatus::Grading.to_string(),
        job_name,
        isolation.to_string()
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
    /// Why it errored (`None` when graded).
    pub error_kind: Option<ErrorKind>,
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
                error_kind = $11, finished_at = now()
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
        outcome.notify,
        outcome.error_kind.map(|k| k.to_string())
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// End a run that can't be graded (it can never start, or its watcher
/// failed) as an error. Returns false if it had already finished.
pub async fn abandon_run(pool: &PgPool, run: i64, error: &str) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "UPDATE grading_runs SET status = $2, error = $3, error_kind = $4, finished_at = now()
          WHERE id = $1 AND status IN ('queued', 'grading')",
        run,
        RunStatus::Error.to_string(),
        error,
        ErrorKind::Internal.to_string()
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// When a regrade batch tells students about its results.
pub async fn batch_notify(pool: &PgPool, batch: i64) -> sqlx::Result<Option<Notify>> {
    let row = sqlx::query!("SELECT notify FROM regrade_batches WHERE id = $1", batch)
        .fetch_optional(pool)
        .await?;
    row.map(|r| Notify::from_str(&r.notify).map_err(decode))
        .transpose()
}

macro_rules! select_submissions {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        sqlx::query_as!(
            SubmissionRow,
            r#"SELECT s.id, s.assignment_id, s.user_id, s.student_id, u.email AS "user_email?",
                      b.email AS "submitted_by?", s.staff,
                      to_char(s.counted_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS "counted_at!",
                      to_char(s.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS "created_at!",
                      s.build_id, s.source, s.tree_hash, s.object_key, s.size_bytes,
                      s.receipt::text AS "receipt!"
                 FROM submissions s
                 LEFT JOIN users u ON u.id = s.user_id
                 LEFT JOIN users b ON b.id = s.submitted_by "# + $tail
            $(, $arg)*
        )
    };
}

/// One submission by id.
pub async fn get_submission(pool: &PgPool, id: i64) -> sqlx::Result<Option<Submission>> {
    select_submissions!("WHERE s.id = $1", id)
        .fetch_optional(pool)
        .await?
        .map(Submission::try_from)
        .transpose()
}

/// An operator student's submissions to an assignment, latest first.
pub async fn submissions_for_student(
    pool: &PgPool,
    assignment_id: i64,
    student_id: &str,
) -> sqlx::Result<Vec<Submission>> {
    select_submissions!(
        "WHERE s.assignment_id = $1 AND s.student_id = $2 ORDER BY s.counted_at DESC, s.id DESC",
        assignment_id,
        student_id
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(Submission::try_from)
    .collect()
}

/// A user's submissions to an assignment, latest (by counted time) first.
pub async fn submissions_for_user(
    pool: &PgPool,
    assignment_id: i64,
    user: i64,
) -> sqlx::Result<Vec<Submission>> {
    select_submissions!(
        "WHERE s.assignment_id = $1 AND s.user_id = $2 ORDER BY s.counted_at DESC, s.id DESC",
        assignment_id,
        user
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(Submission::try_from)
    .collect()
}

/// What it takes to start a grading run: its submission and the build it runs against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunContext {
    pub run_id: i64,
    pub submission_id: i64,
    pub assignment_id: i64,
    /// The assignment's course and slug (for names in the cluster).
    pub course_id: String,
    pub slug: String,
    /// An instructor's or grader's own submission (never notified).
    pub staff: bool,
    pub build_id: i64,
    pub image_ref: String,
    pub upload_id: i64,
    /// The submission's stored tree.
    pub object_key: String,
    pub tree_hash: String,
    /// Set for regrade runs.
    pub regrade_batch_id: Option<i64>,
}

pub async fn run_context(pool: &PgPool, run: i64) -> sqlx::Result<Option<RunContext>> {
    sqlx::query_as!(
        RunContext,
        "SELECT g.id AS run_id, s.id AS submission_id, s.assignment_id, a.course_id, a.slug,
                s.staff, b.id AS build_id, b.image_ref, b.upload_id, s.object_key, s.tree_hash,
                g.regrade_batch_id
           FROM grading_runs g
           JOIN submissions s ON s.id = g.submission_id
           JOIN assignments a ON a.id = s.assignment_id
           JOIN assignment_builds b ON b.id = g.build_id
          WHERE g.id = $1",
        run
    )
    .fetch_optional(pool)
    .await
}

/// One grading run by id.
pub async fn get_run(pool: &PgPool, run: i64) -> sqlx::Result<Option<GradingRun>> {
    select_runs!("WHERE id = $1", run)
        .fetch_optional(pool)
        .await?
        .map(GradingRun::try_from)
        .transpose()
}

/// A submission's runs, latest first (the first is the one shown).
pub async fn runs_for_submission(pool: &PgPool, submission: i64) -> sqlx::Result<Vec<GradingRun>> {
    select_runs!("WHERE submission_id = $1 ORDER BY id DESC", submission)
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
    select_runs!(
        "WHERE submission_id = $1 AND id < $2 AND status = 'graded' ORDER BY id DESC LIMIT 1",
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
    select_runs!("WHERE status = 'grading' ORDER BY id")
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
    assignment_id: i64,
    user_id: Option<i64>,
    student_id: Option<String>,
    user_email: Option<String>,
    submitted_by: Option<String>,
    staff: bool,
    counted_at: String,
    created_at: String,
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
            user_id: row.user_id,
            student_id: row.student_id,
            user_email: row.user_email,
            submitted_by: row.submitted_by,
            staff: row.staff,
            counted_at: row.counted_at,
            created_at: row.created_at,
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
    error_kind: Option<String>,
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
            isolation: row
                .isolation
                .map(|i| Isolation::from_str(&i))
                .transpose()
                .map_err(decode)?,
            job_name: row.job_name,
            score: row.score,
            total: row.total,
            results: row.results,
            problems: row.problems,
            error: row.error,
            error_kind: row
                .error_kind
                .map(|k| ErrorKind::from_str(&k))
                .transpose()
                .map_err(decode)?,
            log_tail: row.log_tail,
            changes: row.changes,
            notify: row.notify,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db::assignments::{finish_build, finish_validation, record_upload, start_build};
    use crate::db::queue::claim_next;
    use crate::{QueuedRun, ValidationStatus};

    /// An assignment with a current (validated) build; returns the build.
    pub(crate) async fn current_build(pool: &PgPool, slug: &str, image: &str) -> sqlx::Result<i64> {
        let a = crate::db::test_assignment(pool, slug).await?;
        let up = record_upload(
            pool,
            a.id,
            image,
            &format!("assignments/c/{slug}/{image}.tgz"),
            1,
        )
        .await?;
        let build = start_build(pool, &crate::db::test_build(&up, image, "reg/a:1")).await?;
        finish_build(pool, build, true, None).await?;
        finish_validation(pool, build, ValidationStatus::Skipped, None, &[]).await?;
        Ok(build)
    }

    /// A submission by `student` against `build` (of its assignment).
    pub(crate) async fn submit(pool: &PgPool, student: &str, build: i64) -> sqlx::Result<Accepted> {
        let assignment = sqlx::query_scalar!(
            "SELECT assignment_id FROM assignment_builds WHERE id = $1",
            build
        )
        .fetch_one(pool)
        .await?;
        record_submission(
            pool,
            &NewSubmission {
                assignment_id: assignment,
                owner: Owner::Student(student),
                submitted_by: None,
                staff: false,
                counted_at: None,
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
        let acc = submit(&pool, "s1", build).await?;

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
        let a = sub.assignment_id;
        assert_eq!(submissions_for_student(&pool, a, "s1").await?, [sub]);
        assert!(submissions_for_student(&pool, a, "s2").await?.is_empty());

        let mut conn = pool.acquire().await?;
        assert_eq!(
            claim_next(&mut conn).await?,
            Some(QueuedRun::Grading(acc.run_id))
        );
        Ok(())
    }

    #[sqlx::test]
    async fn users_submissions_carry_who_and_when(pool: PgPool) -> sqlx::Result<()> {
        use crate::db::accounts::upsert_user;
        let build = current_build(&pool, "ll", "img-a").await?;
        let a = sqlx::query_scalar!(
            "SELECT assignment_id FROM assignment_builds WHERE id = $1",
            build
        )
        .fetch_one(&pool)
        .await?;
        let alice = upsert_user(&pool, "alice@ufl.edu", "").await?;
        let prof = upsert_user(&pool, "prof@ufl.edu", "").await?;
        let new = |by: Option<i64>, at: Option<&'static str>| NewSubmission {
            assignment_id: a,
            owner: Owner::User(alice.id),
            submitted_by: by,
            staff: false,
            counted_at: at,
            build_id: build,
            source: SubmissionSource::Upload,
            tree_hash: "t",
            object_key: "k",
            size_bytes: 1,
            receipt: "{}",
        };
        let own = record_submission(&pool, &new(None, None)).await?;
        // A fix on her behalf, counted before her own.
        let fix = record_submission(
            &pool,
            &new(Some(prof.id), Some("2026-09-08T23:41:00-04:00")),
        )
        .await?;
        let mine = submissions_for_user(&pool, a, alice.id).await?;
        assert_eq!(
            mine.iter().map(|s| s.id).collect::<Vec<_>>(),
            [own.submission_id, fix.submission_id],
            "latest by counted time"
        );
        let f = &mine[1];
        assert_eq!(
            (
                f.user_email.as_deref(),
                f.submitted_by.as_deref(),
                f.counted_at.as_str()
            ),
            (
                Some("alice@ufl.edu"),
                Some("prof@ufl.edu"),
                "2026-09-09T03:41:00.000000Z"
            )
        );
        assert_ne!(f.counted_at, f.created_at);
        // Counted to the microsecond: "at their last time" ties exactly, and the
        // later submission comes first.
        let again = record_submission(
            &pool,
            &NewSubmission {
                counted_at: Some(mine[0].counted_at.as_str()),
                ..new(Some(prof.id), None)
            },
        )
        .await?;
        assert_eq!(
            submissions_for_user(&pool, a, alice.id).await?[0].id,
            again.submission_id
        );
        assert!(submissions_for_user(&pool, a, prof.id).await?.is_empty());

        let both = sqlx::query!(
            "INSERT INTO submissions (assignment_id, user_id, student_id, build_id, source, tree_hash, object_key, size_bytes)
             VALUES ($1, $2, 's', $3, 'upload', 't', 'k', 1)",
            a,
            alice.id,
            build
        )
        .execute(&pool)
        .await;
        assert!(both.is_err(), "a user or a student id, not both");
        Ok(())
    }

    #[sqlx::test]
    async fn runs_finish_once_and_only_from_grading(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "s1", build).await?;
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
        assert!(start_grading(&mut conn, acc.run_id, "grade-1", Isolation::Process).await?);
        assert!(!start_grading(&mut conn, acc.run_id, "grade-1", Isolation::Process).await?);
        assert_eq!(grading_in_flight(&pool).await?.len(), 1);
        assert!(finish_run(&pool, acc.run_id, &graded).await?);
        assert!(!finish_run(&pool, acc.run_id, &RunOutcome::default()).await?);

        let run = get_run(&pool, acc.run_id).await?.unwrap();
        assert_eq!(run.status, RunStatus::Graded);
        assert_eq!((run.score, run.total), (Some(7.5), Some(10.0)));
        assert_eq!(run.isolation, Some(Isolation::Process));
        assert!(grading_in_flight(&pool).await?.is_empty());
        Ok(())
    }

    #[sqlx::test]
    async fn run_context_joins_submission_and_build(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "s1", build).await?;
        let ctx = run_context(&pool, acc.run_id).await?.unwrap();
        assert_eq!(
            (
                ctx.submission_id,
                ctx.build_id,
                ctx.course_id.as_str(),
                ctx.slug.as_str()
            ),
            (acc.submission_id, build, "c", "ll")
        );
        assert_eq!(
            (ctx.image_ref.as_str(), ctx.tree_hash.as_str()),
            ("reg/a:1", "t")
        );
        assert_eq!(run_context(&pool, 999).await?, None);
        Ok(())
    }

    #[sqlx::test]
    async fn errors_keep_no_results(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "s1", build).await?;
        let mut conn = pool.acquire().await?;
        start_grading(&mut conn, acc.run_id, "grade-1", Isolation::InProcess).await?;
        let problems = vec!["results.yaml is missing".to_string()];
        let outcome = RunOutcome {
            problems: &problems,
            error: Some("timed out"),
            error_kind: Some(ErrorKind::Timeout),
            log_tail: Some("Killed"),
            ..Default::default()
        };
        finish_run(&pool, acc.run_id, &outcome).await?;
        let run = get_run(&pool, acc.run_id).await?.unwrap();
        assert_eq!(run.status, RunStatus::Error);
        assert_eq!(run.results, None);
        assert_eq!(run.problems, problems);
        assert_eq!(run.log_tail.as_deref(), Some("Killed"));
        assert_eq!(run.error_kind, Some(ErrorKind::Timeout));
        Ok(())
    }

    #[sqlx::test]
    async fn abandoned_runs_end_as_errors_once(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "s1", build).await?;
        assert!(abandon_run(&pool, acc.run_id, "couldn't start: no plan").await?);
        assert!(!abandon_run(&pool, acc.run_id, "again").await?);
        let run = get_run(&pool, acc.run_id).await?.unwrap();
        assert_eq!(run.status, RunStatus::Error);
        assert_eq!(run.error.as_deref(), Some("couldn't start: no plan"));
        assert_eq!(run.error_kind, Some(ErrorKind::Internal));
        Ok(())
    }

    #[sqlx::test]
    async fn batch_notify_reads_the_batch(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let batch = sqlx::query!(
            "INSERT INTO regrade_batches (assignment_id, build_id, notify, runs)
             SELECT assignment_id, id, 'changed', 0 FROM assignment_builds WHERE id = $1 RETURNING id",
            build
        )
        .fetch_one(&pool)
        .await?
        .id;
        assert_eq!(batch_notify(&pool, batch).await?, Some(Notify::Changed));
        assert_eq!(batch_notify(&pool, batch + 1).await?, None);
        Ok(())
    }

    #[sqlx::test]
    async fn previous_graded_run_skips_errors(pool: PgPool) -> sqlx::Result<()> {
        let build = current_build(&pool, "ll", "img-a").await?;
        let acc = submit(&pool, "s1", build).await?;
        let mut conn = pool.acquire().await?;
        start_grading(&mut conn, acc.run_id, "g", Isolation::Process).await?;
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
