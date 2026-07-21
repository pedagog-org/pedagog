pub mod command;
pub mod hook;
pub mod id;
pub mod ingredient;
pub mod param;
pub mod step;
pub mod version;

pub use hook::{ArgHook, ParamHook};
pub use id::{Id, OsId, PkgName, PlatformId, ToolchainId};
pub use ingredient::{GithubSource, Ingredient, IngredientSource};
pub use param::{ParamDef, ParamType, ParamVal};
pub use step::Step;
pub use version::{MaybeVersioned, Version, Versioned};

use serde::Deserialize;

/// Deserializes a single value or a sequence into a Vec.
/// Use with `#[serde(deserialize_with = "deserialize_one_or_many")]`.
pub(crate) fn deserialize_one_or_many<'de, T, D>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        One(T),
        Many(Vec<T>),
    }

    match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(x) => Ok(vec![x]),
        OneOrMany::Many(xs) => Ok(xs),
    }
}
