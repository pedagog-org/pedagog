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
    ConfigMap, ConfigMapVolumeSource, Container, EmptyDirVolumeSource, PodSpec, PodTemplateSpec,
    ResourceRequirements, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{
    DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams, PropagationPolicy,
};

use pedagog_core::env::Env;

use crate::KubeClient;

const KANIKO_IMAGE: &str = "gcr.io/kaniko-project/executor:v1.24.0";
const RECIPES_PATH: &str = "/pedagog/recipes";
/// Where the OS layer's init step installs `vend` from.
const VEND_PATH: &str = "/pedagog/recipes/lib/vend";
const VEND_KEY: &str = "vend";
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
    /// The small image (`pedagog/jobs-init`) that runs `fetch-context` and
    /// `fetch-ingredients`.
    pub init_image: String,
    /// The recipes' `lib/vend` script, which the OS layer installs.
    pub vend_script: String,
    /// Repository for Kaniko's layer cache (e.g. `<registry>/pedagog/cache`);
    /// `None` builds without a cache.
    pub cache_repo: Option<String>,
    /// Kaniko unpacks the base image and every layer onto its own disk.
    pub disk: crate::Disk,
}

/// What a Job builds, for its name and labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    OsBuild,
    AssignmentBuild,
    AssignmentValidate,
    Grading,
}

