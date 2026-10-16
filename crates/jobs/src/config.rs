//! Service configuration from the environment. Env-var *names* are constants;
//! shared ones live in `core`, these are jobs-specific.

use std::path::PathBuf;

use pedagog_core::env::Env;
use pedagog_k8s::build::BuildEnv;
use pedagog_k8s::grade::RunSize;
use pedagog_store::objects::ObjectsConfig;

use crate::archive::Limits;
use crate::submissions::SubmissionLimits;

pub const DATABASE_URL: &str = "DATABASE_URL";
pub const RECIPES_DIR: &str = "PEDAGOG_RECIPES_DIR";
pub const BUILD_NAMESPACE: &str = "PEDAGOG_BUILD_NAMESPACE";
pub const RUN_NAMESPACE: &str = "PEDAGOG_RUN_NAMESPACE";
pub const REGISTRY: &str = "PEDAGOG_REGISTRY";
pub const JOBS_IMAGE: &str = "PEDAGOG_JOBS_IMAGE";
pub const RECIPES_HOSTPATH: &str = "PEDAGOG_RECIPES_HOSTPATH";
pub const LISTEN_ADDR: &str = "PEDAGOG_LISTEN_ADDR";
pub const MAX_UPLOAD_BYTES: &str = "PEDAGOG_MAX_UPLOAD_BYTES";
pub const MAX_UNPACKED_BYTES: &str = "PEDAGOG_MAX_UNPACKED_BYTES";
pub const MAX_UPLOAD_FILES: &str = "PEDAGOG_MAX_UPLOAD_FILES";
pub const ASSIGNMENT_BUILD_DEADLINE: &str = "PEDAGOG_ASSIGNMENT_BUILD_DEADLINE";
pub const VALIDATION_DEADLINE: &str = "PEDAGOG_VALIDATION_DEADLINE";
pub const VALIDATION_CPU: &str = "PEDAGOG_VALIDATION_CPU";
pub const VALIDATION_MEMORY: &str = "PEDAGOG_VALIDATION_MEMORY";
pub const GRADING_DEADLINE: &str = "PEDAGOG_GRADING_DEADLINE";
pub const GRADING_CPU: &str = "PEDAGOG_GRADING_CPU";
pub const GRADING_MEMORY: &str = "PEDAGOG_GRADING_MEMORY";
pub const MAX_RUNNING: &str = "PEDAGOG_MAX_RUNNING_TESTS";
pub const BLOCKED_CIDRS: &str = "PEDAGOG_BLOCKED_CIDRS";
pub const BUILD_CACHE: &str = "PEDAGOG_BUILD_CACHE";
pub const MAX_SUBMISSION_BYTES: &str = "PEDAGOG_MAX_SUBMISSION_BYTES";
pub const MAX_SUBMISSION_FILES: &str = "PEDAGOG_MAX_SUBMISSION_FILES";

const DEFAULT_RECIPES_DIR: &str = "/opt/pedagog/recipes";
const DEFAULT_NAMESPACE: &str = "pedagog-builds";
const DEFAULT_RUN_NAMESPACE: &str = "pedagog-grading";
const DEFAULT_REGISTRY: &str = "registry-service.pedagog-data.svc:5000";
const DEFAULT_LISTEN: &str = "0.0.0.0:8080";
// Fallbacks until assignments can set their own.
const DEFAULT_ASSIGNMENT_BUILD_DEADLINE: u64 = 1800;
const DEFAULT_VALIDATION_DEADLINE: u64 = 600;
const DEFAULT_VALIDATION_CPU: &str = "1";
const DEFAULT_VALIDATION_MEMORY: &str = "1Gi";
pub const DEFAULT_SUBMISSION_LIMITS: SubmissionLimits = SubmissionLimits {
    max_bytes: 50 << 20,
    max_files: 1_000,
};
pub const DEFAULT_LIMITS: Limits = Limits {
    max_compressed_bytes: 50 << 20,
    max_unpacked_bytes: 200 << 20,
    max_files: 10_000,
};

pub struct Config {
    pub env: Env,
    pub database_url: String,
    pub recipes_dir: PathBuf,
    pub namespace: String,
    /// Where test runs (validation, grading) and their network policies live.
    pub run_namespace: String,
    pub registry: String,
    /// prod: image whose baked recipes the Kaniko initContainer stages.
    pub jobs_image: String,
    /// dev: node path to the recipes checkout mounted into Kaniko.
    pub recipes_hostpath: Option<String>,
    pub listen_addr: String,
    /// Garage, for assignment archives and run results.
    pub objects: ObjectsConfig,
    /// Upload limits (compressed, unpacked, file count).
    pub limits: Limits,
    /// An assignment build Job's `activeDeadlineSeconds`.
    pub assignment_build_deadline: std::time::Duration,
    pub validation: ValidationConfig,
    pub grading: ValidationConfig,
    /// Optional ceiling on test runs at once, on top of the cluster's capacity.
    pub max_running: Option<usize>,
    /// Ranges excluded from `mode: allow` egress beyond the private ones (e.g. a
    /// cluster on public addresses).
    pub blocked_cidrs: Vec<String>,
    /// Kaniko's layer cache in the registry (`PEDAGOG_BUILD_CACHE=false` turns it off).
    pub build_cache: bool,
    /// Ceiling for a submission; an assignment's `max_size` can only lower it.
    pub submission_limits: SubmissionLimits,
}

