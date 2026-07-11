use serde::Deserialize;

use super::primitives::{deserialize_one_or_many, Id, MaybeVersioned, Step, Version};

#[derive(Debug, Deserialize)]
pub struct ToolchainRecipe {
    pub id: Id,
    pub version: Version,
    #[serde(deserialize_with = "deserialize_one_or_many")]
    pub os: Vec<Id>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub addons: Vec<MaybeVersioned>,
}
