use std::collections::HashMap;

use serde::Deserialize;

use super::primitives::{deserialize_one_or_many, Ingredient, OsId, ParamVal, Step, ToolchainId, Version, Versioned};

#[derive(Debug, Deserialize)]
pub struct ToolchainRecipe {
    pub id: ToolchainId,
    pub version: Version,
    #[serde(deserialize_with = "deserialize_one_or_many")]
    pub os: Vec<OsId>,
    #[serde(default)]
    pub params: HashMap<String, ParamVal>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub addons: Vec<Versioned<ToolchainId>>,
    #[serde(default)]
    pub ingredients: Vec<Ingredient>,
}
