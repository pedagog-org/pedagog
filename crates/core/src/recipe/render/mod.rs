use std::borrow::Cow;

use strum_macros::Display;

use crate::recipe::assignment::{AssignmentYaml, Testing};
use crate::recipe::network::NetworkSpec;
use crate::recipe::platform::PlatformKind;
use crate::recipe::primitives::{OsId, ToolchainRef, command::Command, id::AssignmentId};

pub mod containerfile;
pub mod describe;

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

impl LayerSource {
    /// Human-readable layer name shared by every renderer.
    fn label(&self) -> String {
        match self {
            LayerSource::Os(id) => format!("OS: {id}"),
            LayerSource::Platform(kind) => format!("Platform: {kind}"),
            LayerSource::Toolchain { tc, origins } => {
                let addon_of: Vec<String> = origins
                    .iter()
                    .filter(|o| matches!(o, ToolchainOrigin::AddonOf(_)))
                    .map(ToString::to_string)
                    .collect();
                if addon_of.is_empty() {
                    format!("Toolchain: {tc}")
                } else {
                    format!("Toolchain: {tc} ({})", addon_of.join(", "))
                }
            }
            LayerSource::Assignment(id) => format!("Assignment: {id}"),
            LayerSource::OsCleanup(id) => format!("OS Cleanup: {id}"),
        }
    }
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
    #[strum(serialize = "ASSIGNMENT FILES")]
    AssignmentFiles,
}

/// The archive directories baked into the image, by source path inside the
/// build context (the unpacked archive). `None` means not present / not baked;
/// all `None` for environment-only renders.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssignmentFiles {
    /// → `/pedagog/starter` (root, read-only)
    pub starter: Option<String>,
    /// → `/pedagog/tests` (grader only)
    pub tests: Option<String>,
    /// → `/pedagog/solution` (grader only)
    pub solution: Option<String>,
}

impl AssignmentFiles {
    pub fn is_empty(&self) -> bool {
        self.starter.is_none() && self.tests.is_none() && self.solution.is_none()
    }

    /// What the assignment requires of the archive: tests whenever `testing:`
    /// is set. Starter and solution are optional (no solution ⇒ validation is
    /// skipped, not failed).
    pub fn problems(&self, assignment: &AssignmentYaml) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(testing) = &assignment.testing
            && self.tests.is_none()
        {
            problems.push(format!(
                "testing.framework is {} but the archive has no {}/ directory",
                testing.framework, assignment.files.tests
            ));
        }
        problems
    }
}

pub const STARTER_DIR: &str = "/pedagog/starter";
pub const TESTS_DIR: &str = "/pedagog/tests";
pub const SOLUTION_DIR: &str = "/pedagog/solution";
/// Owns the baked tests and solution; see the OS init contract.
pub const GRADER_USER: &str = "grader";

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
        testing: Option<Testing>,
        files: AssignmentFiles,
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

impl ImageSpec {
    /// Sets which archive directories to bake in; no-op for base images.
    pub fn with_files(mut self, baked: AssignmentFiles) -> Self {
        if let ImageSpec::Full { files, .. } = &mut self {
            *files = baked;
        }
        self
    }
}

impl RenderOptions {
    /// FROM image for a `Full` build. The registry prefix only applies to images
    /// pedagog builds; upstream images are pulled as written.
    fn full_from<'a>(&self, upstream: &'a str, base_image: &'a str) -> Cow<'a, str> {
        match self.from {
            FromSource::Standalone => Cow::Borrowed(upstream),
            FromSource::PrebuiltBase => self.prefixed(base_image),
        }
    }

    fn prefixed<'a>(&self, image: &'a str) -> Cow<'a, str> {
        match &self.registry {
            Some(reg) => format!("{reg}/{image}").into(),
            None => image.into(),
        }
    }
}

pub trait Render: ToString {
    fn render(spec: &ImageSpec, opts: &RenderOptions) -> Self;
}
