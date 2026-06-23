//! A versioned toolchain definition: the install/uninstall lifecycle for a named
//! toolchain, authored as a `<id>.toml`, registered, and resolved by `build`.
//! Each schema version's types live in their own module (`v0`, …); the latest is
//! re-exported.

use magic_migrate::TryMigrate;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

pub use v0::{InstallPhase, Toolchain, UninstallPhase};

/// Canonical directory of registered toolchain definitions inside the image.
pub const DEFAULT_TOOLCHAINS: &str = "/pedagog/config/toolchains";

/// Whether `c` is allowed in a toolchain id.
fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')
}

/// Whether `id` is a legal toolchain id: non-empty and only ASCII alphanumerics
/// plus `.`, `-`, `_`. This also keeps an id safe to use as a `<id>.toml`
/// filename — it admits no path separators, so it cannot traverse directories.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(is_id_char)
}

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

/// Map each package to the toolchains that bring it in. `direct` packages
/// (installed without a toolchain) appear with an empty owner set; a package a
/// toolchain's `[install].pkg` lists records that toolchain's id. The owner set
/// of a package is exactly the toolchains that would break if it were removed.
/// Backs `pkg installed` (the listing) and `pkg remove` (the dependency gate).
pub fn package_dependencies<'a>(
    direct: &'a [String],
    toolchains: &'a [Toolchain],
) -> BTreeMap<&'a str, BTreeSet<&'a str>> {
    let mut owners: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for pkg in direct {
        owners.entry(pkg).or_default();
    }
    for tc in toolchains {
        for pkg in &tc.install.pkg {
            owners.entry(pkg).or_default().insert(&tc.id);
        }
    }
    owners
}

/// The toolchains whose install brings in `pkg` — i.e. those that would break if
/// it were removed. The `'static` empty `direct` keeps the result borrowed from
/// `toolchains` (so it outlives the call). Backs `pkg remove`'s dependency gate.
pub fn toolchains_requiring<'a>(pkg: &str, toolchains: &'a [Toolchain]) -> BTreeSet<&'a str> {
    const NONE: &[String] = &[];
    package_dependencies(NONE, toolchains)
        .remove(pkg)
        .unwrap_or_default()
}

/// The packages `removed`'s uninstall can safely `apk del`: those in its
/// `[install].pkg` that no *other* still-installed toolchain (`others`) and no
/// directly-installed package (`direct`) still requires. Backs `toolchain
/// remove`'s dependency-gated purge.
pub fn purgeable_packages<'a>(
    removed: &'a Toolchain,
    others: &'a [Toolchain],
    direct: &'a [String],
) -> Vec<&'a str> {
    let still_needed = package_dependencies(direct, others);
    removed
        .install
        .pkg
        .iter()
        .map(String::as_str)
        .filter(|pkg| !still_needed.contains_key(pkg))
        .collect()
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
        #[serde(deserialize_with = "deserialize_id")]
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

    /// Validate the id charset at parse time: non-empty, ASCII alphanumeric plus
    /// `.`, `-`, `_`.
    fn deserialize_id<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;

        let id = String::deserialize(deserializer)?;
        if id.is_empty() {
            return Err(D::Error::custom("toolchain id must not be empty"));
        }
        if let Some(bad) = id.chars().find(|&c| !super::is_id_char(c)) {
            return Err(D::Error::custom(format!(
                "toolchain id '{id}' contains invalid character '{bad}' (allowed: alphanumeric, '.', '-', '_')"
            )));
        }
        Ok(id)
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

    #[test]
    fn accepts_id_with_allowed_punctuation() {
        let tc = parse("version = \"0.1.0\"\nid = \"python-3.11_x\"\n").unwrap();
        assert_eq!(tc.id, "python-3.11_x");
    }

    #[test]
    fn rejects_id_with_path_separator() {
        assert!(parse("version = \"0.1.0\"\nid = \"../evil\"\n").is_err());
    }

    #[test]
    fn rejects_id_with_space() {
        assert!(parse("version = \"0.1.0\"\nid = \"bad id\"\n").is_err());
    }

    #[test]
    fn rejects_empty_id() {
        assert!(parse("version = \"0.1.0\"\nid = \"\"\n").is_err());
    }

    #[test]
    fn package_dependencies_attributes_owners() {
        let rust = parse(RUST).unwrap();
        let jq = parse("version = \"0.1.0\"\nid = \"jq\"\n[install]\npkg = [\"jq\", \"curl\"]\n").unwrap();
        let direct = vec!["ripgrep".to_owned()];
        let installed = [rust, jq];
        let deps = package_dependencies(&direct, &installed);

        // Directly-installed only: present, no owners.
        assert!(deps["ripgrep"].is_empty());
        // Shared across toolchains: both owners.
        assert_eq!(deps["curl"], BTreeSet::from(["jq", "rust"]));
        // Single owner.
        assert_eq!(deps["gcc"], BTreeSet::from(["rust"]));
        // Not installed anywhere: absent.
        assert!(!deps.contains_key("python"));
    }

    #[test]
    fn toolchains_requiring_finds_dependents() {
        let rust = parse(RUST).unwrap();
        let jq = parse("version = \"0.1.0\"\nid = \"jq\"\n[install]\npkg = [\"jq\"]\n").unwrap();
        let installed = vec![rust, jq];
        assert_eq!(
            toolchains_requiring("curl", &installed),
            BTreeSet::from(["rust"])
        );
        assert!(toolchains_requiring("ripgrep", &installed).is_empty());
    }

    #[test]
    fn purgeable_packages_purges_all_when_nothing_else_needs_them() {
        let rust = parse(RUST).unwrap();
        let purge = purgeable_packages(&rust, &[], &[]);
        assert_eq!(purge, vec!["bash", "curl", "gcc", "musl-dev"]);
    }

    #[test]
    fn purgeable_packages_keeps_those_another_toolchain_needs() {
        let rust = parse(RUST).unwrap();
        let other =
            parse("version = \"0.1.0\"\nid = \"py\"\n[install]\npkg = [\"bash\", \"curl\"]\n").unwrap();
        let others = [other];
        // bash/curl are still needed by py; gcc/musl-dev are rust-only.
        assert_eq!(purgeable_packages(&rust, &others, &[]), vec!["gcc", "musl-dev"]);
    }

    #[test]
    fn purgeable_packages_keeps_directly_installed() {
        let rust = parse(RUST).unwrap();
        let direct = vec!["gcc".to_owned()];
        let purge = purgeable_packages(&rust, &[], &direct);
        assert_eq!(purge, vec!["bash", "curl", "musl-dev"]);
    }
}
