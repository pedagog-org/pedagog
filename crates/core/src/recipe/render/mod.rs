use strum_macros::Display;

use crate::recipe::network::NetworkSpec;
use crate::recipe::platform::PlatformKind;
use crate::recipe::primitives::{command::Command, id::AssignmentId, OsId, ToolchainRef};

pub mod containerfile;

#[derive(Debug, Clone)]
pub struct Layer {
    pub source: LayerSource,
    pub steps: Vec<ResolvedStep>,
}

#[derive(Debug, Clone)]
pub enum LayerSource {
    Os(OsId),
    Platform(PlatformKind),
    /// `origins` records every place the toolchain was requested (deduped).
    Toolchain {
        tc: ToolchainRef,
        origins: Vec<ToolchainOrigin>,
    },
    Assignment(AssignmentId),
    OsCleanup(OsId),
}

/// Why a toolchain is in the image: listed directly, or chosen as an addon of
/// another toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainOrigin {
    TopLevel,
    AddonOf(ToolchainRef),
}

impl std::fmt::Display for ToolchainOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolchainOrigin::TopLevel => f.write_str("listed directly"),
            ToolchainOrigin::AddonOf(parent) => write!(f, "addon of {parent}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedStep {
    pub name: Option<String>,
    pub commands: Vec<Command>,
}

#[derive(Display)]
pub enum BuildPhase {
    #[strum(serialize = "BASE")]
    Base,
    #[strum(serialize = "PLATFORM")]
    Platform,
    #[strum(serialize = "TOOLCHAIN")]
    Toolchain,
    #[strum(serialize = "ASSIGNMENT")]
    Assignment,
    #[strum(serialize = "OS CLEANUP")]
    OsCleanup,
}

/// Runtime metadata for the final image: the user the entrypoint runs as and the
/// entrypoint command.
#[derive(Clone)]
pub struct Runtime {
    pub user: String,
    pub entrypoint: Vec<String>,
}

#[derive(Clone)]
pub struct BuildPlan {
    pub os: Layer,
    pub platform: Layer,
    pub toolchain: Vec<Layer>,
    pub assignment: Layer,
    pub os_cleanup: Layer,
}

/// The two kinds of image the resolver produces. `Base` is a reusable OS base
/// image; `Full` is an assignment image with every build phase and runtime.
// Constructed once per invocation and passed by reference; the size gap between
// the variants doesn't matter here.
#[allow(clippy::large_enum_variant)]
pub enum ImageSpec {
    Base {
        upstream: String,
        image: String,
        os: Layer,
    },
    Full {
        upstream: String,
        base_image: String,
        plan: BuildPlan,
        runtime: Runtime,
        ports: Vec<u16>,
        /// Egress policy for the platform to enforce; does not affect the image.
        network: NetworkSpec,
    },
}

/// Where a `Full` image's FROM comes from: `Standalone` builds from upstream and
/// emits the os layer inline; `PrebuiltBase` builds from the base image, which
/// already contains the os layer.
#[derive(Clone, Copy)]
pub enum FromSource {
    Standalone,
    PrebuiltBase,
}

pub struct RenderOptions {
    pub registry: Option<String>,
    pub from: FromSource,
}

pub trait Render: ToString {
    fn render(spec: &ImageSpec, opts: &RenderOptions) -> Self;
}
