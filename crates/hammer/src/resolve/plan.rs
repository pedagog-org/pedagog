use pedagog_core::recipe::platform::PlatformKind;
use pedagog_core::recipe::primitives::{Id, OsId, ToolchainId, Versioned};

pub struct BasePlan {
    pub os_id: OsId,
    pub upstream: String,
    pub image: String,
    pub layers: Vec<Layer>,
}

pub struct BuildPlan {
    pub id: Id,
    pub name: String,
    /// Always the `FROM` image for this plan. When base layers are included,
    /// this is the upstream (e.g. `ubuntu:22.04`); otherwise the intermediate
    /// base image (e.g. `pedagog/ubuntu:22`).
    pub base_image: String,
    pub layers: Vec<Layer>,
    pub entrypoint: String,
}

#[derive(Clone)]
pub struct Layer {
    pub source: LayerSource,
    pub steps: Vec<ResolvedStep>,
}

#[derive(Clone)]
pub enum LayerSource {
    Os(OsId),
    Platform(PlatformKind),
    Toolchain(Versioned<ToolchainId>),
    BuildCleanup,
}

#[derive(Clone)]
pub struct ResolvedStep {
    pub name: Option<String>,
    pub commands: Vec<Command>,
}

/// Resolved shell command. Thin newtype; can grow env vars, cwd, exec/shell
/// distinction without changing call sites.
#[derive(Clone)]
pub struct Command(pub String);