impl JobKind {
    fn prefix(self) -> &'static str {
        match self {
            JobKind::OsBuild => "os-build",
            JobKind::AssignmentBuild => "assignment-build",
            JobKind::AssignmentValidate => "assignment-validate",
            JobKind::Grading => "grade",
        }
    }

    fn app_label(self) -> &'static str {
        match self {
            JobKind::OsBuild => "pedagog-os-build",
            JobKind::AssignmentBuild => "pedagog-assignment-build",
            JobKind::AssignmentValidate => "pedagog-assignment-validate",
            JobKind::Grading => "pedagog-grade",
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

    /// One validation run per build attempt.
    pub fn validation(id: &str, build_id: i64) -> Self {
        Self {
            kind: JobKind::AssignmentValidate,
            id: id.into(),
            hash: build_id.to_string(),
        }
    }

    /// One grading Job per grading run.
    pub fn grading(id: &str, run_id: i64) -> Self {
        Self {
            kind: JobKind::Grading,
            id: id.into(),
            hash: run_id.to_string(),
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

/// An ingredient a build `vend`s: where under `/pedagog/ingredients`, which file,
/// and a presigned GET for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngredientFetch {
    pub path: String,
    pub sha256: String,
    pub url: String,
}

/// Everything `ensure` needs to create a build Job.
#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub dest: String,
    pub containerfile: String,
    pub context: Context,
    /// Fetched (and checked) before the build; nothing else is there to `vend`.
    pub ingredients: Vec<IngredientFetch>,
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

/// Groups all build logic over a [`KubeClient`] + environment. Image builds
/// run in `client`'s namespace; test runs (validation, grading) in `runs`'.
pub struct Builder {
    pub(crate) client: KubeClient,
    pub(crate) runs: KubeClient,
    env: BuildEnv,
}

impl Builder {
    pub fn new(client: KubeClient, runs: KubeClient, env: BuildEnv) -> Self {
        Self { client, runs, env }
    }

    /// The namespace a kind of Job runs in.
    pub(crate) fn client_for(&self, kind: JobKind) -> &KubeClient {
        match kind {
            JobKind::AssignmentValidate | JobKind::Grading => &self.runs,
            JobKind::OsBuild | JobKind::AssignmentBuild => &self.client,
        }
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

        let existing = self.client_for(key.kind).jobs().get_opt(&name).await?;
        match action(existing.as_ref().map(phase), req.if_built) {
            Action::Create => self.create(&name, key.kind, req, &cm).await,
            Action::Skip => Ok(Outcome::Skipped),
            Action::Replace => {
                // fallback capture for a failed Job (primary capture is in wait/poll)
                if existing.as_ref().map(phase) == Some(Phase::Failed) {
                    let _ = self.capture_logs(key.kind, &name).await;
                }
                self.delete(key.kind, &name).await?;
                self.await_gone(key.kind, &name).await?;
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
        self.delete(key.kind, &key.name()).await
    }

    /// Block until the Job for `key` reaches a terminal state. With a
    /// `deadline` (the Job's own), give up a minute after it.
    pub async fn wait(&self, key: &JobKey, deadline: Option<Duration>) -> kube::Result<Waited> {
        let name = key.name();
        let give_up = deadline.map(|d| tokio::time::Instant::now() + d + WAIT_GRACE);
        loop {
            if give_up.is_some_and(|t| tokio::time::Instant::now() >= t) {
                return Ok(Waited::Failed {
                    logs: format!("timed out\n{}", self.capture_logs(key.kind, &name).await),
                });
            }
            match self.client_for(key.kind).jobs().get_opt(&name).await? {
                None => {
                    return Ok(Waited::Failed {
                        logs: "job vanished before completion".into(),
                    });
                }
                Some(job) => match phase(&job) {
                    Phase::Succeeded => return Ok(Waited::Succeeded),
                    Phase::Failed => {
                        return Ok(Waited::Failed {
                            logs: self.capture_logs(key.kind, &name).await,
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
        match self.client_for(key.kind).jobs().get_opt(&name).await? {
            None => Ok(JobState::Gone),
            Some(job) => Ok(match phase(&job) {
                Phase::Succeeded => JobState::Succeeded,
                Phase::Failed => JobState::Failed {
                    logs: self.capture_logs(key.kind, &name).await,
                },
                Phase::Active => JobState::Active,
            }),
        }
    }

    pub(crate) async fn create_job(&self, kind: JobKind, job: &Job) -> kube::Result<Option<Job>> {
        match self
            .client_for(kind)
            .jobs()
            .create(&PostParams::default(), job)
            .await
        {
            Ok(created) => Ok(Some(created)),
            // race backstop: another pass created it first
            Err(kube::Error::Api(e)) if e.code == 409 => Ok(None),
            Err(e) => Err(e),
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

    async fn delete(&self, kind: JobKind, name: &str) -> kube::Result<()> {
        let dp = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Background),
            ..Default::default()
        };
        match self.client_for(kind).jobs().delete(name, &dp).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn await_gone(&self, kind: JobKind, name: &str) -> kube::Result<()> {
        let deadline = DELETE_WAIT.as_secs() / POLL_INTERVAL.as_secs();
        for _ in 0..deadline {
            if self.client_for(kind).jobs().get_opt(name).await?.is_none() {
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
        let cm = build_configmap(name, kind, containerfile, &self.env.vend_script);
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

    /// Why Kubernetes ended `key`'s Job, if it did: its deadline (on the Job),
    /// or a container out of memory (on its pods).
    pub async fn stopped(&self, key: &JobKey) -> kube::Result<Option<crate::runs::Stopped>> {
        let name = key.name();
        let client = self.client_for(key.kind);
        if let Some(job) = client.jobs().get_opt(&name).await? {
            let deadline = job
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .is_some_and(|cs| {
                    cs.iter().any(|c| {
                        c.type_ == "Failed" && c.reason.as_deref() == Some("DeadlineExceeded")
                    })
                });
            if deadline {
                return Ok(Some(crate::runs::Stopped::Deadline));
            }
        }
        let lp = ListParams::default().labels(&format!("batch.kubernetes.io/job-name={name}"));
        Ok(client
            .pods()
            .list(&lp)
            .await?
            .iter()
            .find_map(crate::runs::stopped_pod))
    }

    /// Best-effort logs of `key`'s pod (empty if unavailable).
    pub async fn logs(&self, key: &JobKey) -> String {
        self.capture_logs(key.kind, &key.name()).await
    }

    /// Best-effort pod logs for a Job (empty string if unavailable).
    async fn capture_logs(&self, kind: JobKind, job_name: &str) -> String {
        let pods = self.client_for(kind).pods();
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

fn build_configmap(name: &str, kind: JobKind, containerfile: &str, vend: &str) -> ConfigMap {
    let data = BTreeMap::from([
        (DOCKERFILE_NAME.to_string(), containerfile.to_string()),
        (VEND_KEY.to_string(), vend.to_string()),
    ]);
    ConfigMap {
        metadata: meta(name, kind),
        data: Some(data),
        ..Default::default()
    }
}

/// Ephemeral-storage requests and limits only (Kaniko sets no CPU or memory).
fn disk_resources(disk: &crate::Disk) -> ResourceRequirements {
    let one = |q: &str| BTreeMap::from([("ephemeral-storage".to_string(), Quantity(q.into()))]);
    ResourceRequirements {
        requests: Some(one(&disk.request)),
        limits: Some(one(&disk.limit)),
        ..Default::default()
    }
}

fn kaniko_job(env: &BuildEnv, name: &str, kind: JobKind, req: &BuildRequest, cm: &str) -> Job {
    let mut init = Vec::new();
    let mut volumes = vec![
        configmap_volume("dockerfile", cm),
        Volume {
            name: "ingredients".into(),
            empty_dir: Some(EmptyDirVolumeSource::default()),
            ..Default::default()
        },
    ];
    let mut mounts = vec![
        volume_mount("dockerfile", DOCKERFILE_DIR, true),
        VolumeMount {
            sub_path: Some(VEND_KEY.into()),
            ..volume_mount("dockerfile", VEND_PATH, true)
        },
        volume_mount("ingredients", INGREDIENTS_PATH, true),
    ];
    // Exactly the files its recipes pin, fetched by checksum: nothing else is
    // there to `vend`, and nothing comes from the internet.
    if !req.ingredients.is_empty() {
        let mut args = vec![
            "fetch-ingredients".to_string(),
            INGREDIENTS_PATH.to_string(),
        ];
        for i in &req.ingredients {
            args.extend([i.sha256.clone(), i.path.clone(), i.url.clone()]);
        }
        init.push(Container {
            name: "fetch-ingredients".into(),
            image: Some(env.init_image.clone()),
            args: Some(args),
            volume_mounts: Some(vec![volume_mount("ingredients", INGREDIENTS_PATH, false)]),
            ..Default::default()
        });
    }
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
                image: Some(env.init_image.clone()),
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
        args: Some(
            vec![
                format!("--dockerfile={DOCKERFILE_DIR}/{DOCKERFILE_NAME}"),
                format!("--context=dir://{context_dir}"),
                format!("--destination={}", req.dest),
                "--insecure".into(),
                "--insecure-pull".into(),
                format!("--ignore-path={RECIPES_PATH}"),
                format!("--ignore-path={INGREDIENTS_PATH}"),
            ]
            .into_iter()
            .chain(cache_args(env))
            .collect(),
        ),
        volume_mounts: Some(mounts),
        resources: Some(disk_resources(&env.disk)),
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

/// Unchanged layers (toolchains, platform, ...) come from the cache, so an
/// upload that only changes its files rebuilds just the ASSIGNMENT FILES phase.
/// The cache lives in the same plain-HTTP in-cluster registry as the images.
fn cache_args(env: &BuildEnv) -> Vec<String> {
    let Some(repo) = &env.cache_repo else {
        return Vec::new();
    };
    let registry = repo.split('/').next().unwrap_or(repo);
    vec![
        "--cache=true".into(),
        format!("--cache-repo={repo}"),
        format!("--insecure-registry={registry}"),
    ]
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
            init_image: "unused-init".into(),
            vend_script: "#!/bin/sh\n".into(),
            cache_repo: None,
            disk: crate::Disk {
                request: "2Gi".into(),
                limit: "8Gi".into(),
            },
        }
    }

    fn prod_env() -> BuildEnv {
        BuildEnv {
            kind: Env::Prod,
            init_image: "registry/pedagog/jobs-init:1".into(),
            vend_script: "#!/bin/sh\n".into(),
            cache_repo: Some("registry:5000/pedagog/cache".into()),
            disk: crate::Disk {
                request: "2Gi".into(),
                limit: "8Gi".into(),
            },
        }
    }

    fn os_req(dest: &str) -> BuildRequest {
        BuildRequest {
            dest: dest.into(),
            containerfile: "FROM scratch\n".into(),
            context: Context::Containerfile,
            ingredients: Vec::new(),
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
            ingredients: vec![IngredientFetch {
                path: "toolchain/pedagog-pytest/2/pedagog-pytest-bundle.tar.gz".into(),
                sha256: "fef3".into(),
                url: "http://garage/ingredient".into(),
            }],
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
    fn a_build_without_ingredients_needs_no_init() {
        let job = os_job(&prod_env(), "reg/x:1");
        let pod = job.spec.unwrap().template.spec.unwrap();
        assert!(
            pod.init_containers.is_none(),
            "nothing staged from the jobs image"
        );
    }

    #[test]
    fn builds_fetch_exactly_their_ingredients_by_checksum() {
        let job = kaniko_job(
            &prod_env(),
            "j",
            JobKind::AssignmentBuild,
            &assignment_req(),
            "j-cf",
        );
        let init = job
            .spec
            .unwrap()
            .template
            .spec
            .unwrap()
            .init_containers
            .unwrap();
        let fetch = init.iter().find(|c| c.name == "fetch-ingredients").unwrap();
        assert_eq!(fetch.image.as_deref(), Some("registry/pedagog/jobs-init:1"));
        assert_eq!(
            fetch.args.clone().unwrap(),
            [
                "fetch-ingredients",
                "/pedagog/ingredients",
                "fef3",
                "toolchain/pedagog-pytest/2/pedagog-pytest-bundle.tar.gz",
                "http://garage/ingredient"
            ]
        );
    }

    #[test]
    fn vend_comes_with_the_containerfile() {
        let cm = build_configmap(
            "j-cf",
            JobKind::OsBuild,
            "FROM scratch\n",
            "#!/bin/sh\necho x\n",
        );
        assert_eq!(cm.data.unwrap()["vend"], "#!/bin/sh\necho x\n");
        let job = os_job(&prod_env(), "reg/x:1");
        let kaniko = &job.spec.unwrap().template.spec.unwrap().containers[0];
        let vend = kaniko
            .volume_mounts
            .as_ref()
            .unwrap()
            .iter()
            .find(|m| m.mount_path == "/pedagog/recipes/lib/vend")
            .expect("vend where the OS layer installs it from");
        assert_eq!(
            (vend.name.as_str(), vend.sub_path.as_deref()),
            ("dockerfile", Some("vend"))
        );
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
        let limits = kaniko.resources.clone().unwrap().limits.unwrap();
        assert_eq!(limits["ephemeral-storage"], Quantity("8Gi".into()));
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
            assert_eq!(mount.name, "ingredients");
            assert_eq!(mount.read_only, Some(true));
        }
    }

    #[test]
    fn layer_cache_is_on_when_a_cache_repo_is_set() {
        let args = |env: &BuildEnv| {
            os_job(env, "reg/x:1")
                .spec
                .unwrap()
                .template
                .spec
                .unwrap()
                .containers[0]
                .args
                .clone()
                .unwrap()
        };
        let cached = args(&prod_env());
        assert!(cached.contains(&"--cache=true".to_string()));
        assert!(cached.contains(&"--cache-repo=registry:5000/pedagog/cache".to_string()));
        assert!(cached.contains(&"--insecure-registry=registry:5000".to_string()));
        assert!(!args(&dev_env()).iter().any(|a| a.starts_with("--cache")));
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
            let fetch = init.iter().find(|c| c.name == "fetch-context").unwrap();
            assert_eq!(fetch.name, "fetch-context");
            assert_eq!(fetch.image.as_deref(), Some(env.init_image.as_str()));
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
        let cm = build_configmap("j-cf", JobKind::OsBuild, "FROM scratch\n", "");
        assert_eq!(cm.data.unwrap()["Containerfile"], "FROM scratch\n");
    }
}
