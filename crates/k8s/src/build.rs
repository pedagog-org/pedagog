//! Kaniko build orchestration: Job construction, `ensure`/`wait`/`poll`.
//!
//! Recipes are mounted at `/pedagog/recipes` (dev: hostPath checkout; prod:
//! emptyDir filled by a jobs-image initContainer) and excluded from the built
//! image via kaniko `--ignore-path`. The rendered Containerfile arrives in a
//! per-build ConfigMap. `k8s` speaks a Containerfile *string*, never recipe types.

use std::collections::BTreeMap;
use std::time::Duration;

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, EmptyDirVolumeSource, HostPathVolumeSource,
    PodSpec, PodTemplateSpec, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{
    DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams, PropagationPolicy,
};

use pedagog_core::env::Env;

use crate::KubeClient;

const KANIKO_IMAGE: &str = "gcr.io/kaniko-project/executor:v1.24.0";
const RECIPES_PATH: &str = "/pedagog/recipes";
const RECIPES_SRC: &str = "/opt/pedagog/recipes"; // baked path inside the jobs image
const DOCKERFILE_DIR: &str = "/kaniko/dockerfile";
const DOCKERFILE_NAME: &str = "Containerfile";
const APP_LABEL: &str = "pedagog-os-build";
const TTL_SECONDS: i32 = 3600;
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const DELETE_WAIT: Duration = Duration::from_secs(30);

/// Environment inputs that shape the Kaniko pod (dev vs prod).
#[derive(Clone)]
pub struct BuildEnv {
    pub kind: Env,
    /// prod: image whose baked recipes the initContainer stages.
    pub jobs_image: String,
    /// dev: node path to the recipes checkout mounted straight into kaniko.
    pub recipes_hostpath: Option<String>,
}

/// What `ensure` did — whether a fresh Job now exists to wait on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Retried,
    Skipped,
}

/// Result of blocking until a Job finishes.
#[derive(Clone, Debug)]
pub enum Waited {
    Succeeded,
    Failed { logs: String },
}

/// A single non-blocking status read (reconcile).
#[derive(Clone, Debug)]
pub enum JobState {
    Active,
    Succeeded,
    Failed { logs: String },
    Gone,
}

/// Terminal classification of a Job's status.
enum Phase {
    Active,
    Succeeded,
    Failed,
}

/// Groups all build logic over a [`KubeClient`] + environment.
pub struct Builder {
    client: KubeClient,
    env: BuildEnv,
}

impl Builder {
    pub fn new(client: KubeClient, env: BuildEnv) -> Self {
        Self { client, env }
    }

    /// Idempotently ensure a Kaniko build Job exists for `(os_id, hash)`.
    ///
    /// Upserts the Containerfile ConfigMap, then branches on any existing Job:
    /// none → create; active/succeeded → skip; failed → delete + recreate.
    pub async fn ensure(
        &self,
        os_id: &str,
        hash: &str,
        dest: &str,
        containerfile: &str,
    ) -> kube::Result<Outcome> {
        let name = job_name(os_id, hash);
        let cm = configmap_name(&name);
        self.put_configmap(&cm, containerfile).await?;

        match self.client.jobs().get_opt(&name).await? {
            None => self.create(&name, dest, &cm).await,
            Some(job) => match phase(&job) {
                Phase::Succeeded | Phase::Active => Ok(Outcome::Skipped),
                Phase::Failed => {
                    // fallback capture (primary capture is in wait/poll), then retry
                    let _ = self.capture_logs(&name).await;
                    self.delete(&name).await?;
                    self.await_gone(&name).await?;
                    self.create(&name, dest, &cm).await.map(|o| {
                        if o == Outcome::Created {
                            Outcome::Retried
                        } else {
                            o
                        }
                    })
                }
            },
        }
    }

    /// Block until the Job for `(os_id, hash)` reaches a terminal state.
    pub async fn wait(&self, os_id: &str, hash: &str) -> kube::Result<Waited> {
        let name = job_name(os_id, hash);
        loop {
            match self.client.jobs().get_opt(&name).await? {
                None => {
                    return Ok(Waited::Failed {
                        logs: "job vanished before completion".into(),
                    });
                }
                Some(job) => match phase(&job) {
                    Phase::Succeeded => return Ok(Waited::Succeeded),
                    Phase::Failed => {
                        return Ok(Waited::Failed {
                            logs: self.capture_logs(&name).await,
                        });
                    }
                    Phase::Active => tokio::time::sleep(POLL_INTERVAL).await,
                },
            }
        }
    }

    /// Single non-blocking read of the Job for `(os_id, hash)` — used by reconcile.
    pub async fn poll(&self, os_id: &str, hash: &str) -> kube::Result<JobState> {
        let name = job_name(os_id, hash);
        match self.client.jobs().get_opt(&name).await? {
            None => Ok(JobState::Gone),
            Some(job) => Ok(match phase(&job) {
                Phase::Succeeded => JobState::Succeeded,
                Phase::Failed => JobState::Failed {
                    logs: self.capture_logs(&name).await,
                },
                Phase::Active => JobState::Active,
            }),
        }
    }

