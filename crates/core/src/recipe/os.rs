use serde::Deserialize;

use super::primitives::{ArgHook, Ingredient, OsId, Step};

// ---- Arg enums ---------------------------------------------------------------

/// Valid args for pkg.install and pkg.remove hooks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PkgArg {
    Packages,
}

/// Valid args for network.transcribe hooks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscribeArg {
    Cidrs,
}

/// Uninhabited — hooks using this type accept no args.
/// `BTreeSet<NoArg>` is always empty; serde errors on any non-empty sequence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoArg {}

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
    pub network: NetworkHookDefs,
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

#[derive(Debug, Deserialize)]
pub struct NetworkHookDefs {
    pub transcribe: ArgHook<TranscribeArg>,
    pub enable:     ArgHook<NoArg>,
    pub disable:    ArgHook<NoArg>,
}
