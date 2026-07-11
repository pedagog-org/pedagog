use std::collections::HashMap;

use serde::Deserialize;

use super::primitives::{deserialize_one_or_many, Id, ParamVal, Step, Version, Versioned};

#[derive(Debug, Deserialize)]
pub struct ToolchainRecipe {
    pub id: Id,
    pub version: Version,
    #[serde(deserialize_with = "deserialize_one_or_many")]
    pub os: Vec<Id>,
    #[serde(default)]
    pub params: HashMap<String, ParamVal>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub addons: Vec<Versioned>,
}
