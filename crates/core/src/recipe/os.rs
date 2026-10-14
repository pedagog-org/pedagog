use serde::Deserialize;
use strum_macros::IntoStaticStr;

use super::primitives::{ArgHook, Ingredient, OsId, Step};

// ---- Arg enums ---------------------------------------------------------------

/// Valid args for pkg.install and pkg.remove hooks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, IntoStaticStr)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum PkgArg {
    Packages,
}

// ---- OS hook structs ---------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct OsDef {
    pub id: OsId,
    pub upstream: String,
    pub image: String,
    pub hooks: OsHookDefs,
    #[serde(default)]
    pub ingredients: Vec<Ingredient>,
}

#[derive(Debug, Deserialize)]
pub struct OsHookDefs {
    pub build: BuildHookDefs,
    pub pkg: PkgHookDefs,
}

#[derive(Debug, Deserialize)]
pub struct BuildHookDefs {
    pub init: Vec<Step>,
    #[serde(default)]
    pub cleanup: Vec<Step>,
}

#[derive(Debug, Deserialize)]
pub struct PkgHookDefs {
    pub install: ArgHook<PkgArg>,
    /// Reserved; not used by the resolver yet.
    #[serde(default)]
    pub remove: Option<ArgHook<PkgArg>>,
}
