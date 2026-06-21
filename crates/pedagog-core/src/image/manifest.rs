//! The versioned `pedagog.toml` manifest: load it and migrate any older schema
//! version forward. Each schema version's types are grouped in its own module
//! (`v0`, …); the latest is re-exported as the crate's manifest types.

use magic_migrate::TryMigrate;
use std::str::FromStr;

pub use v0::{Action, Manifest, NetworkConfig, Rule};

impl FromStr for Manifest {
    type Err = ManifestError;

    /// Parse a `pedagog.toml`, migrating any older schema version forward.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match Manifest::try_from_str_migrations(input) {
            Some(result) => Ok(result?),
            // No version in the chain matched; re-run the latest deserialize so
            // the caller gets its concrete error, not a generic "no match".
            None => Ok(toml::from_str::<Manifest>(input)?),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error(transparent)]
    Parse(#[from] toml::de::Error),
    #[error(transparent)]
    Migrate(#[from] magic_migrate::MigrateError),
}

/// Schema version 0.
mod v0 {
    use ipnet::IpNet;
    use magic_migrate::TryMigrate;
    use semver::{Version, VersionReq};
    use serde::{Deserialize, Deserializer};

    #[derive(TryMigrate, Debug, Deserialize, Clone, PartialEq, Eq)]
    #[try_migrate(from = None)]
    #[serde(deny_unknown_fields)]
    pub struct Manifest {
        #[serde(deserialize_with = "deserialize_version")]
        pub version: Version,
        pub network: NetworkConfig,
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
                "manifest version {version} is not supported (expected ^0.1)"
            )))
        }
    }

    /// Student egress policy, selected by `mode`. serde picks the variant from
    /// `mode` and reads only its fields.
    #[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
    #[serde(tag = "mode", rename_all = "lowercase")]
    pub enum NetworkConfig {
        /// All student egress blocked (fail-closed default).
        Default,
        /// Blocked except these destinations.
        Block { allow: Vec<IpNet> },
        /// Allowed except these destinations.
        Open { block: Vec<IpNet> },
        /// Ordered, first-match rules; unmatched traffic is dropped.
        Custom { rules: Vec<Rule> },
    }

    /// One egress rule: an action for traffic to a destination network.
    #[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
    #[serde(deny_unknown_fields)]
    pub struct Rule {
        pub action: Action,
        #[serde(rename = "to")]
        pub target: IpNet,
    }

    /// What to do with traffic to a destination.
    #[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
    pub enum Action {
        Allow,
        Block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(s: &str) -> ipnet::IpNet {
        s.parse().unwrap()
    }

    fn parse(input: &str) -> Result<Manifest, ManifestError> {
        input.parse()
    }

    #[test]
    fn parses_default() {
        let m = parse("version = \"0.1.0\"\n[network]\nmode = \"default\"\n").unwrap();
        assert_eq!(m.network, NetworkConfig::Default);
    }

    #[test]
    fn accepts_compatible_patch() {
        let m = parse("version = \"0.1.7\"\n[network]\nmode = \"default\"\n").unwrap();
        assert_eq!(m.network, NetworkConfig::Default);
    }

    #[test]
    fn parses_block_with_allow() {
        let m =
            parse("version = \"0.1.0\"\n[network]\nmode = \"block\"\nallow = [\"10.0.0.0/24\"]\n")
                .unwrap();
        assert_eq!(
            m.network,
            NetworkConfig::Block {
                allow: vec![net("10.0.0.0/24")]
            }
        );
    }

    #[test]
    fn parses_custom_rules_in_order() {
        let toml = "version = \"0.1.0\"\n[network]\nmode = \"custom\"\n\
            [[network.rules]]\naction = \"allow\"\nto = \"10.0.0.5/32\"\n\
            [[network.rules]]\naction = \"block\"\nto = \"10.0.0.0/8\"\n";
        let m = parse(toml).unwrap();
        assert_eq!(
            m.network,
            NetworkConfig::Custom {
                rules: vec![
                    Rule {
                        action: Action::Allow,
                        target: net("10.0.0.5/32")
                    },
                    Rule {
                        action: Action::Block,
                        target: net("10.0.0.0/8")
                    },
                ]
            }
        );
    }

    #[test]
    fn rejects_incompatible_minor() {
        let err = parse("version = \"0.2.0\"\n[network]\nmode = \"default\"\n").unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    #[test]
    fn rejects_future_major() {
        let err = parse("version = \"1.0.0\"\n[network]\nmode = \"default\"\n").unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let err = parse("version = \"0.1.0\"\nbogus = true\n[network]\nmode = \"default\"\n")
            .unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    // Probe: does serde reject a field that belongs to a different mode?
    #[test]
    fn stray_field_under_mode() {
        let r = parse(
            "version = \"0.1.0\"\n[network]\nmode = \"default\"\nallow = [\"10.0.0.0/24\"]\n",
        );
        assert!(r.is_ok());
    }
}
