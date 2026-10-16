//! Assignment builds: decide whether an upload needs a build, supersede an
//! attempt in flight, drive the Kaniko Job, then validate the image by running
//! the reference solution against the tests, and judge its results.
//!
//! Each stage can be resumed from the database and the cluster after a restart
//! ([`reconcile`]).

use std::time::Duration;

use pedagog_core::results::Results;
use pedagog_k8s::build::{BuildRequest, Context, IfBuilt, JobKey, JobState, Waited};
use pedagog_k8s::validate::ValidationRequest;
use pedagog_store::db::queue;
use pedagog_store::db::{self as store_db, assignments as db};
use pedagog_store::objects::results_key;
use pedagog_store::{AssignmentBuild, AssignmentBuildStatus, AssignmentUpload, ValidationStatus};
use serde::Serialize;
use sqlx::PgConnection;

use crate::admission::Launch;
use crate::intake;
use crate::settings::{RunSettings, settings_of};
use crate::state::AppState;

/// Extra validity for presigned URLs beyond their Job's deadline.
const URL_MARGIN: Duration = Duration::from_secs(600);
/// How much of a failed validation pod's log is kept with its problems.
const LOG_TAIL: usize = 4000;

/// An upload, rendered and ready to build and validate.
#[derive(Clone, Debug)]
pub struct Plan {
    pub assignment_id: String,
    pub upload_id: i64,
    pub tree_hash: String,
    pub object_key: String,
    pub containerfile: String,
    pub has_solution: bool,
    pub settings: RunSettings,
    pub force: bool,
}

impl Plan {
    /// Covers everything that goes into the image: the rendered Containerfile
    /// and the archive's files.
    pub fn image_hash(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(self.containerfile.as_bytes());
        h.update(b"\0");
        h.update(self.tree_hash.as_bytes());
        h.finalize().to_hex().to_string()
    }

    pub fn image_ref(&self, registry: &str) -> String {
        let hash = self.image_hash();
        format!(
            "{registry}/pedagog/assignments/{}:{}",
            self.assignment_id,
            &hash[..16]
        )
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum Decision {
    /// A build attempt started.
    Dispatched { build_id: i64 },
    /// This exact image was already built and validated (or validation was skipped).
    AlreadyBuilt { build_id: i64 },
}

pub async fn dispatch(state: &AppState, plan: Plan) -> anyhow::Result<Decision> {
    let id = &plan.assignment_id;
    // One dispatch decision per assignment at a time, across replicas. The
    // decision's queries run on the lock's own connection, so concurrent
    // dispatches can't exhaust the pool waiting for a second one.
    let mut tx = state.pool.begin().await?;
    store_db::lock(&mut tx, &format!("dispatch:{id}")).await?;
    let image_hash = plan.image_hash();
    if !plan.force
        && let Some(build_id) = db::usable_build_for(&mut *tx, id, &image_hash).await?
    {
        return Ok(Decision::AlreadyBuilt { build_id });
    }
    if let Some(running) = db::in_flight_for(&mut *tx, id).await? {
        supersede(state, &mut tx, &running).await?;
    }
    let image_ref = plan.image_ref(&state.config.registry);
    let build_id = db::start_build(
        &mut *tx,
        id,
        plan.upload_id,
        &image_hash,
        &image_ref,
        &plan.settings.to_stored()?,
    )
    .await?;
    tx.commit().await?;
    tracing::info!(assignment = %id, build = build_id, image = %image_ref, "build dispatched");
    spawn(state, build_id, plan, Resume::Build);
    Ok(Decision::Dispatched { build_id })
}

/// Latest upload wins: stop the older attempt's Jobs and mark it superseded.
async fn supersede(
    state: &AppState,
    conn: &mut PgConnection,
    running: &AssignmentBuild,
) -> anyhow::Result<()> {
    let id = &running.assignment_id;
    for key in [
        JobKey::assignment(id, &running.image_hash),
        JobKey::validation(id, running.id),
    ] {
        if let Err(e) = state.builder.remove(&key).await {
            tracing::warn!(build = running.id, error = %e, "deleting a superseded Job failed");
        }
    }
    db::supersede(conn, running.id).await?;
    tracing::info!(assignment = %id, build = running.id, "superseded");
    Ok(())
}

/// Where an attempt picks up.
#[derive(Clone, Copy, Debug)]
enum Resume {
    /// Ensure the build Job (reusing one that already built this image unless forced).
    Build,
    /// The build Job exists; wait for it.
    AwaitBuild,
    /// The image is built; queue validation (admission starts it, the
    /// test-run watcher judges it).
    Validate,
}

fn spawn(state: &AppState, build_id: i64, plan: Plan, from: Resume) {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run(&state, build_id, &plan, from).await {
            tracing::error!(build = build_id, error = %e, "build attempt failed to run");
            let message = e.to_string();
            match from {
                Resume::Build | Resume::AwaitBuild => {
                    let _ = db::finish_build(&state.pool, build_id, false, Some(&message)).await;
                }
                Resume::Validate => {
                    let problems = [format!("validation couldn't run: {message}")];
                    let _ = db::finish_validation(
                        &state.pool,
                        build_id,
                        ValidationStatus::Invalid,
                        None,
                        &problems,
                    )
                    .await;
                }
            }
        }
    });
}

