//! The admission gate: releases test runs (validation and grading) from the
//! queue while the cluster has room, so a burst of submissions waits in
//! Postgres, with a visible position, instead of as Pending pods burning
//! their deadlines.
//!
//! Every tick, under a transaction-level advisory lock (one replica releases
//! at a time): if no test run is waiting for a node (a Job without its pod,
//! or a pod not yet scheduled) and the optional ceiling allows, claim the next
//! run and create its Job in the same transaction. A Job that can't be created
//! leaves the run queued for the next tick; a run that can never start is
//! ended so it doesn't block the queue.
//!
//! The interface (enqueue in `store::db::queue`, release here) is what Kueue
//! would replace for multi-course fair sharing.

use std::time::Duration;

use pedagog_k8s::{KubeError, is_refusal};
use pedagog_store::QueuedRun;
use pedagog_store::db::queue;

use crate::assignments;
use crate::grading;
use crate::state::AppState;

const TICK: Duration = Duration::from_secs(2);

/// Why a released run didn't start.
#[derive(Debug)]
pub enum Launch {
    /// Try again next tick (the cluster or a store was unreachable).
    Transient(String),
    /// It will never start; end it.
    Permanent(String),
}

impl Launch {
    pub fn transient(e: impl std::fmt::Display) -> Self {
        Launch::Transient(e.to_string())
    }

    pub fn permanent(e: impl std::fmt::Display) -> Self {
        Launch::Permanent(e.to_string())
    }

    /// The API refusing the Job outright (4xx other than conflicts and rate
    /// limits) won't change on a retry.
    pub fn kube(e: KubeError) -> Self {
        if is_refusal(&e) {
            Launch::permanent(e)
        } else {
            Launch::transient(e)
        }
    }
}

pub fn spawn(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        loop {
            match release_once(&state).await {
                // Released one: check again at once, the next may fit too.
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(e) => tracing::warn!(error = %e, "admission tick failed"),
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

/// Release at most one run. Returns it if it was released (started, or ended
/// because it can never start).
async fn release_once(state: &AppState) -> anyhow::Result<Option<QueuedRun>> {
    let mut tx = state.pool.begin().await?;
    if !queue::try_admission_lock(&mut tx).await? {
        return Ok(None);
    }
    // Read capacity under the lock, so two replicas can't both see room.
    let backlog = state.builder.run_backlog().await?;
    if backlog.waiting > 0
        || state
            .config
            .max_running
            .is_some_and(|max| backlog.unfinished >= max)
    {
        return Ok(None);
    }
    let Some(next) = queue::claim_next(&mut tx).await? else {
        return Ok(None);
    };
    let launched = match next {
        QueuedRun::Validation(build) => assignments::launch_validation(state, &mut tx, build).await,
        QueuedRun::Grading(run) => grading::launch(state, &mut tx, run).await,
    };
    match launched {
        Ok(()) => {
            tx.commit().await?;
            tracing::info!(?next, "released");
            if let QueuedRun::Grading(run) = next {
                grading::watch(state, run);
            }
            Ok(Some(next))
        }
        Err(Launch::Transient(e)) => {
            drop(tx);
            tracing::warn!(?next, error = %e, "couldn't start; retrying");
            Ok(None)
        }
        Err(Launch::Permanent(e)) => {
            tx.commit().await?;
            tracing::warn!(?next, error = %e, "can't start; ending it");
            let message = format!("couldn't start: {e}");
            match next {
                QueuedRun::Validation(build) => {
                    assignments::fail_validation(state, build, &message).await;
                }
                QueuedRun::Grading(run) => grading::fail(state, run, &message).await,
            }
            Ok(Some(next))
        }
    }
}
