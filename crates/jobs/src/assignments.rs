//! Assignment builds: decide whether an upload needs a build, supersede an
//! attempt in flight, and drive the Kaniko Job to completion.
//!
//! Validation (running the reference solution) comes next; until then a
//! successful build ends its attempt with validation still `pending`.

use std::time::Duration;

use pedagog_k8s::build::{BuildRequest, Context, IfBuilt, JobKey, Outcome, Waited};
use pedagog_store::ValidationStatus;
use pedagog_store::db::assignments as db;
use serde::Serialize;

use crate::state::AppState;

/// Extra validity for the archive's presigned GET beyond the build deadline.
const URL_MARGIN: Duration = Duration::from_secs(600);

/// An upload, rendered and ready to build.
#[derive(Clone, Debug)]
pub struct Plan {
    pub assignment_id: String,
    pub upload_id: i64,
    pub tree_hash: String,
    pub object_key: String,
    pub containerfile: String,
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
    let _guard = state.dispatch_lock.lock().await;
    let id = &plan.assignment_id;
    let image_hash = plan.image_hash();
    if !plan.force
        && let Some(build_id) = db::usable_build_for(&state.pool, id, &image_hash).await?
    {
        return Ok(Decision::AlreadyBuilt { build_id });
    }
    if let Some(running) = db::in_flight_for(&state.pool, id).await? {
        // Latest upload wins: stop the older attempt's Job and mark it superseded.
        if let Err(e) = state
            .builder
            .remove(&JobKey::assignment(id, &running.image_hash))
            .await
        {
            tracing::warn!(assignment = %id, build = running.id, error = %e, "deleting a superseded Job failed");
        }
        db::supersede(&state.pool, running.id).await?;
        tracing::info!(assignment = %id, build = running.id, "superseded");
    }
    let image_ref = plan.image_ref(&state.config.registry);
    let build_id =
        db::start_build(&state.pool, id, plan.upload_id, &image_hash, &image_ref).await?;
    tracing::info!(assignment = %id, build = build_id, image = %image_ref, "build dispatched");
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_build(&state, build_id, &plan, &image_hash, &image_ref).await {
            tracing::error!(build = build_id, error = %e, "build failed to run");
            let _ = db::finish_build(&state.pool, build_id, false, Some(&e.to_string())).await;
        }
    });
    Ok(Decision::Dispatched { build_id })
}

async fn run_build(
    state: &AppState,
    build_id: i64,
    plan: &Plan,
    image_hash: &str,
    image_ref: &str,
) -> anyhow::Result<()> {
    let deadline = state.config.assignment_build_deadline;
    let url = state
        .objects
        .presign_get(&plan.object_key, deadline + URL_MARGIN)
        .await?;
    let key = JobKey::assignment(&plan.assignment_id, image_hash);
    let request = BuildRequest {
        dest: image_ref.to_owned(),
        containerfile: plan.containerfile.clone(),
        context: Context::Archive {
            url: url.to_string(),
            tree_hash: plan.tree_hash.clone(),
        },
        deadline: Some(deadline),
        // A Job that already built this image is reused unless forced.
        if_built: if plan.force {
            IfBuilt::Rebuild
        } else {
            IfBuilt::Skip
        },
    };
    let outcome = state.builder.ensure(&key, &request).await?;
    tracing::info!(build = build_id, ?outcome, "build Job ensured");
    let waited = match outcome {
        Outcome::Created | Outcome::Retried | Outcome::Skipped => {
            state.builder.wait(&key, Some(deadline)).await?
        }
    };
    match waited {
        Waited::Succeeded => {
            if db::finish_build(&state.pool, build_id, true, None).await? {
                tracing::info!(build = build_id, "image built");
                let note = ["validation isn't implemented yet".to_owned()];
                db::finish_validation(
                    &state.pool,
                    build_id,
                    ValidationStatus::Pending,
                    None,
                    &note,
                )
                .await?;
            }
        }
        Waited::Failed { logs } => {
            if db::finish_build(&state.pool, build_id, false, Some(&logs)).await? {
                tracing::warn!(build = build_id, "image build failed");
            }
        }
    }
    Ok(())
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
}
