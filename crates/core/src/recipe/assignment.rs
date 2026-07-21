use serde::Deserialize;

use super::platform::PlatformSpec;
use super::primitives::{AssignmentId, OsId, Step, ToolchainRef};

#[derive(Debug, Deserialize)]
pub struct AssignmentYaml {
    pub id: AssignmentId,
    pub name: String,
    pub environment: Environment,
}

#[derive(Debug, Deserialize)]
pub struct Environment {
    pub os: OsId,
    pub platform: PlatformSpec,
    #[serde(default)]
    pub toolchains: Vec<ToolchainRef>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub network: NetworkSpec,
}

/// Build-time egress policy. `Allow` is unrestricted (no os_configure layer, no
/// runtime privilege drop); `Deny` restricts egress to the `allow` CIDR list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum NetworkSpec {
    #[default]
    Allow,
    Deny {
        #[serde(default)]
        allow: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_defaults_to_allow() {
        let yaml = "os: ubuntu-22\nplatform:\n  kind: interactive\n";
        let env: Environment = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(env.network, NetworkSpec::Allow));
        assert!(env.toolchains.is_empty());
        assert!(env.steps.is_empty());
    }

    #[test]
    fn network_deny_with_allowlist() {
        let yaml = "\
os: ubuntu-22
platform:
  kind: interactive
network:
  mode: deny
  allow: [\"10.0.0.0/8\", \"192.168.0.0/16\"]
";
        let env: Environment = serde_yaml::from_str(yaml).unwrap();
        match env.network {
            NetworkSpec::Deny { allow } => assert_eq!(allow.len(), 2),
            NetworkSpec::Allow => panic!("expected deny"),
        }
    }

    #[test]
    fn network_deny_defaults_empty_allowlist() {
        let yaml = "os: ubuntu-22\nplatform:\n  kind: interactive\nnetwork:\n  mode: deny\n";
        let env: Environment = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(env.network, NetworkSpec::Deny { allow } if allow.is_empty()));
    }
}
