use strum_macros::Display;

use crate::recipe::{platform::PlatformKind, primitives::{OsId, ToolchainId, Versioned, command::Command, id::AssignmentId}};

pub mod containerfile;

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
    Assignment(AssignmentId)
}

#[derive(Clone)]
pub struct ResolvedStep {
    pub name: Option<String>,
    pub commands: Vec<Command>,
}

#[derive(Display)]
pub enum BuildPhase {
    Base,
    Platform,
    Toolchain,
    Assignment,
}

#[derive(Clone)]
pub struct BuildPlan {
    pub base: Layer,
    pub platform: Layer,
    pub toolchain: Vec<Layer>,
    pub assignment: Layer
}

pub struct ImageSpec {
    pub upstream_image: String,
    pub base_image: String,
    pub build_plan: BuildPlan,
    pub entrypoint: Command
}

pub enum RenderStartpoint {
    Scratch,
    BaseImage
}

pub trait Render<O>: ToString {
    fn render(spec: &ImageSpec, from: RenderStartpoint, options: O) -> Self;
}


