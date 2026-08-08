//! The dispatch pass: reconcile orphans, then dispatch changed recipes as
//! concurrent build tasks that finalize their own row on completion (Model C).

use std::collections::HashSet;
use std::path::Path;

use anyhow::anyhow;
use pedagog_core::recipe::render::containerfile::Containerfile;
use pedagog_core::recipe::render::{FromSource, ImageSpec, Render, RenderOptions};
use pedagog_core::recipe::resolve::resolve_base;
use pedagog_core::recipe::store::RecipeStore;
use pedagog_k8s::build::{JobState, Outcome, Waited};
use pedagog_store::{BuildRun, BuildStatus, db};
use serde::Serialize;
use sqlx::PgPool;

use crate::state::AppState;

/// What a dispatch pass kicked off — returned immediately; builds finish in the background.
#[derive(Debug, Serialize)]
pub struct Dispatched {
    pub dispatched: Vec<String>,
    pub already_in_flight: Vec<String>,
}

struct Planned {
    os_id: String,
    hash: String,
    dest: String,
    containerfile: String,
}

/// Reconcile orphans, then dispatch changed/not-in-flight recipes. Returns as soon
/// as work is dispatched; the short lock covers only detect → ensure → start_run.
pub async fn dispatch(
    state: &AppState,
    force: bool,
    only: Option<&str>,
) -> anyhow::Result<Dispatched> {
    let _guard = state.dispatch_lock.lock().await;

    // 1. reconcile still-running rows as concurrent adopt tasks (completion-order)
    for run in db::builds::running(&state.pool).await? {
        spawn_adopt(state, run);
    }
    let in_flight: HashSet<String> = db::builds::running(&state.pool)
        .await?
        .into_iter()
        .map(|r| r.os_id)
        .collect();

    // 2. plan + dispatch changed, not-already-in-flight recipes
    let store = load_store(&state.config.recipes_dir)?;
    let planned = plan_builds(
        &state.pool,
        &store,
        &state.config.registry,
        force,
        only,
        &in_flight,
    )
    .await?;

    let mut dispatched = Vec::new();
    for p in planned {
        match state
            .builder
            .ensure(&p.os_id, &p.hash, &p.dest, &p.containerfile)
            .await?
        {
            Outcome::Skipped => {}
            Outcome::Created | Outcome::Retried => {
                let run = db::builds::start_run(&state.pool, &p.os_id, &p.hash, &p.dest).await?;
                spawn_build(state, run, p.os_id.clone(), p.hash, p.dest);
                dispatched.push(p.os_id);
            }
        }
    }

    Ok(Dispatched {
        dispatched,
        already_in_flight: in_flight.into_iter().collect(),
    })
}

async fn plan_builds(
    pool: &PgPool,
    store: &RecipeStore,
    registry: &str,
    force: bool,
    only: Option<&str>,
    in_flight: &HashSet<String>,
) -> anyhow::Result<Vec<Planned>> {
    let mut out = Vec::new();
    for os_id in store.list_oses() {
        let id = os_id.as_str().to_owned();
        if only.is_some_and(|o| o != id) || in_flight.contains(&id) {
            continue;
        }
        let spec = resolve_base(os_id, store).map_err(|e| anyhow!(e))?;
        let ImageSpec::Base { image, .. } = &spec else {
            continue;
        };
        let containerfile = Containerfile::render(
            &spec,
            &RenderOptions {
                registry: None,
                from: FromSource::Standalone,
            },
        )
        .to_string();
        let hash = blake3::hash(containerfile.as_bytes()).to_hex().to_string();
        if !needs_build(
            force,
            db::builds::last_hash(pool, &id).await?.as_deref(),
            &hash,
        ) {
            continue;
        }
        out.push(Planned {
            os_id: id,
            hash,
            dest: format!("{registry}/{image}"),
            containerfile,
        });
    }
    Ok(out)
}

fn needs_build(force: bool, recorded: Option<&str>, current: &str) -> bool {
    force || recorded != Some(current)
}

