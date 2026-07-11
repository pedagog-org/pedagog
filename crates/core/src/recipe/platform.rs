use super::primitives::Id;
use serde::Deserialize;

// Design TBD — placeholder until the platform hook model is defined.

#[derive(Debug, Deserialize)]
pub struct PlatformRecipe {
    pub id: Id,
    pub os: Id,
}