async fn run(state: &AppState, build_id: i64, plan: &Plan, from: Resume) -> anyhow::Result<()> {
    let image_hash = plan.image_hash();
    let image_ref = plan.image_ref(&state.config.registry);
    let build_key = JobKey::assignment(&plan.assignment_id, &image_hash);
    let deadline = state.config.assignment_build_deadline;
    if let Resume::Build = from {
        let url = state
            .objects
            .presign_get(&plan.object_key, deadline + URL_MARGIN)
            .await?;
        let request = BuildRequest {
            dest: image_ref.clone(),
            containerfile: plan.containerfile.clone(),
            context: Context::Archive {
                url: url.to_string(),
                tree_hash: plan.tree_hash.clone(),
            },
            deadline: Some(deadline),
            if_built: if plan.force {
                IfBuilt::Rebuild
            } else {
                IfBuilt::Skip
            },
        };
        let outcome = state.builder.ensure(&build_key, &request).await?;
        tracing::info!(build = build_id, ?outcome, "build Job ensured");
    }
    if let Resume::Build | Resume::AwaitBuild = from {
        match state.builder.wait(&build_key, Some(deadline)).await? {
            Waited::Succeeded => {
                if !db::finish_build(&state.pool, build_id, true, None).await? {
                    return Ok(()); // superseded meanwhile
                }
                tracing::info!(build = build_id, "image built");
            }
            Waited::Failed { logs } => {
                db::finish_build(&state.pool, build_id, false, Some(&logs)).await?;
                tracing::warn!(build = build_id, "image build failed");
                return Ok(());
            }
        }
    }
    if !plan.has_solution {
        db::finish_validation(&state.pool, build_id, ValidationStatus::Skipped, None, &[]).await?;
        tracing::info!(
            build = build_id,
            "validation skipped (no reference solution)"
        );
        return Ok(());
    }
    // Admission starts the validation; the test-run watcher judges it.
    if queue::enqueue_validation(&state.pool, build_id).await? {
        tracing::info!(build = build_id, "validation queued");
    }
    Ok(())
}

/// Judge a validation whose pod ended (or whose Job is gone) from the results
/// it uploaded, and end the attempt. Does nothing if it isn't running (already
/// settled, or superseded), so reports of the same pod can repeat.
pub async fn settle_validation(
    state: &AppState,
    build_id: i64,
    succeeded: bool,
) -> anyhow::Result<()> {
    let Some(build) = db::build_by_id(&state.pool, build_id).await? else {
        return Ok(());
    };
    if build.finished || build.validation_status != ValidationStatus::Running {
        return Ok(());
    }
    let uploaded = state.objects.get(&results_key(build_id)).await.ok();
    let mut verdict = judge(uploaded.as_deref());
    if uploaded.is_none() && !succeeded {
        let logs = state
            .builder
            .logs(&JobKey::validation(&build.assignment_id, build_id))
            .await;
        let tail = &logs[logs.len().saturating_sub(LOG_TAIL)..];
        verdict
            .problems
            .push(format!("validation pod log (tail):\n{tail}"));
    }
    db::finish_validation(
        &state.pool,
        build_id,
        verdict.status,
        verdict.results.as_deref(),
        &verdict.problems,
    )
    .await?;
    tracing::info!(build = build_id, status = %verdict.status, "validated");
    Ok(())
}

