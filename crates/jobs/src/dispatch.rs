//! The dispatch pass: reconcile orphans, then dispatch changed recipes as
//! concurrent build tasks that finalize their own row on completion (Model C).

use std::collections::HashSet;
use std::path::Path;

use anyhow::anyhow;
use pedagog_core::recipe::render::containerfile::Containerfile;
use pedagog_core::recipe::render::{FromSource, ImageSpec, Needed, Render, RenderOptions};
use pedagog_core::recipe::resolve::resolve_base;
use pedagog_core::recipe::store::RecipeStore;
use pedagog_k8s::build::{BuildRequest, Context, IfBuilt, JobKey, JobState, Outcome, Waited};
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
    ingredients: Vec<Needed>,
}

/// How long an OS build's ingredient URLs last (OS builds have no deadline).
const OS_INGREDIENTS_TTL: std::time::Duration = std::time::Duration::from_secs(4 * 3600);

/// Which OS images a dispatch pass builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Those whose recipe changed since their last build (jobs' startup).
    Changed,
    /// All of them, replacing finished Jobs (the rebuild endpoint).
    All,
}

/// Reconcile orphans, then dispatch the pass's not-in-flight recipes. Returns as
/// soon as work is dispatched; the short lock covers only detect → ensure → start_run.
pub async fn dispatch(
    state: &AppState,
    pass: Pass,
    only: Option<&str>,
) -> anyhow::Result<Dispatched> {
    // One OS dispatch pass at a time, across replicas; held until `_lock`
    // drops at the end (a rollback: nothing is written through it). Only one
    // pass holds it, so its queries borrowing other connections can't starve.
    let mut _lock = state.pool.begin().await?;
    db::lock(&mut _lock, "dispatch:os").await?;

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
        pass,
        only,
        &in_flight,
    )
    .await?;

    // Rebuilding all also replaces Jobs that already succeeded for this hash;
    // otherwise a lingering Job would leave the image as it was until its TTL.
    let if_built = if pass == Pass::All {
        IfBuilt::Rebuild
    } else {
        IfBuilt::Skip
    };
    let mut dispatched = Vec::new();
    for p in planned {
        let ingredients =
            crate::ingredients::fetches(state, &p.ingredients, OS_INGREDIENTS_TTL).await?;
        let request = BuildRequest {
            dest: p.dest.clone(),
            containerfile: p.containerfile.clone(),
            context: Context::Containerfile,
            ingredients,
            deadline: None,
            if_built,
        };
        match state
            .builder
            .ensure(&JobKey::os(&p.os_id, &p.hash), &request)
            .await?
        {
            Outcome::Skipped => {}
            Outcome::Created | Outcome::Retried => {
                let run = db::builds::start_run(&state.pool, &p.os_id, &p.hash, &p.dest).await?;
                let sums = crate::ingredients::checksums(&p.ingredients);
                db::builds::record_run_ingredients(&state.pool, run, &sums).await?;
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
    pass: Pass,
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
            pass,
            db::builds::last_hash(pool, &id).await?.as_deref(),
            &hash,
        ) {
            continue;
        }
        out.push(Planned {
            os_id: id,
            hash,
            dest: format!("{registry}/{image}"),
            ingredients: spec.needed_ingredients(FromSource::Standalone),
            containerfile,
        });
    }
    Ok(out)
}

fn needs_build(pass: Pass, recorded: Option<&str>, current: &str) -> bool {
    pass == Pass::All || recorded != Some(current)
}

fn spawn_build(state: &AppState, run: i64, os_id: String, hash: String, dest: String) {
    let pool = state.pool.clone();
    let builder = state.builder.clone();
    tokio::spawn(async move {
        match builder.wait(&JobKey::os(&os_id, &hash), None).await {
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
        let key = JobKey::os(&run.os_id, &run.hash);
        let waited = match builder.poll(&key).await {
            Ok(JobState::Succeeded) => Some(Waited::Succeeded),
            Ok(JobState::Failed { logs }) => Some(Waited::Failed { logs }),
            Ok(JobState::Active) => builder.wait(&key, None).await.ok(),
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

pub(crate) fn load_store(dir: &Path) -> anyhow::Result<RecipeStore> {
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
        assert!(needs_build(Pass::Changed, None, "h")); // empty DB → build
        assert!(!needs_build(Pass::Changed, Some("h"), "h")); // unchanged → skip
        assert!(needs_build(Pass::Changed, Some("old"), "h")); // changed → build
        assert!(needs_build(Pass::All, Some("h"), "h")); // rebuild all → build regardless
    }

    const RECIPES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../recipes");

    #[sqlx::test(migrations = "../store/migrations")]
    async fn plan_reflects_recorded_hashes(pool: PgPool) -> anyhow::Result<()> {
        let store = load_store(Path::new(RECIPES))?;
        let none = HashSet::new();

        // empty DB → ubuntu-22 is planned
        let planned = plan_builds(&pool, &store, "reg", Pass::Changed, None, &none).await?;
        let ubuntu = planned
            .iter()
            .find(|p| p.os_id == "ubuntu-22")
            .expect("ubuntu-22 planned on empty DB");
        let (hash, dest) = (ubuntu.hash.clone(), ubuntu.dest.clone());

        // record its hash → no longer planned
        db::builds::record_build(&pool, "ubuntu-22", &hash, &dest).await?;
        assert!(
            plan_builds(&pool, &store, "reg", Pass::Changed, None, &none)
                .await?
                .iter()
                .all(|p| p.os_id != "ubuntu-22")
        );

        // rebuilding all → planned again despite matching hash
        assert!(
            plan_builds(&pool, &store, "reg", Pass::All, None, &none)
                .await?
                .iter()
                .any(|p| p.os_id == "ubuntu-22")
        );

        // in-flight → skipped even when rebuilding all
        let inflight = HashSet::from(["ubuntu-22".to_string()]);
        assert!(
            plan_builds(&pool, &store, "reg", Pass::All, None, &inflight)
                .await?
                .iter()
                .all(|p| p.os_id != "ubuntu-22")
        );
        Ok(())
    }
}
