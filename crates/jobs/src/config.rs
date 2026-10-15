//! Service configuration from the environment. Env-var *names* are constants;
//! shared ones live in `core`, these are jobs-specific.

use std::path::PathBuf;

use pedagog_core::env::Env;
use pedagog_k8s::build::BuildEnv;
use pedagog_store::objects::ObjectsConfig;

use crate::archive::Limits;

pub const DATABASE_URL: &str = "DATABASE_URL";
pub const RECIPES_DIR: &str = "PEDAGOG_RECIPES_DIR";
pub const BUILD_NAMESPACE: &str = "PEDAGOG_BUILD_NAMESPACE";
pub const REGISTRY: &str = "PEDAGOG_REGISTRY";
pub const JOBS_IMAGE: &str = "PEDAGOG_JOBS_IMAGE";
pub const RECIPES_HOSTPATH: &str = "PEDAGOG_RECIPES_HOSTPATH";
pub const LISTEN_ADDR: &str = "PEDAGOG_LISTEN_ADDR";
pub const MAX_UPLOAD_BYTES: &str = "PEDAGOG_MAX_UPLOAD_BYTES";
pub const MAX_UNPACKED_BYTES: &str = "PEDAGOG_MAX_UNPACKED_BYTES";
pub const MAX_UPLOAD_FILES: &str = "PEDAGOG_MAX_UPLOAD_FILES";
pub const ASSIGNMENT_BUILD_DEADLINE: &str = "PEDAGOG_ASSIGNMENT_BUILD_DEADLINE";

const DEFAULT_RECIPES_DIR: &str = "/opt/pedagog/recipes";
const DEFAULT_NAMESPACE: &str = "pedagog-builds";
const DEFAULT_REGISTRY: &str = "registry-service.pedagog-data.svc:5000";
const DEFAULT_LISTEN: &str = "0.0.0.0:8080";
/// Fallback until assignments can set their own (seconds).
const DEFAULT_ASSIGNMENT_BUILD_DEADLINE: u64 = 1800;
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
        })
    }

    pub fn build_env(&self) -> BuildEnv {
        BuildEnv {
            kind: self.env,
            jobs_image: self.jobs_image.clone(),
            recipes_hostpath: self.recipes_hostpath.clone(),
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