/// Start a released validation: mark it running and create its Job, in the
/// caller's release transaction.
pub async fn launch_validation(
    state: &AppState,
    conn: &mut PgConnection,
    build_id: i64,
) -> Result<(), Launch> {
    let build = db::build_by_id(&state.pool, build_id)
        .await
        .map_err(Launch::transient)?
        .ok_or_else(|| Launch::permanent(format!("build {build_id} is missing")))?;
    let settings = settings_of(state, &build)
        .await
        .map_err(Launch::permanent)?;
    let deadline = state.config.validation.deadline;
    let results_url = state
        .objects
        .presign_put(&results_key(build_id), deadline + URL_MARGIN)
        .await
        .map_err(Launch::transient)?;
    if !db::start_validation(&mut *conn, build_id)
        .await
        .map_err(Launch::transient)?
    {
        return Ok(()); // superseded meanwhile
    }
    let request = ValidationRequest {
        build_id,
        image: build.image_ref.clone(),
        jobs_image: state.config.jobs_image.clone(),
        results_url: results_url.to_string(),
        isolation: settings.isolation,
        egress: settings.egress,
        deadline,
        cpu: state.config.validation.cpu.clone(),
        memory: state.config.validation.memory.clone(),
    };
    let key = JobKey::validation(&build.assignment_id, build_id);
    let outcome = state
        .builder
        .ensure_validation(&key, &request)
        .await
        .map_err(Launch::kube)?;
    tracing::info!(build = build_id, ?outcome, "validation Job ensured");
    Ok(())
}

/// End a validation that can never start.
pub async fn fail_validation(state: &AppState, build_id: i64, message: &str) {
    let problems = [format!("validation {message}")];
    if let Err(e) = db::finish_validation(
        &state.pool,
        build_id,
        ValidationStatus::Invalid,
        None,
        &problems,
    )
    .await
    {
        tracing::error!(build = build_id, error = %e, "recording a failed validation failed");
    }
}

/// What a validation run's results.yaml says about the assignment.
#[derive(Debug, PartialEq, Eq)]
pub struct Verdict {
    pub status: ValidationStatus,
    /// The parsed results as JSON, when they can be trusted.
    pub results: Option<String>,
    pub problems: Vec<String>,
}

/// Missing, unparseable or inconsistent results are `invalid` (untrusted, not
/// stored); consistent results where the solution disagrees with the tests
/// are `failed` (stored, with the mismatches); otherwise `passed`.
pub fn judge(uploaded: Option<&[u8]>) -> Verdict {
    let invalid = |problems: Vec<String>| Verdict {
        status: ValidationStatus::Invalid,
        results: None,
        problems,
    };
    let Some(bytes) = uploaded else {
        return invalid(vec![
            "no results.yaml was uploaded (the run crashed or timed out)".into(),
        ]);
    };
    let results: Results = match serde_yaml::from_slice(bytes) {
        Ok(r) => r,
        Err(e) => return invalid(vec![format!("results.yaml didn't parse: {e}")]),
    };
    let problems = results.problems();
    if !problems.is_empty() {
        return invalid(problems);
    }
    let json = serde_json::to_string(&results).ok();
    let mismatches: Vec<String> = results
        .solution_mismatches()
        .iter()
        .map(ToString::to_string)
        .collect();
    Verdict {
        status: if mismatches.is_empty() {
            ValidationStatus::Passed
        } else {
            ValidationStatus::Failed
        },
        results: json,
        problems: mismatches,
    }
}

/// After a restart: resume every attempt in flight from where the cluster says
/// it is, and dispatch uploads that never got an attempt.
pub async fn reconcile(state: &AppState) -> anyhow::Result<()> {
    for build in db::in_flight(&state.pool).await? {
        let upload = upload_of(state, &build).await?;
        let plan = match plan_for(state, &upload, false).await {
            Ok(plan) => plan,
            Err(e) => {
                let message = format!("can't resume after a restart: {e}");
                db::finish_build(&state.pool, build.id, false, Some(&message)).await?;
                continue;
            }
        };
        let from = match build.build_status {
            AssignmentBuildStatus::Running => {
                match state
                    .builder
                    .poll(&JobKey::assignment(&build.assignment_id, &build.image_hash))
                    .await?
                {
                    JobState::Gone => {
                        db::finish_build(&state.pool, build.id, false, Some("interrupted")).await?;
                        continue;
                    }
                    _ => Resume::AwaitBuild,
                }
            }
            AssignmentBuildStatus::Succeeded => match build.validation_status {
                ValidationStatus::Pending => Resume::Validate,
                // Queued: admission starts it; running: the test-run watcher
                // and sweep judge it (from its results, even if the Job is gone).
                _ => continue,
            },
            _ => continue,
        };
        tracing::info!(build = build.id, ?from, "resuming");
        spawn(state, build.id, plan, from);
    }
    for upload in db::uploads_without_builds(&state.pool).await? {
        match plan_for(state, &upload, false).await {
            Ok(plan) => {
                let decision = dispatch(state, plan).await?;
                tracing::info!(
                    upload = upload.id,
                    ?decision,
                    "dispatched an orphaned upload"
                );
            }
            Err(e) => {
                tracing::warn!(upload = upload.id, error = %e, "orphaned upload can't be planned")
            }
        }
    }
    Ok(())
}

