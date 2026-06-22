//! The resolved build ledger (`/pedagog/config/build.toml`): the single record of
//! what `build`, `toolchain`, and `pkg` have installed in the image. Read and
//! rewritten by those verbs; `build --info` prints it.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

/// The resolved provisioning state of an image.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildState {
    /// Packages installed directly via `pkg` (the manifest's `additional_packages`),
    /// not owned by a toolchain. Listed before the toolchain tables so the ledger
    /// serializes to valid TOML (array keys precede `[toolchains.*]` tables).
    #[serde(default)]
    pub additional_packages: Vec<String>,
    /// Installed toolchains, keyed by id; each records the packages it brought in
    /// (so removal can be dependency-gated).
    #[serde(default)]
    pub toolchains: BTreeMap<String, ToolchainRecord>,
}

/// What an installed toolchain brought in, recorded at install time.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainRecord {
    #[serde(default)]
    pub packages: Vec<String>,
}

impl FromStr for BuildState {
    type Err = toml::de::Error;

    /// Parse the ledger from `build.toml`.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        toml::from_str(input)
    }
}

/// Why a removal can't proceed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RemoveError {
    /// A package can't be removed because installed toolchains still need it.
    #[error("package `{pkg}` is required by installed toolchain(s): {}", .toolchains.join(", "))]
    PackageDependedOn {
        pkg: String,
        toolchains: Vec<String>,
    },
    /// The named toolchain isn't in the ledger.
    #[error("toolchain `{0}` is not installed")]
    ToolchainNotInstalled(String),
}

