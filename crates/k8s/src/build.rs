//! Kaniko build orchestration: Job construction, `ensure`/`wait`/`poll`.
//!
//! OS builds use only the Containerfile as their context; assignment builds get
//! an archive context, downloaded and unpacked into an emptyDir by an
//! initContainer (`pedagog-jobs fetch-context`).
//!
//! Recipes are mounted at `/pedagog/recipes` (dev: hostPath checkout; prod:
//! emptyDir filled by a jobs-image initContainer); their vended `ingredients/`
//! are also mounted at `/pedagog/ingredients`, where recipes' `vend` looks. Both
//! are excluded from the built image via kaniko `--ignore-path`. The rendered Containerfile arrives in a
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
const INGREDIENTS_PATH: &str = "/pedagog/ingredients";
const DOCKERFILE_DIR: &str = "/kaniko/dockerfile";
const DOCKERFILE_NAME: &str = "Containerfile";
const WORKSPACE: &str = "/workspace";
const TTL_SECONDS: i32 = 3600;
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const DELETE_WAIT: Duration = Duration::from_secs(30);
/// How long past a Job's own deadline `wait` keeps polling before giving up.
const WAIT_GRACE: Duration = Duration::from_secs(60);

/// Environment inputs that shape the Kaniko pod (dev vs prod).
#[derive(Clone)]
pub struct BuildEnv {
    pub kind: Env,
    /// prod: image whose baked recipes the initContainer stages.
    pub jobs_image: String,
    /// dev: node path to the recipes checkout mounted straight into kaniko.
    pub recipes_hostpath: Option<String>,
}

/// What a Job builds, for its name and labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    OsBuild,
    AssignmentBuild,
}

impl JobKind {
    fn prefix(self) -> &'static str {
        match self {
            JobKind::OsBuild => "os-build",
            JobKind::AssignmentBuild => "assignment-build",
        }
    }

    fn app_label(self) -> &'static str {
        match self {
            JobKind::OsBuild => "pedagog-os-build",
            JobKind::AssignmentBuild => "pedagog-assignment-build",
        }
    }
}

/// Identifies one build Job: its kind, what it builds, and the content hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobKey {
    pub kind: JobKind,
    pub id: String,
    pub hash: String,
}

impl JobKey {
    pub fn os(id: &str, hash: &str) -> Self {
        Self {
            kind: JobKind::OsBuild,
            id: id.into(),
            hash: hash.into(),
        }
    }

    pub fn assignment(id: &str, hash: &str) -> Self {
        Self {
            kind: JobKind::AssignmentBuild,
            id: id.into(),
            hash: hash.into(),
        }
    }

    /// Deterministic Job name `<prefix>-<id>-<hash12>`, DNS-1123-safe (≤63); a
    /// long id is shortened so the hash always survives.
    pub fn name(&self) -> String {
        let prefix = self.kind.prefix();
        let hash: String = self.hash.chars().take(12).collect();
        let room = 63 - prefix.len() - hash.len() - 2;
        let mut id: String = self
            .id
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        id.truncate(room);
        format!("{prefix}-{}-{hash}", id.trim_matches('-'))
    }
}

/// Where Kaniko's build context comes from.
#[derive(Clone, Debug)]
pub enum Context {
    /// Only the Containerfile (OS builds).
    Containerfile,
    /// An assignment archive: a presigned GET URL and the tree hash the
    /// unpacked files must match.
    Archive { url: String, tree_hash: String },
}

/// Everything `ensure` needs to create a build Job.
#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub dest: String,
    pub containerfile: String,
    pub context: Context,
    /// The Job's `activeDeadlineSeconds`; `None` for no limit.
    pub deadline: Option<Duration>,
    pub if_built: IfBuilt,
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

/// What `ensure` does with an existing Job that already succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfBuilt {
    /// Leave it: the image for this Containerfile hash is already built.
    Skip,
    /// Delete and recreate it (forced rebuild, e.g. after editing files the hash
    /// doesn't cover).
    Rebuild,
}

/// What `ensure` does for a given existing-Job state.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Create,
    Skip,
    Replace,
}

