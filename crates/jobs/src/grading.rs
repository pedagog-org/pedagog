//! Grading runs: admission launches a run's Job (the pinned build's image,
//! the submission fetched into `/pedagog/submission`), then a watcher waits
//! for it and records the outcome. Runs in flight are watched again after a
//! restart ([`reconcile`]).

use std::time::Duration;

use pedagog_core::changes::compare;
use pedagog_core::results::Results;
use pedagog_k8s::build::JobKey;
use pedagog_k8s::grade::GradingRequest;
use pedagog_k8s::runs::Stopped;
use pedagog_store::db::assignments as builds_db;
use pedagog_store::db::submissions::{self as db, RunOutcome};
use pedagog_store::objects::grading_results_key;
use pedagog_store::{ErrorKind, RunStatus};
use sqlx::PgConnection;

use crate::admission::Launch;
use crate::assignments::job_id;
use crate::settings::settings_of;
use crate::state::AppState;

/// Extra validity for presigned URLs beyond the Job's deadline.
const URL_MARGIN: Duration = Duration::from_secs(600);
/// How many lines of a load error are kept (the error itself is at the end).
const ERROR_LINES: usize = 12;
/// How much of a failed pod's log is kept.
const LOG_TAIL: usize = 4000;

/// Start a released run: record it as grading and create its Job, in the
/// caller's release transaction.
pub async fn launch(state: &AppState, conn: &mut PgConnection, run: i64) -> Result<(), Launch> {
    let ctx = db::run_context(&state.pool, run)
        .await
        .map_err(Launch::transient)?
        .ok_or_else(|| Launch::permanent(format!("grading run {run} is missing")))?;
    let build = builds_db::build_by_id(&state.pool, ctx.build_id)
        .await
        .map_err(Launch::transient)?
        .ok_or_else(|| Launch::permanent(format!("build {} is missing", ctx.build_id)))?;
    let settings = settings_of(state, &build)
        .await
        .map_err(Launch::permanent)?;
    let key = JobKey::grading(&job_id(&ctx.course_id, &ctx.slug), run);
    let limits = &state.config.grading;
    let submission_url = state
        .objects
        .presign_get(&ctx.object_key, limits.deadline + URL_MARGIN)
        .await
        .map_err(Launch::transient)?;
    let results_url = state
        .objects
        .presign_put(&grading_results_key(run), limits.deadline + URL_MARGIN)
        .await
        .map_err(Launch::transient)?;
    if !db::start_grading(&mut *conn, run, &key.name(), settings.isolation)
        .await
        .map_err(Launch::transient)?
    {
        return Ok(()); // already started
    }
    let request = GradingRequest {
        run_id: run,
        image: ctx.image_ref,
        init_image: state.config.init_image.clone(),
        submission_url: submission_url.to_string(),
        tree_hash: ctx.tree_hash,
        results_url: results_url.to_string(),
        isolation: settings.isolation,
        egress: settings.egress,
        deadline: limits.deadline,
        cpu: limits.cpu.clone(),
        memory: limits.memory.clone(),
        disk: limits.disk.clone(),
    };
    let outcome = state
        .builder
        .ensure_grading(&key, &request)
        .await
        .map_err(Launch::kube)?;
    tracing::info!(run, course = %ctx.course_id, assignment = %ctx.slug, ?outcome, "grading Job ensured");
    Ok(())
}

