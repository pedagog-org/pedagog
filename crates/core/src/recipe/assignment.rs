use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::network::NetworkSpec;
use super::platform::PlatformSpec;
use super::primitives::{AssignmentId, OsId, Step, ToolchainId, ToolchainRef, Version};

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
    pub toolchains: Vec<ToolchainSelection>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub network: NetworkSpec,
}

/// A pinned toolchain reference, optionally marked to install alongside other
/// versions of the same toolchain even if its recipe doesn't declare `coexist`.
/// Written `gdb:15`, or `gdb:15!` when marked.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct ToolchainPick {
    pub tc: ToolchainRef,
    pub allow_coexist: bool,
}

impl TryFrom<String> for ToolchainPick {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let (raw, allow_coexist) = match s.strip_suffix('!') {
            Some(raw) => (raw.to_owned(), true),
            None => (s, false),
        };
        Ok(ToolchainPick {
            tc: ToolchainRef::try_from(raw)?,
            allow_coexist,
        })
    }
}

impl fmt::Display for ToolchainPick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.tc, if self.allow_coexist { "!" } else { "" })
    }
}

/// A pinned toolchain plus the pinned addons chosen from its declared extras.
/// Written as `gcc:13+gdb:14+valgrind:3` or as a map with `id`/`version`/
/// `allow_coexist`/`addons`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainSelection {
    pub parent: ToolchainPick,
    pub addons: Vec<ToolchainPick>,
}

impl TryFrom<&str> for ToolchainSelection {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let mut picks = s.split('+').map(|part| {
            if part.is_empty() {
                return Err(format!("empty segment in toolchain {s:?}"));
            }
            ToolchainPick::try_from(part.to_owned())
        });
        // `split` always yields at least one item.
        let parent = picks.next().unwrap_or_else(|| Err(format!("empty toolchain {s:?}")))?;
        Ok(ToolchainSelection {
            parent,
            addons: picks.collect::<Result<_, _>>()?,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSelection {
    id: ToolchainId,
    version: Version,
    #[serde(default)]
    allow_coexist: bool,
    #[serde(default)]
    addons: Vec<ToolchainPick>,
}

// Hand-written rather than `#[serde(untagged)]`, which would replace the
// specific shorthand parse error with "did not match any variant".
impl<'de> Deserialize<'de> for ToolchainSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SelectionVisitor;

        impl<'de> Visitor<'de> for SelectionVisitor {
            type Value = ToolchainSelection;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(r#"a toolchain like "gcc:13+gdb:14", or a map with id, version, addons"#)
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                ToolchainSelection::try_from(s).map_err(E::custom)
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                let raw = RawSelection::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(ToolchainSelection {
                    parent: ToolchainPick {
                        tc: ToolchainRef {
                            id: raw.id,
                            version: raw.version,
                        },
                        allow_coexist: raw.allow_coexist,
                    },
                    addons: raw.addons,
                })
            }
        }

        deserializer.deserialize_any(SelectionVisitor)
    }
}

impl fmt::Display for ToolchainSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.parent)?;
        for addon in &self.addons {
            write!(f, "+{addon}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selections(yaml: &str) -> Result<Vec<ToolchainSelection>, String> {
        serde_yaml::from_str(yaml).map_err(|e| e.to_string())
    }

    #[test]
    fn shorthand_parent_only() {
        let s = selections("[gcc:12]").unwrap();
        assert_eq!(s[0].to_string(), "gcc:12");
        assert!(s[0].addons.is_empty());
    }

    #[test]
    fn shorthand_with_addons_including_same_addon_twice() {
        let s = selections("- gcc:13+gdb:14+valgrind:3.22+gdb:15\n").unwrap();
        assert_eq!(s[0].parent.to_string(), "gcc:13");
        let addons: Vec<String> = s[0].addons.iter().map(ToString::to_string).collect();
        assert_eq!(addons, ["gdb:14", "valgrind:3.22", "gdb:15"]);
    }

    #[test]
    fn bang_marks_allow_coexist_in_shorthand() {
        let s = selections("[gcc:13!+gdb:15!+valgrind:3]").unwrap();
        assert!(s[0].parent.allow_coexist);
        assert!(s[0].addons[0].allow_coexist);
        assert!(!s[0].addons[1].allow_coexist);
        assert_eq!(s[0].to_string(), "gcc:13!+gdb:15!+valgrind:3");
    }

    #[test]
    fn object_form_allow_coexist_and_marked_addons() {
        let object = selections(
            "- id: gcc\n  version: \"13\"\n  allow_coexist: true\n  addons: [gdb:15!]\n",
        )
        .unwrap();
        assert_eq!(object, selections("[gcc:13!+gdb:15!]").unwrap());
    }

    #[test]
    fn misplaced_bang_is_rejected() {
        assert!(selections("[gcc:13!!]").is_err());
        assert!(selections("[gcc!:13]").is_err());
        assert!(selections("- id: gcc\n  version: \"13!\"\n").is_err());
    }

    #[test]
    fn shorthand_equals_object_form() {
        let short = selections("[gcc:13+gdb:14]").unwrap();
        let object = selections("- id: gcc\n  version: \"13\"\n  addons: [gdb:14]\n").unwrap();
        assert_eq!(short, object);
    }

    #[test]
    fn shorthand_requires_versions() {
        let err = selections("[gcc:13+gdb]").unwrap_err();
        assert!(err.contains("must include a version"), "{err}");
        assert!(selections("[gcc+gdb:14]").is_err());
    }

    #[test]
    fn shorthand_rejects_empty_segments_and_bad_ids() {
        let err = selections("[gcc:13++gdb:14]").unwrap_err();
        assert!(err.contains("empty segment"), "{err}");
        assert!(selections("[gcc:13+]").is_err());
        assert!(selections("[GCC:13]").is_err());
    }

    #[test]
    fn object_form_rejects_unknown_keys_and_missing_version() {
        assert!(selections("- id: gcc\n  version: \"13\"\n  adons: [gdb:14]\n").is_err());
        assert!(selections("- id: gcc\n").is_err());
    }

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
  allow:
    - cidr: 10.0.0.0/8
    - host: mirror.internal
      ports: [80]
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
