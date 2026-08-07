//! Persistent build-state models, and (behind `feature = "db"`) Postgres access.
//!
//! The model types are always available and dependency-light so a WASM consumer
//! can depend on `store` for them; the `db` feature gates the sqlx/Postgres layer.

use strum_macros::{Display, EnumString};

#[cfg(feature = "db")]
pub mod db;

/// State of a single build attempt (`build_runs.status`).
///
/// Serialized to a lowercase `TEXT` column via strum. strum only guards the Rust
/// write path, so the migration adds a `CHECK` constraint as the DB-side backstop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum BuildStatus {
    Running,
    Succeeded,
    Failed,
}

/// A `build_runs` row, as needed by crash/restart reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildRun {
    pub id: i64,
    pub os_id: String,
    pub hash: String,
    pub image_ref: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn build_status_serializes_lowercase() {
        assert_eq!(BuildStatus::Running.to_string(), "running");
        assert_eq!(BuildStatus::Succeeded.to_string(), "succeeded");
        assert_eq!(BuildStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn build_status_roundtrips() {
        for s in [
            BuildStatus::Running,
            BuildStatus::Succeeded,
            BuildStatus::Failed,
        ] {
            assert_eq!(BuildStatus::from_str(&s.to_string()).unwrap(), s);
        }
    }

    #[test]
    fn build_status_rejects_unknown() {
        assert!(BuildStatus::from_str("bogus").is_err());
    }
}