/// Record the outcome of a grading run whose pod ended (or whose Job is
/// gone), from the results it uploaded. Does nothing if the run isn't grading
/// (already settled), so reports of the same pod can repeat.
pub async fn settle(
    state: &AppState,
    run: i64,
    succeeded: bool,
    stopped: Option<Stopped>,
) -> anyhow::Result<()> {
    let Some(current) = db::get_run(&state.pool, run).await? else {
        return Ok(());
    };
    if current.status != RunStatus::Grading {
        return Ok(());
    }
    let ctx = db::run_context(&state.pool, run)
        .await?
        .ok_or_else(|| anyhow::anyhow!("grading run {run} is missing"))?;
    let uploaded = state.objects.get(&grading_results_key(run)).await.ok();
    let mut verdict = judge(uploaded.as_deref());
    let mut log_tail = None;
    if verdict.results.is_none() && !succeeded {
        let logs = state
            .builder
            .logs(&JobKey::grading(&job_id(&ctx.course_id, &ctx.slug), run))
            .await;
        log_tail = Some(logs[logs.len().saturating_sub(LOG_TAIL)..].to_owned());
        let (kind, error) = match stopped {
            Some(Stopped::Deadline) => (ErrorKind::Timeout, "the run passed its time limit"),
            Some(Stopped::OutOfMemory) => (ErrorKind::OutOfMemory, "the run ran out of memory"),
            Some(Stopped::OutOfDisk) => (
                ErrorKind::OutOfDisk,
                "the run used more local disk than its limit",
            ),
            None => (ErrorKind::Crash, "the run stopped unexpectedly"),
        };
        verdict.kind = Some(kind);
        verdict.error = Some(error.to_owned());
    }
    let (changes, notify) = match ctx.regrade_batch_id {
        Some(batch) => regrade_changes(state, &ctx, batch, verdict.results.as_deref()).await?,
        None => (None, false),
    };
    let outcome = RunOutcome {
        score: verdict.score,
        results: verdict.results.as_deref(),
        problems: &verdict.problems,
        error: verdict.error.as_deref(),
        error_kind: verdict.kind,
        log_tail: log_tail.as_deref(),
        changes: changes.as_deref(),
        notify,
    };
    db::finish_run(&state.pool, run, &outcome).await?;
    match verdict.score {
        Some((score, total)) => tracing::info!(run, score, total, "graded"),
        None => tracing::warn!(run, problems = ?verdict.problems, "grading run errored"),
    }
    Ok(())
}

/// A regrade's difference from the submission's previous graded run, as
/// JSON, and whether its batch says to tell the student.
async fn regrade_changes(
    state: &AppState,
    ctx: &db::RunContext,
    batch: i64,
    results: Option<&str>,
) -> anyhow::Result<(Option<String>, bool)> {
    let parse = |json: Option<&str>| json.and_then(|j| serde_json::from_str::<Results>(j).ok());
    let previous = db::previous_graded_run(&state.pool, ctx.submission_id, ctx.run_id).await?;
    let before = parse(previous.as_ref().and_then(|r| r.results.as_deref()));
    let changes = compare(before.as_ref(), parse(results).as_ref());
    // Staff's own submissions are never notified.
    let notify = !ctx.staff
        && db::batch_notify(&state.pool, batch)
            .await?
            .is_some_and(|n| n.tells(&changes));
    Ok((serde_json::to_string(&changes).ok(), notify))
}

/// End a run that can't be graded (it never started, or its watcher failed).
pub async fn fail(state: &AppState, run: i64, error: &str) {
    if let Err(e) = db::abandon_run(&state.pool, run, error).await {
        tracing::error!(run, error = %e, "recording a failed grading run failed");
    }
}

/// What a grading run's results.yaml says.
#[derive(Debug, PartialEq)]
pub struct Verdict {
    /// The parsed results as JSON, when they can be trusted.
    pub results: Option<String>,
    pub score: Option<(f64, f64)>,
    pub problems: Vec<String>,
    pub error: Option<String>,
    /// Why it's an error.
    pub kind: Option<ErrorKind>,
}

