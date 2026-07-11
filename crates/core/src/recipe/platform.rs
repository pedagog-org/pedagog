use std::collections::HashMap;

use serde::Deserialize;

use super::primitives::{deserialize_one_or_many, HookDef, Id, ParamVal};

#[derive(Debug, Deserialize)]
pub struct PlatformRecipe {
    pub platform: PlatformKind,
    #[serde(deserialize_with = "deserialize_one_or_many")]
    pub os: Vec<Id>,
    #[serde(default)]
    pub params: HashMap<String, ParamVal>,
    pub hooks: PlatformHookDefs,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlatformKind {
    Interactive,
}

#[derive(Debug, Deserialize)]
pub struct PlatformHookDefs {
    pub build: HookDef,
    pub entrypoint: String,
}
