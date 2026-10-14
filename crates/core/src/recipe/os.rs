use serde::Deserialize;

use super::primitives::{ArgHook, Ingredient, OsId, Step};

// ---- Arg enums ---------------------------------------------------------------

/// Valid args for pkg.install and pkg.remove hooks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
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
    pub remove:  ArgHook<PkgArg>,
}
