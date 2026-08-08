//! Service configuration from the environment. Env-var *names* are constants;
//! shared ones live in `core`, these are jobs-specific.

use std::path::PathBuf;

use pedagog_core::env::Env;
use pedagog_k8s::build::BuildEnv;

pub const DATABASE_URL: &str = "DATABASE_URL";
pub const RECIPES_DIR: &str = "PEDAGOG_RECIPES_DIR";
pub const BUILD_NAMESPACE: &str = "PEDAGOG_BUILD_NAMESPACE";
pub const REGISTRY: &str = "PEDAGOG_REGISTRY";
pub const JOBS_IMAGE: &str = "PEDAGOG_JOBS_IMAGE";
pub const RECIPES_HOSTPATH: &str = "PEDAGOG_RECIPES_HOSTPATH";
pub const LISTEN_ADDR: &str = "PEDAGOG_LISTEN_ADDR";

const DEFAULT_RECIPES_DIR: &str = "/opt/pedagog/recipes";
const DEFAULT_NAMESPACE: &str = "pedagog-builds";
const DEFAULT_REGISTRY: &str = "registry-service.pedagog-data.svc:5000";
const DEFAULT_LISTEN: &str = "0.0.0.0:8080";

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

fn var_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
