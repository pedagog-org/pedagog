//! Settling test runs (validation and grading): one watch on the run
//! namespace's pods settles a run as soon as its pod ends, and a periodic
//! sweep settles whatever the watch can't see: a Job that failed before it
//! had a pod (e.g. its deadline passed while the quota was full), or one
//! whose pods are gone. Settling is idempotent, so the two can overlap.

use std::time::Duration;

use pedagog_k8s::build::{JobKey, JobState};
use pedagog_k8s::runs::{RunEnded, RunKind};
use pedagog_store::ValidationStatus;
use pedagog_store::db::{assignments as builds_db, queue, submissions as db};

use crate::assignments::{self, job_id};
use crate::grading;
use crate::state::AppState;

const SWEEP: Duration = Duration::from_secs(30);

/// The watch; runs for the life of the service.
pub async fn watch(state: AppState) {
    state
        .builder
        .on_runs_ended(|ended| settle(&state, ended))
        .await;
}

async fn settle(state: &AppState, ended: RunEnded) {
    let settled = match ended.kind {
        RunKind::Validation => {
            assignments::settle_validation(state, ended.id, ended.succeeded).await
        }
        RunKind::Grading => grading::settle(state, ended.id, ended.succeeded).await,
    };
    if let Err(e) = settled {
        tracing::warn!(?ended, error = %e, "settling a test run failed; the sweep retries");
    }
    // Its slot is free (or about to be): let admission look now.
    if let Err(e) = queue::wake(&state.pool).await {
        tracing::warn!(error = %e, "waking admission failed");
    }
}

/// The sweep; runs for the life of the service, starting at once (after a
/// restart it settles what ended meanwhile).
pub async fn sweep(state: AppState) {
    loop {
        if let Err(e) = sweep_once(&state).await {
            tracing::warn!(error = %e, "test-run sweep failed");
        }
        tokio::time::sleep(SWEEP).await;
    }
}

async fn sweep_once(state: &AppState) -> anyhow::Result<()> {
    for run in db::grading_in_flight(&state.pool).await? {
        let Some(ctx) = db::run_context(&state.pool, run.id).await? else {
            continue;
        };
        let key = JobKey::grading(&job_id(&ctx.course_id, &ctx.slug), run.id);
        if let Some(succeeded) = finished(state, &key).await? {
            settle(
                state,
                RunEnded {
                    kind: RunKind::Grading,
                    id: run.id,
                    succeeded,
                },
            )
            .await;
        }
    }
    for build in builds_db::in_flight(&state.pool).await? {
        if build.validation_status != ValidationStatus::Running {
            continue;
        }
        let key = JobKey::validation(&job_id(&build.course_id, &build.slug), build.id);
        if let Some(succeeded) = finished(state, &key).await? {
            settle(
                state,
                RunEnded {
                    kind: RunKind::Validation,
                    id: build.id,
                    succeeded,
                },
            )
            .await;
        }
    }
    Ok(())
}

/// Whether a test run's Job is over, and how; `None` while it's active.
async fn finished(state: &AppState, key: &JobKey) -> anyhow::Result<Option<bool>> {
    Ok(match state.builder.poll(key).await? {
        JobState::Active => None,
        JobState::Succeeded => Some(true),
        JobState::Failed { .. } | JobState::Gone => Some(false),
    })
}
