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
use sqlx::postgres::PgListener;

use crate::assignments;
use crate::grading;
use crate::state::AppState;

const TICK: Duration = Duration::from_secs(2);
/// After a release, how soon to check for room again: the released run's pod
/// is usually created (and placed) by then.
const RECHECK: Duration = Duration::from_millis(250);

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

/// The release loop; runs for the life of the service. Besides its tick, it
/// wakes when a run is queued or a test run ends (`queue::WAKE_CHANNEL`).
pub async fn run(state: AppState) {
    let mut wakes = None;
    loop {
        if wakes.is_none() {
            // (Re)connect: at startup, and after the listener failed.
            wakes = listen(&state).await;
        }
        let pause = match release_once(&state).await {
            // Released one: the next may fit as soon as its pod is placed.
            Ok(Some(_)) => RECHECK,
            Ok(None) => TICK,
            Err(e) => {
                tracing::warn!(error = %e, "admission tick failed");
                TICK
            }
        };
        match wakes.as_mut() {
            Some(listener) => {
                tokio::select! {
                    () = tokio::time::sleep(pause) => {}
                    woke = listener.recv() => {
                        if let Err(e) = woke {
                            tracing::warn!(error = %e, "admission's wake-ups failed; reconnecting");
                            wakes = None;
                        }
                    }
                }
            }
            None => tokio::time::sleep(pause).await,
        }
    }
}

/// A listener on the wake channel, or `None` (tick only) if it can't connect.
async fn listen(state: &AppState) -> Option<PgListener> {
    let connected = async {
        let mut listener = PgListener::connect_with(&state.pool).await?;
        listener.listen(queue::WAKE_CHANNEL).await?;
        Ok::<_, sqlx::Error>(listener)
    };
    match connected.await {
        Ok(listener) => Some(listener),
        Err(e) => {
            tracing::warn!(error = %e, "can't listen for admission wake-ups; ticking only");
            None
        }
    }
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
        || !state.builder.run_fits(&state.config.run_sizes()).await?
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
