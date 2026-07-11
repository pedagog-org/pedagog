use std::collections::HashMap;

use serde::Deserialize;

use super::primitives::{HookDef, Id, Step};

#[derive(Debug, Deserialize)]
pub struct OsDef {
    pub id: Id,
    pub upstream: String,
    pub image: String,
    pub hooks: OsHookDefs,
}

#[derive(Debug, Deserialize)]
pub struct OsHookDefs {
    pub init: HookDef,
    pub pkg: PkgHookDefs,
    pub network: NetworkHookDefs,
}

#[derive(Debug, Deserialize)]
pub struct PkgHookDefs {
    pub install: ParamHookDef,
    pub remove: ParamHookDef,
}

#[derive(Debug, Deserialize)]
pub struct NetworkHookDefs {
    pub transcribe: ParamHookDef,
    pub enable: HookDef,
    pub disable: HookDef,
}

#[derive(Debug, Deserialize)]
pub struct ParamHookDef {
    pub params: Vec<Param>,
    pub steps: Vec<Step>,
}

// A single hook parameter declared as a one-key map: `- <id>: <example>`.
// e.g. `- packages: "gcc-13 g++-13"`
#[derive(Debug)]
pub struct Param {
    pub id: Id,
    pub example: String,
}

impl<'de> Deserialize<'de> for Param {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = HashMap::<String, String>::deserialize(deserializer)?;
        if map.len() != 1 {
            return Err(serde::de::Error::custom(
                "param must be a single-key map, e.g. `- packages: \"gcc-13 g++-13\"`",
            ));
        }
        let (id_str, example) = map.into_iter().next().unwrap();
        let id = Id::try_from(id_str).map_err(serde::de::Error::custom)?;
        Ok(Param { id, example })
    }
}
