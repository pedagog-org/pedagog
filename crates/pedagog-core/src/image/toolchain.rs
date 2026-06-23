//! A versioned toolchain definition: the install/uninstall lifecycle for a named
//! toolchain, authored as a `<id>.toml`, registered, and resolved by `build`.
//! Each schema version's types live in their own module (`v0`, …); the latest is
//! re-exported.

use magic_migrate::TryMigrate;
use std::str::FromStr;

pub use v0::{InstallPhase, Toolchain, UninstallPhase};

impl FromStr for Toolchain {
    type Err = ToolchainError;

    /// Parse a toolchain definition, migrating any older schema version forward.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match Toolchain::try_from_str_migrations(input) {
            Some(result) => Ok(result?),
            // No version in the chain matched; re-run the latest deserialize so
            // the caller gets its concrete error, not a generic "no match".
            None => Ok(toml::from_str::<Toolchain>(input)?),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolchainError {
    #[error(transparent)]
    Parse(#[from] toml::de::Error),
    #[error(transparent)]
    Migrate(#[from] magic_migrate::MigrateError),
}

/// Schema version 0.
mod v0 {
    use magic_migrate::TryMigrate;
    use semver::{Version, VersionReq};
    use serde::{Deserialize, Deserializer};

    /// A toolchain definition. Authored by an instructor (or shipped by the base
    /// image), registered into `/pedagog/config/toolchain/`, and referenced from
    /// the manifest by `id`.
    #[derive(TryMigrate, Debug, Deserialize, Clone, PartialEq, Eq)]
    #[try_migrate(from = None)]
    #[serde(deny_unknown_fields)]
    pub struct Toolchain {
        #[serde(deserialize_with = "deserialize_version")]
        pub version: Version,
        pub id: String,
        #[serde(default)]
        pub description: Option<String>,
        #[serde(default)]
        pub install: InstallPhase,
        #[serde(default)]
        pub uninstall: UninstallPhase,
    }

    /// `[install]` — what to add and run to provision the toolchain. Packages are
    /// installed first, then the commands, then the verify checks.
    #[derive(Debug, Deserialize, Clone, PartialEq, Eq, Default)]
    #[serde(deny_unknown_fields)]
    pub struct InstallPhase {
        /// apk packages, installed before the commands run.
        #[serde(default)]
        pub pkg: Vec<String>,
        /// Shell commands, run in order after the packages are installed.
        #[serde(default)]
        pub cmd: Vec<String>,
        /// Shell commands asserting the toolchain works; all must succeed.
        #[serde(default)]
        pub verify: Vec<String>,
    }

    /// `[uninstall]` — what to run when the toolchain is removed (before any
    /// package purge), e.g. tearing down the directory the install wrote to.
    #[derive(Debug, Deserialize, Clone, PartialEq, Eq, Default)]
    #[serde(deny_unknown_fields)]
    pub struct UninstallPhase {
        /// Shell commands, run on remove.
        #[serde(default)]
        pub cmd: Vec<String>,
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
                "toolchain version {version} is not supported (expected ^0.1)"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: &str = r#"
version     = "0.1.0"
id          = "rust"
description = "Rust 1.88.0 via rustup"

[install]
pkg    = ["bash", "curl", "gcc", "musl-dev"]
cmd    = ["curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.88.0"]
verify = ["cargo --version"]

[uninstall]
cmd = ["rm -rf /opt/rust"]
"#;

    fn parse(input: &str) -> Result<Toolchain, ToolchainError> {
        input.parse()
    }

    #[test]
    fn parses_full_lifecycle() {
        let tc = parse(RUST).unwrap();
        assert_eq!(tc.id, "rust");
        assert_eq!(tc.description.as_deref(), Some("Rust 1.88.0 via rustup"));
        assert_eq!(tc.install.pkg, vec!["bash", "curl", "gcc", "musl-dev"]);
        assert_eq!(tc.install.verify, vec!["cargo --version"]);
        assert_eq!(tc.uninstall.cmd, vec!["rm -rf /opt/rust"]);
    }

    #[test]
    fn phases_default_when_absent() {
        let tc = parse("version = \"0.1.0\"\nid = \"jq\"\n[install]\npkg = [\"jq\"]\n").unwrap();
        assert_eq!(tc.install.pkg, vec!["jq"]);
        assert!(tc.install.cmd.is_empty());
        assert!(tc.install.verify.is_empty());
        assert!(tc.uninstall.cmd.is_empty());
        assert!(tc.description.is_none());
    }

    #[test]
    fn accepts_compatible_patch() {
        let tc = parse("version = \"0.1.7\"\nid = \"jq\"\n").unwrap();
        assert_eq!(tc.id, "jq");
    }

    #[test]
    fn rejects_incompatible_minor() {
        let err = parse("version = \"0.2.0\"\nid = \"jq\"\n").unwrap_err();
        assert!(matches!(err, ToolchainError::Parse(_)));
    }

    #[test]
    fn rejects_future_major() {
        let err = parse("version = \"1.0.0\"\nid = \"jq\"\n").unwrap_err();
        assert!(matches!(err, ToolchainError::Parse(_)));
    }

    #[test]
    fn requires_version() {
        assert!(parse("id = \"jq\"\n[install]\npkg = [\"jq\"]\n").is_err());
    }

    #[test]
    fn requires_id() {
        assert!(parse("version = \"0.1.0\"\n[install]\npkg = [\"jq\"]\n").is_err());
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(parse("version = \"0.1.0\"\nid = \"x\"\nbogus = true\n").is_err());
    }
}