fn action(existing: Option<Phase>, if_built: IfBuilt) -> Action {
    match (existing, if_built) {
        (None, _) => Action::Create,
        (Some(Phase::Active), _) => Action::Skip,
        (Some(Phase::Succeeded), IfBuilt::Skip) => Action::Skip,
        (Some(Phase::Succeeded), IfBuilt::Rebuild) | (Some(Phase::Failed), _) => Action::Replace,
    }
}

/// Terminal classification of a Job's status.
#[derive(Debug, PartialEq, Eq)]
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

    /// Idempotently ensure a Kaniko build Job exists for `key`.
    ///
    /// Upserts the Containerfile ConfigMap, then branches on any existing Job:
    /// none → create; active/succeeded → skip; failed → delete + recreate.
    pub async fn ensure(&self, key: &JobKey, req: &BuildRequest) -> kube::Result<Outcome> {
        let name = key.name();
        let cm = configmap_name(&name);
        self.put_configmap(&cm, key.kind, &req.containerfile)
            .await?;

        let existing = self.client.jobs().get_opt(&name).await?;
        match action(existing.as_ref().map(phase), req.if_built) {
            Action::Create => self.create(&name, key.kind, req, &cm).await,
            Action::Skip => Ok(Outcome::Skipped),
            Action::Replace => {
                // fallback capture for a failed Job (primary capture is in wait/poll)
                if existing.as_ref().map(phase) == Some(Phase::Failed) {
                    let _ = self.capture_logs(&name).await;
                }
                self.delete(&name).await?;
                self.await_gone(&name).await?;
                self.create(&name, key.kind, req, &cm).await.map(|o| {
                    if o == Outcome::Created {
                        Outcome::Retried
                    } else {
                        o
                    }
                })
            }
        }
    }

    /// Delete the Job for `key` (and its pods), e.g. when its build is superseded.
    pub async fn remove(&self, key: &JobKey) -> kube::Result<()> {
        self.delete(&key.name()).await
    }

    /// Block until the Job for `key` reaches a terminal state. With a
    /// `deadline` (the Job's own), give up a minute after it.
    pub async fn wait(&self, key: &JobKey, deadline: Option<Duration>) -> kube::Result<Waited> {
        let name = key.name();
        let give_up = deadline.map(|d| tokio::time::Instant::now() + d + WAIT_GRACE);
        loop {
            if give_up.is_some_and(|t| tokio::time::Instant::now() >= t) {
                return Ok(Waited::Failed {
                    logs: format!("timed out\n{}", self.capture_logs(&name).await),
                });
            }
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

    /// Single non-blocking read of the Job for `key` — used by reconcile.
    pub async fn poll(&self, key: &JobKey) -> kube::Result<JobState> {
        let name = key.name();
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

    async fn create(
        &self,
        name: &str,
        kind: JobKind,
        req: &BuildRequest,
        cm: &str,
    ) -> kube::Result<Outcome> {
        let job = kaniko_job(&self.env, name, kind, req, cm);
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

    async fn put_configmap(
        &self,
        name: &str,
        kind: JobKind,
        containerfile: &str,
    ) -> kube::Result<()> {
        let cm = build_configmap(name, kind, containerfile);
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

fn configmap_name(job_name: &str) -> String {
    // ≤63 with the suffix; job_name is already ≤63 so trim room for "-cf"
    let mut base = job_name.to_string();
    base.truncate(60);
    format!("{base}-cf")
}

fn build_configmap(name: &str, kind: JobKind, containerfile: &str) -> ConfigMap {
    let data = BTreeMap::from([(DOCKERFILE_NAME.to_string(), containerfile.to_string())]);
    ConfigMap {
        metadata: meta(name, kind),
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

fn kaniko_job(env: &BuildEnv, name: &str, kind: JobKind, req: &BuildRequest, cm: &str) -> Job {
    let (recipes_vol, mut init) = recipes_volume(env);
    let mut volumes = vec![recipes_vol, configmap_volume("dockerfile", cm)];
    let mut mounts = vec![
        volume_mount("recipes", RECIPES_PATH, true),
        VolumeMount {
            sub_path: Some("ingredients".into()),
            ..volume_mount("recipes", INGREDIENTS_PATH, true)
        },
        volume_mount("dockerfile", DOCKERFILE_DIR, true),
    ];
    let context_dir = match &req.context {
        Context::Containerfile => DOCKERFILE_DIR,
        Context::Archive { url, tree_hash } => {
            volumes.push(Volume {
                name: "workspace".into(),
                empty_dir: Some(EmptyDirVolumeSource::default()),
                ..Default::default()
            });
            mounts.push(volume_mount("workspace", WORKSPACE, true));
            init.push(Container {
                name: "fetch-context".into(),
                image: Some(env.jobs_image.clone()),
                args: Some(vec![
                    "fetch-context".into(),
                    url.clone(),
                    WORKSPACE.into(),
                    tree_hash.clone(),
                ]),
                volume_mounts: Some(vec![volume_mount("workspace", WORKSPACE, false)]),
                ..Default::default()
            });
            WORKSPACE
        }
    };
    let kaniko = Container {
        name: "kaniko".into(),
        image: Some(KANIKO_IMAGE.into()),
        args: Some(vec![
            format!("--dockerfile={DOCKERFILE_DIR}/{DOCKERFILE_NAME}"),
            format!("--context=dir://{context_dir}"),
            format!("--destination={}", req.dest),
            "--insecure".into(),
            "--insecure-pull".into(),
            format!("--ignore-path={RECIPES_PATH}"),
            format!("--ignore-path={INGREDIENTS_PATH}"),
        ]),
        volume_mounts: Some(mounts),
        ..Default::default()
    };

    let pod = PodSpec {
        init_containers: (!init.is_empty()).then_some(init),
        containers: vec![kaniko],
        restart_policy: Some("Never".into()),
        volumes: Some(volumes),
        ..Default::default()
    };

    Job {
        metadata: meta(name, kind),
        spec: Some(JobSpec {
            backoff_limit: Some(0), // retries are handled by ensure(), not the Job controller
            ttl_seconds_after_finished: Some(TTL_SECONDS),
            active_deadline_seconds: req.deadline.map(|d| d.as_secs() as i64),
            template: PodTemplateSpec {
                metadata: Some(labelled_meta(kind)),
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

fn meta(name: &str, kind: JobKind) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        ..labelled_meta(kind)
    }
}

fn labelled_meta(kind: JobKind) -> ObjectMeta {
    ObjectMeta {
        labels: Some(BTreeMap::from([(
            "app".to_string(),
            kind.app_label().to_string(),
        )])),
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

    fn os_req(dest: &str) -> BuildRequest {
        BuildRequest {
            dest: dest.into(),
            containerfile: "FROM scratch\n".into(),
            context: Context::Containerfile,
            deadline: None,
            if_built: IfBuilt::Skip,
        }
    }

    fn assignment_req() -> BuildRequest {
        BuildRequest {
            dest: "reg/pedagog/assignments/ll:abc".into(),
            containerfile: "FROM scratch\n".into(),
            context: Context::Archive {
                url: "http://garage/presigned".into(),
                tree_hash: "d36ae4fe".into(),
            },
            deadline: Some(Duration::from_secs(1800)),
            if_built: IfBuilt::Skip,
        }
    }

    fn os_job(env: &BuildEnv, dest: &str) -> Job {
        kaniko_job(env, "j", JobKind::OsBuild, &os_req(dest), "j-cf")
    }

    #[test]
    fn ensure_action_per_existing_job_state() {
        use IfBuilt::{Rebuild, Skip};
        assert_eq!(action(None, Skip), Action::Create);
        assert_eq!(action(Some(Phase::Active), Rebuild), Action::Skip);
        assert_eq!(action(Some(Phase::Succeeded), Skip), Action::Skip);
        assert_eq!(action(Some(Phase::Succeeded), Rebuild), Action::Replace);
        assert_eq!(action(Some(Phase::Failed), Skip), Action::Replace);
    }

    #[test]
    fn job_name_is_deterministic_and_safe() {
        let a = JobKey::os("ubuntu-22", "abcdef0123456789").name();
        assert_eq!(a, "os-build-ubuntu-22-abcdef012345");
        assert_eq!(a, JobKey::os("ubuntu-22", "abcdef0123456789").name());
        assert_eq!(
            JobKey::assignment("linked-list-python", "0123456789abcdef").name(),
            "assignment-build-linked-list-python-0123456789ab"
        );
        // A long id is shortened so the hash survives within 63 characters.
        let long = JobKey::assignment(&"x".repeat(80), "0123456789abcdef").name();
        assert!(long.len() <= 63, "{long}");
        assert!(long.ends_with("-0123456789ab"), "{long}");
    }

    #[test]
    fn dev_mounts_hostpath_no_init() {
        let job = os_job(&dev_env(), "reg/x:1");
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
        let job = os_job(&prod_env(), "reg/x:1");
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
        let job = os_job(&prod_env(), "reg/pedagog/ubuntu-22:latest");
        let spec = job.spec.unwrap();
        assert_eq!(spec.backoff_limit, Some(0));
        assert_eq!(spec.ttl_seconds_after_finished, Some(3600));
        let kaniko = &spec.template.spec.unwrap().containers[0];
        let args = kaniko.args.clone().unwrap();
        assert!(args.contains(&"--ignore-path=/pedagog/recipes".to_string()));
        assert!(args.contains(&"--ignore-path=/pedagog/ingredients".to_string()));
        assert!(args.contains(&"--destination=reg/pedagog/ubuntu-22:latest".to_string()));
        assert!(args.iter().any(|a| a.starts_with("--dockerfile=")));
    }

    #[test]
    fn vended_ingredients_are_mounted_where_vend_looks() {
        for env in [dev_env(), prod_env()] {
            let job = os_job(&env, "reg/x:1");
            let kaniko = &job.spec.unwrap().template.spec.unwrap().containers[0];
            let mount = kaniko
                .volume_mounts
                .as_ref()
                .unwrap()
                .iter()
                .find(|m| m.mount_path == "/pedagog/ingredients")
                .expect("ingredients mount");
            assert_eq!(mount.name, "recipes");
            assert_eq!(mount.sub_path.as_deref(), Some("ingredients"));
            assert_eq!(mount.read_only, Some(true));
        }
    }

    #[test]
    fn os_builds_have_no_deadline_or_workspace() {
        let spec = os_job(&prod_env(), "reg/x:1").spec.unwrap();
        assert_eq!(spec.active_deadline_seconds, None);
        let args = spec.template.spec.unwrap().containers[0]
            .args
            .clone()
            .unwrap();
        assert!(args.contains(&"--context=dir:///kaniko/dockerfile".to_string()));
    }

    #[test]
    fn assignment_builds_fetch_their_archive_into_the_workspace() {
        for env in [dev_env(), prod_env()] {
            let job = kaniko_job(
                &env,
                "j",
                JobKind::AssignmentBuild,
                &assignment_req(),
                "j-cf",
            );
            let labels = job.metadata.labels.clone().unwrap();
            assert_eq!(labels["app"], "pedagog-assignment-build");
            let spec = job.spec.unwrap();
            assert_eq!(spec.active_deadline_seconds, Some(1800));
            let pod = spec.template.spec.unwrap();
            let init = pod.init_containers.unwrap();
            // fetch-context runs last, after prod's recipe staging
            let fetch = init.last().unwrap();
            assert_eq!(fetch.name, "fetch-context");
            assert_eq!(fetch.image.as_deref(), Some(env.jobs_image.as_str()));
            assert_eq!(
                fetch.args.clone().unwrap(),
                [
                    "fetch-context",
                    "http://garage/presigned",
                    "/workspace",
                    "d36ae4fe"
                ]
            );
            assert!(pod.volumes.unwrap().iter().any(|v| v.name == "workspace"));
            let kaniko = &pod.containers[0];
            assert!(
                kaniko
                    .args
                    .clone()
                    .unwrap()
                    .contains(&"--context=dir:///workspace".to_string())
            );
            let ws = kaniko
                .volume_mounts
                .as_ref()
                .unwrap()
                .iter()
                .find(|m| m.mount_path == "/workspace")
                .unwrap();
            assert_eq!(ws.read_only, Some(true));
        }
    }

    #[test]
    fn configmap_carries_containerfile() {
        let cm = build_configmap("j-cf", JobKind::OsBuild, "FROM scratch\n");
        assert_eq!(cm.data.unwrap()["Containerfile"], "FROM scratch\n");
    }
}
