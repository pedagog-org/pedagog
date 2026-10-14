use std::collections::HashMap;

use serde::Deserialize;

use super::primitives::{Ingredient, OsId, ParamHook, ParamVal, deserialize_one_or_many};

#[derive(Debug, Deserialize)]
pub struct PlatformRecipe {
    pub platform: PlatformKind,
    #[serde(deserialize_with = "deserialize_one_or_many")]
    pub os: Vec<OsId>,
    pub hooks: PlatformHookDefs,
    #[serde(default)]
    pub ingredients: Vec<Ingredient>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlatformKind {
    Interactive,
}

impl PlatformKind {
    pub fn as_str(&self) -> &str {
        match self {
            PlatformKind::Interactive => "interactive",
        }
    }
}

impl TryFrom<String> for PlatformKind {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.as_str() {
            "interactive" => Ok(PlatformKind::Interactive),
            other => Err(format!("unknown platform kind {other:?}")),
        }
    }
}

impl<'de> Deserialize<'de> for PlatformKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        PlatformKind::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for PlatformKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Deserialize)]
pub struct PlatformHookDefs {
    pub build: ParamHook,
    /// Exec-form argv; each item is interpolated with the platform params.
    pub entrypoint: Vec<String>,
    /// Interpolated like `entrypoint`, then parsed as non-zero port numbers.
    #[serde(default)]
    pub ports: Vec<String>,
}

// ---- PlatformSpec -----------------------------------------------------------

/// Platform kind + assignment-provided param overrides.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformSpec {
    pub kind: PlatformKind,
    #[serde(default)]
    pub params: HashMap<String, ParamVal>,
}