async fn upload_of(state: &AppState, build: &AssignmentBuild) -> anyhow::Result<AssignmentUpload> {
    db::get_upload(&state.pool, build.upload_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("build {} has no upload {}", build.id, build.upload_id))
}

/// Re-derives an upload's plan from its stored archive.
pub(crate) async fn plan_for(
    state: &AppState,
    upload: &AssignmentUpload,
    force: bool,
) -> anyhow::Result<Plan> {
    let bytes = state.objects.get(&upload.object_key).await?;
    let config = state.config.clone();
    let checked = tokio::task::spawn_blocking(move || {
        intake::check_upload(
            &bytes,
            &config.limits,
            &config.recipes_dir,
            &config.registry,
        )
    })
    .await?
    .map_err(|r| anyhow::anyhow!(r.error))?;
    Ok(Plan::from_checked(
        checked,
        upload.id,
        upload.object_key.clone(),
        force,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(containerfile: &str, tree: &str) -> Plan {
        Plan {
            assignment_id: "ll".into(),
            upload_id: 1,
            tree_hash: tree.into(),
            object_key: "assignments/ll/t.tgz".into(),
            containerfile: containerfile.into(),
            has_solution: true,
            settings: crate::settings::RunSettings {
                platform: pedagog_core::recipe::platform::PlatformKind::Upload,
                isolation: "process".into(),
                egress: pedagog_k8s::validate::Egress::Deny,
                submission: Default::default(),
            },
            force: false,
        }
    }

    #[test]
    fn image_hash_covers_containerfile_and_files() {
        let base = plan("FROM a\n", "tree-1");
        assert_eq!(base.image_hash(), plan("FROM a\n", "tree-1").image_hash());
        assert_ne!(base.image_hash(), plan("FROM b\n", "tree-1").image_hash());
        assert_ne!(base.image_hash(), plan("FROM a\n", "tree-2").image_hash());
        let image = base.image_ref("reg:5000");
        assert!(
            image.starts_with("reg:5000/pedagog/assignments/ll:"),
            "{image}"
        );
        assert_eq!(image.rsplit(':').next().unwrap().len(), 16);
    }

    #[test]
    fn decisions_serialize_with_a_status_tag() {
        let json = serde_json::to_value(Decision::AlreadyBuilt { build_id: 7 }).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"status": "already_built", "build_id": 7})
        );
    }

    const FIXTURE: &[u8] = include_bytes!("../../core/testdata/pedagog-pytest-results.json");

    #[test]
    fn real_results_pass() {
        let v = judge(Some(FIXTURE));
        assert_eq!(v.status, ValidationStatus::Passed, "{:?}", v.problems);
        assert!(v.problems.is_empty());
        assert!(v.results.unwrap().contains("\"pop-front\""));
    }

    #[test]
    fn mismatches_fail_and_keep_the_results() {
        // The stretch goal is declared to fail; make the solution pass it.
        let text = std::str::from_utf8(FIXTURE).unwrap().replacen(
            "\"status\": \"fail\"",
            "\"status\": \"pass\"",
            1,
        );
        let v = judge(Some(text.as_bytes()));
        assert_eq!(v.status, ValidationStatus::Failed);
        assert!(v.results.is_some());
        assert_eq!(
            v.problems,
            ["reverse: expected the solution to fail, but it passed"]
        );
    }

    #[test]
    fn untrustworthy_results_are_invalid_and_not_kept() {
        for (bytes, needle) in [
            (None, "no results.yaml"),
            (Some(&b"{not yaml"[..]), "didn't parse"),
        ] {
            let v = judge(bytes);
            assert_eq!(v.status, ValidationStatus::Invalid);
            assert!(v.results.is_none());
            assert!(v.problems[0].contains(needle), "{:?}", v.problems);
        }
        // Consistent shape, inconsistent numbers: the summary doesn't add up.
        let text =
            std::str::from_utf8(FIXTURE)
                .unwrap()
                .replacen("\"total\": 10.0", "\"total\": 99.0", 1);
        let v = judge(Some(text.as_bytes()));
        assert_eq!(v.status, ValidationStatus::Invalid);
        assert!(v.problems[0].contains("summary"), "{:?}", v.problems);
    }
}