fn spawn_build(state: &AppState, run: i64, os_id: String, hash: String, dest: String) {
    let pool = state.pool.clone();
    let builder = state.builder.clone();
    tokio::spawn(async move {
        match builder.wait(&os_id, &hash).await {
            Ok(waited) => finalize(&pool, run, &os_id, &hash, &dest, waited).await,
            Err(e) => {
                tracing::error!(%os_id, error = %e, "build wait failed; row left running for reconcile")
            }
        }
    });
}

fn spawn_adopt(state: &AppState, run: BuildRun) {
    let pool = state.pool.clone();
    let builder = state.builder.clone();
    tokio::spawn(async move {
        let waited = match builder.poll(&run.os_id, &run.hash).await {
            Ok(JobState::Succeeded) => Some(Waited::Succeeded),
            Ok(JobState::Failed { logs }) => Some(Waited::Failed { logs }),
            Ok(JobState::Active) => builder.wait(&run.os_id, &run.hash).await.ok(),
            Ok(JobState::Gone) => Some(Waited::Failed {
                logs: "interrupted".into(),
            }),
            Err(e) => {
                tracing::error!(os_id = %run.os_id, error = %e, "reconcile poll failed");
                None
            }
        };
        if let Some(waited) = waited {
            finalize(&pool, run.id, &run.os_id, &run.hash, &run.image_ref, waited).await;
        }
    });
}

async fn finalize(pool: &PgPool, run: i64, os_id: &str, hash: &str, dest: &str, waited: Waited) {
    match waited {
        Waited::Succeeded => {
            if let Err(e) = db::builds::finish_run(pool, run, BuildStatus::Succeeded, None).await {
                tracing::error!(%os_id, error = %e, "finish_run(succeeded) failed");
                return;
            }
            if let Err(e) = db::builds::record_build(pool, os_id, hash, dest).await {
                tracing::error!(%os_id, error = %e, "record_build failed");
            }
        }
        Waited::Failed { logs } => {
            if let Err(e) =
                db::builds::finish_run(pool, run, BuildStatus::Failed, Some(&logs)).await
            {
                tracing::error!(%os_id, error = %e, "finish_run(failed) failed");
            }
        }
    }
}

fn load_store(dir: &Path) -> anyhow::Result<RecipeStore> {
    RecipeStore::load(&[dir.to_path_buf()]).map_err(|errs| {
        anyhow!(
            "failed to load recipes from {}: {} error(s)",
            dir.display(),
            errs.len()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_build_logic() {
        assert!(needs_build(false, None, "h")); // empty DB → build
        assert!(!needs_build(false, Some("h"), "h")); // unchanged → skip
        assert!(needs_build(false, Some("old"), "h")); // changed → build
        assert!(needs_build(true, Some("h"), "h")); // force → build regardless
    }

    const RECIPES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../recipes");

    #[sqlx::test(migrations = "../store/migrations")]
    async fn plan_reflects_recorded_hashes(pool: PgPool) -> anyhow::Result<()> {
        let store = load_store(Path::new(RECIPES))?;
        let none = HashSet::new();

        // empty DB → ubuntu-22 is planned
        let planned = plan_builds(&pool, &store, "reg", false, None, &none).await?;
        let ubuntu = planned
            .iter()
            .find(|p| p.os_id == "ubuntu-22")
            .expect("ubuntu-22 planned on empty DB");
        let (hash, dest) = (ubuntu.hash.clone(), ubuntu.dest.clone());

        // record its hash → no longer planned
        db::builds::record_build(&pool, "ubuntu-22", &hash, &dest).await?;
        assert!(
            plan_builds(&pool, &store, "reg", false, None, &none)
                .await?
                .iter()
                .all(|p| p.os_id != "ubuntu-22")
        );

        // force → planned again despite matching hash
        assert!(
            plan_builds(&pool, &store, "reg", true, None, &none)
                .await?
                .iter()
                .any(|p| p.os_id == "ubuntu-22")
        );

        // in-flight → skipped even with force
        let inflight = HashSet::from(["ubuntu-22".to_string()]);
        assert!(
            plan_builds(&pool, &store, "reg", true, None, &inflight)
                .await?
                .iter()
                .all(|p| p.os_id != "ubuntu-22")
        );
        Ok(())
    }
}
