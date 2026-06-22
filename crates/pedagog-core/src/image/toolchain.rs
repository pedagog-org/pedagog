//! A registered toolchain definition: the install/uninstall lifecycle for a
//! named toolchain, authored as a `<id>.toml` and resolved by `build`.

use serde::Deserialize;
use std::str::FromStr;

/// A toolchain definition. Authored by an instructor, registered into
/// `/pedagog/config/toolchain/`, and referenced from the manifest by `id`.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Toolchain {
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

/// `[uninstall]` — what to run when the toolchain is removed (before any package
/// purge), e.g. tearing down the directory the install wrote to.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct UninstallPhase {
    /// Shell commands, run on remove.
    #[serde(default)]
    pub cmd: Vec<String>,
}

impl FromStr for Toolchain {
    type Err = toml::de::Error;

    /// Parse a toolchain definition from its TOML.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        toml::from_str(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: &str = r#"
id          = "rust"
description = "Rust 1.88.0 via rustup"

[install]
pkg    = ["bash", "curl", "gcc", "musl-dev"]
cmd    = ["curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.88.0"]
verify = ["cargo --version"]

[uninstall]
cmd = ["rm -rf /opt/rust"]
"#;

    #[test]
    fn parses_full_lifecycle() {
        let tc: Toolchain = RUST.parse().unwrap();
        assert_eq!(tc.id, "rust");
        assert_eq!(tc.description.as_deref(), Some("Rust 1.88.0 via rustup"));
        assert_eq!(tc.install.pkg, vec!["bash", "curl", "gcc", "musl-dev"]);
        assert_eq!(tc.install.verify, vec!["cargo --version"]);
        assert_eq!(tc.uninstall.cmd, vec!["rm -rf /opt/rust"]);
    }

    #[test]
    fn phases_default_when_absent() {
        let tc: Toolchain = "id = \"jq\"\n[install]\npkg = [\"jq\"]\n".parse().unwrap();
        assert_eq!(tc.install.pkg, vec!["jq"]);
        assert!(tc.install.cmd.is_empty());
        assert!(tc.install.verify.is_empty());
        assert!(tc.uninstall.cmd.is_empty());
        assert!(tc.description.is_none());
    }

    #[test]
    fn requires_id() {
        assert!("[install]\npkg = [\"jq\"]\n".parse::<Toolchain>().is_err());
    }

    #[test]
    fn rejects_unknown_field() {
        assert!("id = \"x\"\nbogus = true\n".parse::<Toolchain>().is_err());
    }
}