/// Results that parse and pass the format checks are graded (failing tests
/// included); anything else is an error, and nothing from it is kept.
pub fn judge(uploaded: Option<&[u8]>) -> Verdict {
    let error = |kind: ErrorKind, problems: Vec<String>| Verdict {
        results: None,
        score: None,
        problems,
        error: None,
        kind: Some(kind),
    };
    let Some(bytes) = uploaded else {
        return error(
            ErrorKind::NoResults,
            vec!["no results.yaml was uploaded".into()],
        );
    };
    let results: Results = match serde_yaml::from_slice(bytes) {
        Ok(r) => r,
        Err(e) => {
            return error(
                ErrorKind::Invalid,
                vec![format!("results.yaml didn't parse: {e}")],
            );
        }
    };
    let problems = results.problems();
    if !problems.is_empty() {
        return error(ErrorKind::Invalid, problems);
    }
    // Tests that couldn't even run (e.g. a syntax error in the submission):
    // the run is an error, not a 0 out of 0.
    if let Some(output) = results.run_error() {
        return error(
            ErrorKind::Load,
            vec![format!(
                "the tests couldn't all run:\n{}",
                tail_lines(&output, ERROR_LINES)
            )],
        );
    }
    Verdict {
        score: Some((results.summary.score, results.summary.total)),
        results: serde_json::to_string(&results).ok(),
        problems: Vec::new(),
        error: None,
        kind: None,
    }
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../core/testdata/pedagog-pytest-results.json");

    #[test]
    fn results_are_graded_with_their_score() {
        let v = judge(Some(FIXTURE));
        assert!(v.problems.is_empty(), "{:?}", v.problems);
        let (score, total) = v.score.unwrap();
        assert!(total > 0.0 && score <= total);
        assert!(v.results.unwrap().contains("\"pop-front\""));
    }

    #[test]
    fn failing_tests_are_still_graded() {
        // Results where the reference solution would disagree are fine for a
        // student: only the format matters.
        let text = std::str::from_utf8(FIXTURE).unwrap().replacen(
            "\"status\": \"fail\"",
            "\"status\": \"pass\"",
            1,
        );
        assert!(judge(Some(text.as_bytes())).score.is_some());
    }

    #[test]
    fn tests_that_couldnt_load_make_an_error() {
        // What the helper reported for a submission with a syntax error.
        let broken = r#"{
            "summary": {"score": 0.0, "total": 0.0},
            "tests": [{
                "node": "test_linked_list.py::<collection>",
                "name": "Collecting test_linked_list.py",
                "status": "error", "score": 0.0, "max": 0.0,
                "output": "Traceback ...\nE     File \"linked_list.py\", line 1\nE       class LinkedList(:\nE   invalid syntax",
                "visibility": {"score": "visible", "output": "after_end"}
            }]
        }"#;
        let v = judge(Some(broken.as_bytes()));
        assert_eq!(
            (v.results, v.score, v.kind),
            (None, None, Some(ErrorKind::Load))
        );
        assert!(
            v.problems[0].starts_with("the tests couldn't all run:\ntest_linked_list.py:"),
            "{:?}",
            v.problems
        );
        assert!(v.problems[0].contains("invalid syntax"), "{:?}", v.problems);

        // What pedagog-pytest 2 reports instead.
        let v2 = r#"{"run": {"status": "error", "output": "linked_list.py: invalid syntax"},
                     "summary": {"score": 0.0, "total": 0.0}, "tests": []}"#;
        let v = judge(Some(v2.as_bytes()));
        assert_eq!(v.kind, Some(ErrorKind::Load));
        assert!(v.problems[0].contains("invalid syntax"));
    }

    #[test]
    fn untrustworthy_results_are_errors_and_not_kept() {
        for (bytes, needle) in [
            (None, "no results.yaml"),
            (Some(&b"{not yaml"[..]), "didn't parse"),
        ] {
            let v = judge(bytes);
            assert_eq!((v.results, v.score), (None, None));
            assert!(v.problems[0].contains(needle), "{:?}", v.problems);
            assert!(matches!(
                v.kind,
                Some(ErrorKind::NoResults | ErrorKind::Invalid)
            ));
        }
        let text =
            std::str::from_utf8(FIXTURE)
                .unwrap()
                .replacen("\"total\": 10.0", "\"total\": 99.0", 1);
        let v = judge(Some(text.as_bytes()));
        assert_eq!(v.score, None);
        assert!(v.problems[0].contains("summary"), "{:?}", v.problems);
    }
}