    async fn create(&self, name: &str, dest: &str, cm: &str) -> kube::Result<Outcome> {
        let job = kaniko_job(&self.env, name, dest, cm);
        match self
            .client
            .jobs()
            .create(&PostParams::default(), &job)
            .await
        {
            Ok(_) => Ok(Outcome::Created),
            // race backstop: another pass created it first (single-replica + mutex make this rare)
            Err(kube::Error::Api(e)) if e.code == 409 => Ok(Outcome::Skipped),
            Err(e) => Err(e),
        }
    }

    async fn delete(&self, name: &str) -> kube::Result<()> {
        let dp = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Background),
            ..Default::default()
        };
        match self.client.jobs().delete(name, &dp).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn await_gone(&self, name: &str) -> kube::Result<()> {
        let deadline = DELETE_WAIT.as_secs() / POLL_INTERVAL.as_secs();
        for _ in 0..deadline {
            if self.client.jobs().get_opt(name).await?.is_none() {
                return Ok(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        Ok(())
    }

    async fn put_configmap(&self, name: &str, containerfile: &str) -> kube::Result<()> {
        let cm = build_configmap(name, containerfile);
        // server-side apply is idempotent across retries (Containerfile may change)
        self.client
            .configmaps()
            .patch(
                name,
                &PatchParams::apply("pedagog-jobs").force(),
                &Patch::Apply(&cm),
            )
            .await
            .map(|_| ())
    }

    /// Best-effort pod logs for a Job (empty string if unavailable).
    async fn capture_logs(&self, job_name: &str) -> String {
        let pods = self.client.pods();
        let lp = ListParams::default().labels(&format!("batch.kubernetes.io/job-name={job_name}"));
        let Ok(list) = pods.list(&lp).await else {
            return String::new();
        };
        for pod in list {
            if let Some(name) = pod.metadata.name
                && let Ok(logs) = pods.logs(&name, &LogParams::default()).await
            {
                return logs;
            }
        }
        String::new()
    }
}

fn phase(job: &Job) -> Phase {
    let status = job.status.as_ref();
    let succeeded = status.and_then(|s| s.succeeded).unwrap_or(0);
    let failed = status.and_then(|s| s.failed).unwrap_or(0);
    if succeeded > 0 {
        Phase::Succeeded
    } else if failed > 0 {
        Phase::Failed
    } else {
        Phase::Active
    }
}

/// Deterministic Job name `os-build-<os-id>-<hash-prefix>`, DNS-1123-safe (≤63).
fn job_name(os_id: &str, hash: &str) -> String {
    let prefix: String = hash.chars().take(12).collect();
    let raw = format!("os-build-{os_id}-{prefix}");
    let mut name: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    name.truncate(63);
    name.trim_matches('-').to_string()
}

fn configmap_name(job_name: &str) -> String {
    // ≤63 with the suffix; job_name is already ≤63 so trim room for "-cf"
    let mut base = job_name.to_string();
    base.truncate(60);
    format!("{base}-cf")
}

fn build_configmap(name: &str, containerfile: &str) -> ConfigMap {
    let data = BTreeMap::from([(DOCKERFILE_NAME.to_string(), containerfile.to_string())]);
    ConfigMap {
        metadata: meta(name),
        data: Some(data),
        ..Default::default()
    }
}

/// dev: recipes checkout mounted straight in (no init); prod: emptyDir staged from the jobs image.
fn recipes_volume(env: &BuildEnv) -> (Volume, Vec<Container>) {
    match (env.kind, &env.recipes_hostpath) {
        (Env::Dev, Some(hostpath)) => (
            Volume {
                name: "recipes".into(),
                host_path: Some(HostPathVolumeSource {
                    path: hostpath.clone(),
                    type_: Some("Directory".into()),
                }),
                ..Default::default()
            },
            vec![],
        ),
        _ => (
            Volume {
                name: "recipes".into(),
                empty_dir: Some(EmptyDirVolumeSource::default()),
                ..Default::default()
            },
            vec![Container {
                name: "stage-recipes".into(),
                image: Some(env.jobs_image.clone()),
                command: Some(vec![
                    "sh".into(),
                    "-c".into(),
                    format!("cp -a {RECIPES_SRC}/. {RECIPES_PATH}/"),
                ]),
                volume_mounts: Some(vec![volume_mount("recipes", RECIPES_PATH, false)]),
                ..Default::default()
            }],
        ),
    }
}

fn kaniko_job(env: &BuildEnv, name: &str, dest: &str, cm: &str) -> Job {
    let (recipes_vol, init) = recipes_volume(env);
    let kaniko = Container {
        name: "kaniko".into(),
        image: Some(KANIKO_IMAGE.into()),
        args: Some(vec![
            format!("--dockerfile={DOCKERFILE_DIR}/{DOCKERFILE_NAME}"),
            format!("--context=dir://{DOCKERFILE_DIR}"),
            format!("--destination={dest}"),
            "--insecure".into(),
            "--insecure-pull".into(),
            format!("--ignore-path={RECIPES_PATH}"),
        ]),
        volume_mounts: Some(vec![
            volume_mount("recipes", RECIPES_PATH, true),
            volume_mount("dockerfile", DOCKERFILE_DIR, true),
        ]),
        ..Default::default()
    };

    let pod = PodSpec {
        init_containers: (!init.is_empty()).then_some(init),
        containers: vec![kaniko],
        restart_policy: Some("Never".into()),
        volumes: Some(vec![recipes_vol, configmap_volume("dockerfile", cm)]),
        ..Default::default()
    };

    Job {
        metadata: meta(name),
        spec: Some(JobSpec {
            backoff_limit: Some(0), // retries are handled by ensure(), not the Job controller
            ttl_seconds_after_finished: Some(TTL_SECONDS),
            template: PodTemplateSpec {
                metadata: Some(labelled_meta()),
                spec: Some(pod),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn volume_mount(name: &str, path: &str, read_only: bool) -> VolumeMount {
    VolumeMount {
        name: name.into(),
        mount_path: path.into(),
        read_only: Some(read_only),
        ..Default::default()
    }
}

fn configmap_volume(name: &str, cm: &str) -> Volume {
    Volume {
        name: name.into(),
        config_map: Some(ConfigMapVolumeSource {
            name: cm.into(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn meta(name: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        labels: Some(BTreeMap::from([("app".to_string(), APP_LABEL.to_string())])),
        ..Default::default()
    }
}

fn labelled_meta() -> ObjectMeta {
    ObjectMeta {
        labels: Some(BTreeMap::from([("app".to_string(), APP_LABEL.to_string())])),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev_env() -> BuildEnv {
        BuildEnv {
            kind: Env::Dev,
            jobs_image: "unused".into(),
            recipes_hostpath: Some("/srv/recipes".into()),
        }
    }

    fn prod_env() -> BuildEnv {
        BuildEnv {
            kind: Env::Prod,
            jobs_image: "registry/pedagog/jobs:1".into(),
            recipes_hostpath: None,
        }
    }

    #[test]
    fn job_name_is_deterministic_and_safe() {
        let a = job_name("ubuntu-22", "abcdef0123456789");
        assert_eq!(a, "os-build-ubuntu-22-abcdef012345");
        assert_eq!(a, job_name("ubuntu-22", "abcdef0123456789"));
        assert!(a.len() <= 63);
    }

    #[test]
    fn dev_mounts_hostpath_no_init() {
        let job = kaniko_job(&dev_env(), "j", "reg/x:1", "j-cf");
        let pod = job.spec.unwrap().template.spec.unwrap();
        assert!(pod.init_containers.is_none());
        let recipes = pod
            .volumes
            .unwrap()
            .into_iter()
            .find(|v| v.name == "recipes")
            .unwrap();
        assert_eq!(recipes.host_path.unwrap().path, "/srv/recipes");
    }

    #[test]
    fn prod_stages_recipes_via_init_from_jobs_image() {
        let job = kaniko_job(&prod_env(), "j", "reg/x:1", "j-cf");
        let pod = job.spec.unwrap().template.spec.unwrap();
        let init = pod.init_containers.expect("prod has an initContainer");
        assert_eq!(init[0].image.as_deref(), Some("registry/pedagog/jobs:1"));
        let recipes = pod
            .volumes
            .unwrap()
            .into_iter()
            .find(|v| v.name == "recipes")
            .unwrap();
        assert!(recipes.empty_dir.is_some());
        assert!(recipes.host_path.is_none());
    }

    #[test]
    fn kaniko_args_and_job_policy() {
        let job = kaniko_job(&prod_env(), "j", "reg/pedagog/ubuntu-22:latest", "j-cf");
        let spec = job.spec.unwrap();
        assert_eq!(spec.backoff_limit, Some(0));
        assert_eq!(spec.ttl_seconds_after_finished, Some(3600));
        let kaniko = &spec.template.spec.unwrap().containers[0];
        let args = kaniko.args.clone().unwrap();
        assert!(args.contains(&"--ignore-path=/pedagog/recipes".to_string()));
        assert!(args.contains(&"--destination=reg/pedagog/ubuntu-22:latest".to_string()));
        assert!(args.iter().any(|a| a.starts_with("--dockerfile=")));
    }

    #[test]
    fn configmap_carries_containerfile() {
        let cm = build_configmap("j-cf", "FROM scratch\n");
        assert_eq!(cm.data.unwrap()["Containerfile"], "FROM scratch\n");
    }
}