impl BuildState {
    /// Serialize the ledger to TOML for writing back to `build.toml`.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Installed toolchains that list `pkg` among their packages.
    fn toolchains_requiring(&self, pkg: &str) -> Vec<String> {
        self.toolchains
            .iter()
            .filter(|(_, record)| record.packages.iter().any(|p| p == pkg))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Whether `pkg` is still needed by any toolchain or by a directly-installed
    /// package — i.e. it must not be purged.
    fn is_required(&self, pkg: &str) -> bool {
        self.additional_packages.iter().any(|p| p == pkg)
            || self.toolchains.values().any(|r| r.packages.iter().any(|p| p == pkg))
    }

    /// Drop a directly-installed package from the ledger. Errors if an installed
    /// toolchain depends on it (the caller must not `apk del` it then). Removing a
    /// package the ledger doesn't track is a no-op success — `pkg` may legitimately
    /// remove something it didn't install.
    pub fn remove_package(&mut self, pkg: &str) -> Result<(), RemoveError> {
        let dependents = self.toolchains_requiring(pkg);
        if !dependents.is_empty() {
            return Err(RemoveError::PackageDependedOn {
                pkg: pkg.to_owned(),
                toolchains: dependents,
            });
        }
        self.additional_packages.retain(|p| p != pkg);
        Ok(())
    }

    /// Drop a toolchain from the ledger, returning the packages now safe to purge:
    /// the removed toolchain's packages that no *remaining* toolchain and no
    /// directly-installed package still requires.
    pub fn remove_toolchain(&mut self, id: &str) -> Result<Vec<String>, RemoveError> {
        let record = self
            .toolchains
            .remove(id)
            .ok_or_else(|| RemoveError::ToolchainNotInstalled(id.to_owned()))?;
        Ok(record
            .packages
            .into_iter()
            .filter(|pkg| !self.is_required(pkg))
            .collect())
    }

    /// Every installed package — directly-installed and toolchain-owned — paired
    /// with the toolchains that require it (empty = directly-installed only).
    /// Sorted by package name. Backs `pkg installed`.
    pub fn installed_packages(&self) -> Vec<(String, Vec<String>)> {
        let mut names: BTreeSet<&str> =
            self.additional_packages.iter().map(String::as_str).collect();
        for record in self.toolchains.values() {
            names.extend(record.packages.iter().map(String::as_str));
        }
        names
            .into_iter()
            .map(|name| (name.to_owned(), self.toolchains_requiring(name)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        let state = "".parse::<BuildState>().unwrap();
        assert_eq!(state, BuildState::default());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut state = BuildState {
            additional_packages: vec!["ripgrep".to_owned()],
            ..Default::default()
        };
        state.toolchains.insert(
            "rust".to_owned(),
            ToolchainRecord {
                packages: vec!["bash".to_owned(), "curl".to_owned()],
            },
        );

        let reparsed: BuildState = state.to_toml().unwrap().parse().unwrap();
        assert_eq!(reparsed, state);
    }

    #[test]
    fn records_toolchain_packages() {
        let toml = "additional_packages = [\"jq\"]\n\
            [toolchains.rust]\npackages = [\"bash\", \"curl\"]\n";
        let state: BuildState = toml.parse().unwrap();
        assert_eq!(state.toolchains["rust"].packages, vec!["bash", "curl"]);
        assert_eq!(state.additional_packages, vec!["jq"]);
    }

    /// A ledger with toolchain `rust` (bash, curl) and direct packages (jq, curl).
    fn fixture() -> BuildState {
        let toml = "additional_packages = [\"jq\", \"curl\"]\n\
            [toolchains.rust]\npackages = [\"bash\", \"curl\"]\n";
        toml.parse().unwrap()
    }

    #[test]
    fn remove_package_drops_untracked_dependency() {
        let mut state = fixture();
        // jq isn't needed by a toolchain -> removable.
        assert_eq!(state.remove_package("jq"), Ok(()));
        assert_eq!(state.additional_packages, vec!["curl"]);
    }

    #[test]
    fn remove_package_refuses_toolchain_dependency() {
        let mut state = fixture();
        // curl is a directly-installed package, but rust also needs it -> refuse.
        let err = state.remove_package("curl").unwrap_err();
        assert_eq!(
            err,
            RemoveError::PackageDependedOn {
                pkg: "curl".to_owned(),
                toolchains: vec!["rust".to_owned()],
            }
        );
        // Unchanged on error.
        assert_eq!(state.additional_packages, vec!["jq", "curl"]);
    }

    #[test]
    fn remove_package_unknown_is_noop_ok() {
        let mut state = fixture();
        assert_eq!(state.remove_package("nope"), Ok(()));
        assert_eq!(state.additional_packages, vec!["jq", "curl"]);
    }

    #[test]
    fn remove_toolchain_purges_only_unshared_packages() {
        let mut state = fixture();
        // bash: only rust needs it -> purgeable. curl: a direct package needs it -> kept.
        let purgeable = state.remove_toolchain("rust").unwrap();
        assert_eq!(purgeable, vec!["bash"]);
        assert!(!state.toolchains.contains_key("rust"));
    }

    #[test]
    fn remove_toolchain_keeps_packages_shared_with_another_toolchain() {
        let mut state = fixture();
        state.toolchains.insert(
            "go".to_owned(),
            ToolchainRecord {
                packages: vec!["bash".to_owned()],
            },
        );
        // bash now also required by go -> not purgeable when removing rust.
        let purgeable = state.remove_toolchain("rust").unwrap();
        assert!(purgeable.is_empty());
    }

    #[test]
    fn remove_toolchain_not_installed_errors() {
        let mut state = fixture();
        assert_eq!(
            state.remove_toolchain("python"),
            Err(RemoveError::ToolchainNotInstalled("python".to_owned()))
        );
    }

    #[test]
    fn installed_packages_attributes_toolchains() {
        // fixture: rust -> {bash, curl}; direct -> {jq, curl}.
        let listing = fixture().installed_packages();
        assert_eq!(
            listing,
            vec![
                ("bash".to_owned(), vec!["rust".to_owned()]), // toolchain-owned
                ("curl".to_owned(), vec!["rust".to_owned()]), // direct AND rust -> attributed
                ("jq".to_owned(), vec![]),                    // direct only
            ]
        );
    }

    #[test]
    fn installed_packages_lists_a_package_shared_by_two_toolchains() {
        let mut state = fixture();
        state.toolchains.insert(
            "go".to_owned(),
            ToolchainRecord {
                packages: vec!["curl".to_owned()],
            },
        );
        let curl = state
            .installed_packages()
            .into_iter()
            .find(|(name, _)| name == "curl")
            .unwrap();
        assert_eq!(curl.1, vec!["go".to_owned(), "rust".to_owned()]);
    }
}
