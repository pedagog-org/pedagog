//! The build ledger (`/pedagog/config/ledger.toml`): the record of what has been
//! provisioned in the image — directly-installed packages, and every registered
//! toolchain with whether it is installed. Read and rewritten by `pkg`,
//! `toolchain`, and `build`; `build --info` prints it. A toolchain's packages
//! live in its definition file, not here. Versioned like the manifest (`version`
//! + a `v0` module, migratable via `magic_migrate`); validated against `^0.1`.

use magic_migrate::TryMigrate;
use std::str::FromStr;

pub use v0::Ledger;

/// Canonical ledger location inside the image.
pub const DEFAULT_LEDGER: &str = "/pedagog/config/ledger.toml";

impl FromStr for Ledger {
    type Err = LedgerError;

    /// Parse the ledger, migrating any older schema version forward.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match Ledger::try_from_str_migrations(input) {
            Some(result) => Ok(result?),
            // No version in the chain matched; re-run the latest deserialize so
            // the caller gets its concrete error, not a generic "no match".
            None => Ok(toml::from_str::<Ledger>(input)?),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error(transparent)]
    Parse(#[from] toml::de::Error),
    #[error(transparent)]
    Migrate(#[from] magic_migrate::MigrateError),
}

/// Schema version 0.
mod v0 {
    use magic_migrate::TryMigrate;
    use semver::{Version, VersionReq};
    use serde::{Deserialize, Deserializer, Serialize};
    use std::collections::BTreeMap;

    /// The provisioning state of an image.
    #[derive(TryMigrate, Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
    #[try_migrate(from = None)]
    #[serde(deny_unknown_fields)]
    pub struct Ledger {
        #[serde(deserialize_with = "deserialize_version")]
        pub version: Version,
        /// Packages installed directly via `pkg` (the manifest's `additional_packages`),
        /// not owned by a toolchain. Listed before the toolchains table so the ledger
        /// serializes to valid TOML (scalar/array keys precede the `[toolchains]` table).
        #[serde(default)]
        pub additional_packages: Vec<String>,
        /// Registered toolchains, keyed by id; the value is whether it is installed.
        #[serde(default)]
        pub toolchains: BTreeMap<String, bool>,
    }

    impl Default for Ledger {
        fn default() -> Self {
            Ledger {
                version: Version::new(0, 1, 0),
                additional_packages: Vec::new(),
                toolchains: BTreeMap::new(),
            }
        }
    }

    /// Accept any version compatible with `0.1` and reject anything else. The
    /// caret requirement `^0.1` is `>= 0.1.0, < 0.2.0` — a breaking change bumps
    /// the minor and adds a new schema module.
    fn deserialize_version<'de, D>(deserializer: D) -> Result<Version, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;

        let version = Version::deserialize(deserializer)?;
        let req = VersionReq::parse("^0.1").map_err(D::Error::custom)?;
        if req.matches(&version) {
            Ok(version)
        } else {
            Err(D::Error::custom(format!(
                "ledger version {version} is not supported (expected ^0.1)"
            )))
        }
    }
}

impl Ledger {
    /// Serialize the ledger to TOML for writing back to `ledger.toml`.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Record `pkg` as directly-installed, if not already listed.
    pub fn add_package(&mut self, pkg: &str) {
        if !self.additional_packages.iter().any(|p| p == pkg) {
            self.additional_packages.push(pkg.to_owned());
        }
    }

    /// Drop a directly-installed package. Removing one the ledger doesn't track is
    /// a no-op — `pkg` may legitimately remove something it didn't install.
    pub fn remove_package(&mut self, pkg: &str) {
        self.additional_packages.retain(|p| p != pkg);
    }

    /// Register a toolchain as not-yet-installed. Idempotent: keeps the existing
    /// installed state if the toolchain is already known.
    pub fn register_toolchain(&mut self, id: &str) {
        self.toolchains.entry(id.to_owned()).or_insert(false);
    }

    /// Drop a toolchain from the ledger entirely.
    pub fn unregister_toolchain(&mut self, id: &str) {
        self.toolchains.remove(id);
    }

    /// Whether `id` is registered and installed.
    pub fn is_installed(&self, id: &str) -> bool {
        self.toolchains.get(id).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    #[test]
    fn defaults_to_current_version() {
        assert_eq!(Ledger::default().version, Version::new(0, 1, 0));
    }

    #[test]
    fn version_only_is_default() {
        let ledger = "version = \"0.1.0\"\n".parse::<Ledger>().unwrap();
        assert_eq!(ledger, Ledger::default());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut ledger = Ledger {
            additional_packages: vec!["ripgrep".to_owned()],
            ..Default::default()
        };
        ledger.register_toolchain("rust");
        ledger.toolchains.insert("python".to_owned(), true);

        let reparsed: Ledger = ledger.to_toml().unwrap().parse().unwrap();
        assert_eq!(reparsed, ledger);
    }

    #[test]
    fn accepts_compatible_patch() {
        let ledger = "version = \"0.1.7\"\n".parse::<Ledger>().unwrap();
        assert_eq!(ledger.version, Version::new(0, 1, 7));
    }

    #[test]
    fn rejects_incompatible_minor() {
        let err = "version = \"0.2.0\"\n".parse::<Ledger>().unwrap_err();
        assert!(matches!(err, LedgerError::Parse(_)));
    }

    #[test]
    fn requires_version() {
        assert!("additional_packages = [\"jq\"]\n".parse::<Ledger>().is_err());
    }

    #[test]
    fn add_package_dedups() {
        let mut ledger = Ledger::default();
        ledger.add_package("jq");
        ledger.add_package("jq");
        assert_eq!(ledger.additional_packages, vec!["jq"]);
    }

    #[test]
    fn remove_package_drops_it() {
        let mut ledger = Ledger {
            additional_packages: vec!["jq".to_owned(), "ripgrep".to_owned()],
            ..Default::default()
        };
        ledger.remove_package("jq");
        assert_eq!(ledger.additional_packages, vec!["ripgrep"]);
    }

    #[test]
    fn remove_package_unknown_is_noop() {
        let mut ledger = Ledger {
            additional_packages: vec!["jq".to_owned()],
            ..Default::default()
        };
        ledger.remove_package("nope");
        assert_eq!(ledger.additional_packages, vec!["jq"]);
    }

    #[test]
    fn register_is_idempotent_and_keeps_installed_state() {
        let mut ledger = Ledger::default();
        ledger.register_toolchain("rust");
        assert!(!ledger.is_installed("rust"));
        ledger.toolchains.insert("rust".to_owned(), true);
        // Re-registering must not reset the installed flag.
        ledger.register_toolchain("rust");
        assert!(ledger.is_installed("rust"));
    }

    #[test]
    fn unregister_drops_toolchain() {
        let mut ledger = Ledger::default();
        ledger.register_toolchain("rust");
        ledger.unregister_toolchain("rust");
        assert!(!ledger.toolchains.contains_key("rust"));
    }

    #[test]
    fn is_installed_false_when_absent() {
        assert!(!Ledger::default().is_installed("rust"));
    }
}