/// A kind of test run's limits.
pub struct ValidationConfig {
    /// The Job's `activeDeadlineSeconds`.
    pub deadline: std::time::Duration,
    pub cpu: String,
    pub memory: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var(DATABASE_URL)
            .map_err(|_| anyhow::anyhow!("{DATABASE_URL} must be set"))?;
        Ok(Self {
            env: Env::current(),
            database_url,
            recipes_dir: var_or(RECIPES_DIR, DEFAULT_RECIPES_DIR).into(),
            namespace: var_or(BUILD_NAMESPACE, DEFAULT_NAMESPACE),
            run_namespace: var_or(RUN_NAMESPACE, DEFAULT_RUN_NAMESPACE),
            registry: var_or(REGISTRY, DEFAULT_REGISTRY),
            jobs_image: std::env::var(JOBS_IMAGE).unwrap_or_default(),
            recipes_hostpath: std::env::var(RECIPES_HOSTPATH).ok(),
            listen_addr: var_or(LISTEN_ADDR, DEFAULT_LISTEN),
            objects: ObjectsConfig::from_env().map_err(|e| anyhow::anyhow!(e))?,
            limits: Limits {
                max_compressed_bytes: num_or(
                    MAX_UPLOAD_BYTES,
                    DEFAULT_LIMITS.max_compressed_bytes,
                )?,
                max_unpacked_bytes: num_or(MAX_UNPACKED_BYTES, DEFAULT_LIMITS.max_unpacked_bytes)?,
                max_files: num_or(MAX_UPLOAD_FILES, DEFAULT_LIMITS.max_files)?,
            },
            assignment_build_deadline: std::time::Duration::from_secs(num_or(
                ASSIGNMENT_BUILD_DEADLINE,
                DEFAULT_ASSIGNMENT_BUILD_DEADLINE,
            )?),
            validation: ValidationConfig {
                deadline: std::time::Duration::from_secs(num_or(
                    VALIDATION_DEADLINE,
                    DEFAULT_VALIDATION_DEADLINE,
                )?),
                cpu: var_or(VALIDATION_CPU, DEFAULT_VALIDATION_CPU),
                memory: var_or(VALIDATION_MEMORY, DEFAULT_VALIDATION_MEMORY),
            },
            grading: ValidationConfig {
                deadline: std::time::Duration::from_secs(num_or(
                    GRADING_DEADLINE,
                    DEFAULT_VALIDATION_DEADLINE,
                )?),
                cpu: var_or(GRADING_CPU, DEFAULT_VALIDATION_CPU),
                memory: var_or(GRADING_MEMORY, DEFAULT_VALIDATION_MEMORY),
            },
            max_running: std::env::var(MAX_RUNNING)
                .ok()
                .map(|v| {
                    v.parse()
                        .map_err(|_| anyhow::anyhow!("{MAX_RUNNING} must be a number, got {v:?}"))
                })
                .transpose()?,
            build_cache: var_or(BUILD_CACHE, "true") != "false",
            submission_limits: SubmissionLimits {
                max_bytes: num_or(MAX_SUBMISSION_BYTES, DEFAULT_SUBMISSION_LIMITS.max_bytes)?,
                max_files: num_or(MAX_SUBMISSION_FILES, DEFAULT_SUBMISSION_LIMITS.max_files)?,
            },
            blocked_cidrs: var_or(BLOCKED_CIDRS, "")
                .split(',')
                .map(|c| c.trim().to_owned())
                .filter(|c| !c.is_empty())
                .collect(),
        })
    }

    /// A validation's and a grading run's sizes: the next released run may be
    /// either, so admission checks that both would fit.
    pub fn run_sizes(&self) -> [RunSize; 2] {
        [&self.validation, &self.grading].map(|c| RunSize {
            cpu: c.cpu.clone(),
            memory: c.memory.clone(),
        })
    }

    pub fn build_env(&self) -> BuildEnv {
        BuildEnv {
            kind: self.env,
            jobs_image: self.jobs_image.clone(),
            recipes_hostpath: self.recipes_hostpath.clone(),
            cache_repo: self
                .build_cache
                .then(|| format!("{}/pedagog/cache", self.registry)),
        }
    }
}

fn num_or<T: std::str::FromStr>(key: &str, default: T) -> anyhow::Result<T> {
    match std::env::var(key) {
        Ok(v) => v
            .parse()
            .map_err(|_| anyhow::anyhow!("{key} must be a number, got {v:?}")),
        Err(_) => Ok(default),
    }
}

fn var_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
