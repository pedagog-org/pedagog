use serde::Deserialize;

use crate::recipe::primitives::id::AssignmentId;

use super::platform::PlatformSpec;
use super::primitives::{Id, OsId, ToolchainId, Versioned};

#[derive(Debug, Deserialize)]
pub struct AssignmentYaml {
    pub id: AssignmentId,
    pub name: String,
    pub environment: Environment,
}

#[derive(Debug, Deserialize)]
pub struct Environment {
    pub os: OsId,
    pub platform: PlatformSpec,
    #[serde(default)]
    pub toolchains: Vec<Versioned<ToolchainId>>,
}
