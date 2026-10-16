//! Grading runs: admission launches a run's Job (the pinned build's image,
//! the submission fetched into `/pedagog/submission`), then a watcher waits
//! for it and records the outcome. Runs in flight are watched again after a
//! restart ([`reconcile`]).

use std::time::Duration;

use pedagog_core::results::Results;
use pedagog_k8s::build::JobKey;
use pedagog_k8s::grade::GradingRequest;
use pedagog_store::db::assignments as builds_db;
use pedagog_store::db::submissions::{self as db, RunOutcome};
use pedagog_store::objects::grading_results_key;
use sqlx::PgConnection;

use crate::admission::Launch;
use crate::assignments::plan_of;
use crate::state::AppState;

/// Extra validity for presigned URLs beyond the Job's deadline.
const URL_MARGIN: Duration = Duration::from_secs(600);
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
    let plan = plan_of(state, &build).await.map_err(Launch::permanent)?;
    let key = JobKey::grading(&ctx.assignment_id, run);
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
    if !db::start_grading(&mut *conn, run, &key.name(), &plan.isolation)
        .await
        .map_err(Launch::transient)?
    {
        return Ok(()); // already started
    }
    let request = GradingRequest {
        image: ctx.image_ref,
        jobs_image: state.config.jobs_image.clone(),
        submission_url: submission_url.to_string(),
        tree_hash: ctx.tree_hash,
        results_url: results_url.to_string(),
        isolation: plan.isolation.clone(),
        egress: plan.egress.clone(),
        deadline: limits.deadline,
        cpu: limits.cpu.clone(),
        memory: limits.memory.clone(),
    };
    let outcome = state
        .builder
        .ensure_grading(&key, &request)
        .await
        .map_err(Launch::kube)?;
    tracing::info!(run, assignment = %ctx.assignment_id, ?outcome, "grading Job ensured");
    Ok(())
}

/// Wait for a grading run's Job and record its outcome.
pub fn watch(state: &AppState, run: i64) {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = await_outcome(&state, run).await {
            tracing::error!(run, error = %e, "grading run failed to finish");
            fail(&state, run, &format!("grading couldn't finish: {e}")).await;
        }
    });
}

async fn await_outcome(state: &AppState, run: i64) -> anyhow::Result<()> {
    let ctx = db::run_context(&state.pool, run)
        .await?
        .ok_or_else(|| anyhow::anyhow!("grading run {run} is missing"))?;
    let key = JobKey::grading(&ctx.assignment_id, run);
    let waited = state
        .builder
        .wait(&key, Some(state.config.grading.deadline))
        .await?;
    let uploaded = state.objects.get(&grading_results_key(run)).await.ok();
    let mut verdict = judge(uploaded.as_deref());
    let mut log_tail = None;
    if verdict.results.is_none()
        && let pedagog_k8s::build::Waited::Failed { logs } = waited
    {
        log_tail = Some(logs[logs.len().saturating_sub(LOG_TAIL)..].to_owned());
        verdict.error = Some("the run crashed or timed out".to_owned());
    }
    let outcome = RunOutcome {
        score: verdict.score,
        results: verdict.results.as_deref(),
        problems: &verdict.problems,
        error: verdict.error.as_deref(),
        log_tail: log_tail.as_deref(),
        ..Default::default()
    };
    db::finish_run(&state.pool, run, &outcome).await?;
    match verdict.score {
        Some((score, total)) => tracing::info!(run, score, total, "graded"),
        None => tracing::warn!(run, problems = ?verdict.problems, "grading run errored"),
    }
    Ok(())
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
}

/// Results that parse and pass the format checks are graded (failing tests
/// included); anything else is an error, and nothing from it is kept.
pub fn judge(uploaded: Option<&[u8]>) -> Verdict {
    let error = |problems: Vec<String>| Verdict {
        results: None,
        score: None,
        problems,
        error: None,
    };
    let Some(bytes) = uploaded else {
        return error(vec!["no results.yaml was uploaded".into()]);
    };
    let results: Results = match serde_yaml::from_slice(bytes) {
        Ok(r) => r,
        Err(e) => return error(vec![format!("results.yaml didn't parse: {e}")]),
    };
    let problems = results.problems();
    if !problems.is_empty() {
        return error(problems);
    }
    Verdict {
        score: Some((results.summary.score, results.summary.total)),
        results: serde_json::to_string(&results).ok(),
        problems: Vec::new(),
        error: None,
    }
}

/// After a restart: watch every run still grading. A run whose Job is gone is
/// judged from whatever results it uploaded.
pub async fn reconcile(state: &AppState) -> anyhow::Result<()> {
    for run in db::grading_in_flight(&state.pool).await? {
        tracing::info!(run = run.id, "resuming a grading run");
        watch(state, run.id);
    }
    Ok(())
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
    fn untrustworthy_results_are_errors_and_not_kept() {
        for (bytes, needle) in [
            (None, "no results.yaml"),
            (Some(&b"{not yaml"[..]), "didn't parse"),
        ] {
            let v = judge(bytes);
            assert_eq!((v.results, v.score), (None, None));
            assert!(v.problems[0].contains(needle), "{:?}", v.problems);
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
