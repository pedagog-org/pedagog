use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::network::NetworkSpec;
use super::platform::PlatformSpec;
use super::primitives::{
    AssignmentId, Id, OsId, Step, ToolchainId, ToolchainRef, Version, Versioned,
};

#[derive(Debug, Deserialize)]
pub struct AssignmentYaml {
    pub id: AssignmentId,
    pub name: String,
    pub environment: Environment,
    #[serde(default)]
    pub testing: Option<Testing>,
    #[serde(default)]
    pub files: FileSources,
}

/// Where the archive keeps the directories baked into the image. Only the
/// sources are configurable; the image paths (`/pedagog/{starter,tests,solution}`)
/// are a fixed platform contract.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSources {
    #[serde(default = "default_starter")]
    pub starter: ArchivePath,
    #[serde(default = "default_tests")]
    pub tests: ArchivePath,
    #[serde(default = "default_solution")]
    pub solution: ArchivePath,
}

impl Default for FileSources {
    fn default() -> Self {
        Self {
            starter: default_starter(),
            tests: default_tests(),
            solution: default_solution(),
        }
    }
}

fn default_starter() -> ArchivePath {
    ArchivePath("starter".to_owned())
}
fn default_tests() -> ArchivePath {
    ArchivePath("tests".to_owned())
}
fn default_solution() -> ArchivePath {
    ArchivePath("solution".to_owned())
}

#[allow(clippy::expect_used)]
static ARCHIVE_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)*$").expect("valid archive path pattern")
});

/// A directory inside the assignment archive: relative, without `.`/`..`
/// segments, and limited to plain path characters (it's written into COPY lines).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct ArchivePath(String);

impl ArchivePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ArchivePath {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let trimmed = s.trim_end_matches('/');
        let ok = ARCHIVE_PATH_RE.is_match(trimmed)
            && trimmed.split('/').all(|seg| seg != "." && seg != "..");
        if ok {
            Ok(ArchivePath(trimmed.to_owned()))
        } else {
            Err(format!(
                "invalid archive path {s:?}: must be a relative directory inside the archive (letters, digits, '_', '.', '-', '/'; no '..')"
            ))
        }
    }
}

impl fmt::Display for ArchivePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How the assignment's tests run. The framework names a pedagog test helper
/// (`pytest:1` → the `pedagog-pytest:1` toolchain), which resolve adds as an
/// addon of the selected toolchain that declares it compatible.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Testing {
    pub framework: Versioned,
    #[serde(default)]
    pub isolation: Isolation,
}

impl Testing {
    /// The helper toolchain implementing this framework.
    pub fn helper(&self) -> Result<ToolchainRef, String> {
        Ok(ToolchainRef {
            id: Id::try_from(format!("{HELPER_PREFIX}{}", self.framework.id))?,
            version: self.framework.version.clone(),
        })
    }
}

/// Helper toolchain ids are the framework id with this prefix.
pub const HELPER_PREFIX: &str = "pedagog-";

/// Whether student code shares the test harness's process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Isolation {
    /// Student code runs as `runner` in its own process; tests can't be read from it.
    #[default]
    Process,
    /// Student code runs inside the harness process (faster; can read the tests).
    InProcess,
}

impl fmt::Display for Isolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Isolation::Process => "process",
            Isolation::InProcess => "in-process",
        })
    }
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
        write!(
            f,
            "{}{}",
            self.tc,
            if self.allow_coexist { "!" } else { "" }
        )
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
        let parent = picks
            .next()
            .unwrap_or_else(|| Err(format!("empty toolchain {s:?}")))?;
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
                f.write_str(
                    r#"a toolchain like "gcc:13+gdb:14", or a map with id, version, addons"#,
                )
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

    fn assignment(extra: &str) -> Result<AssignmentYaml, String> {
        let yaml = format!(
            "id: t\nname: T\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: workspace\n{extra}"
        );
        serde_yaml::from_str(&yaml).map_err(|e| e.to_string())
    }

    #[test]
    fn file_sources_default_to_conventional_names() {
        let files = assignment("").unwrap().files;
        assert_eq!(
            [
                files.starter.as_str(),
                files.tests.as_str(),
                files.solution.as_str()
            ],
            ["starter", "tests", "solution"]
        );
    }

    #[test]
    fn file_sources_are_configurable_per_directory() {
        let files = assignment("files:\n  starter: src/\n  tests: grading/hidden\n")
            .unwrap()
            .files;
        assert_eq!(files.starter.as_str(), "src", "trailing slash trimmed");
        assert_eq!(files.tests.as_str(), "grading/hidden");
        assert_eq!(
            files.solution.as_str(),
            "solution",
            "unset keeps the default"
        );
    }

    #[test]
    fn file_sources_reject_escapes_and_odd_characters() {
        for bad in [
            "../secrets",
            "/abs",
            "a/../b",
            ".",
            "with space",
            "a\nRUN x",
            "",
        ] {
            assert!(
                ArchivePath::try_from(bad.to_owned()).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert!(assignment("files:\n  sources: x\n").is_err(), "unknown key");
    }

    #[test]
    fn testing_is_optional() {
        assert!(assignment("").unwrap().testing.is_none());
    }

    #[test]
    fn testing_defaults_to_process_isolation_and_maps_to_helper() {
        let t = assignment("testing:\n  framework: pytest:1\n")
            .unwrap()
            .testing
            .unwrap();
        assert_eq!(t.isolation, Isolation::Process);
        assert_eq!(t.helper().unwrap().to_string(), "pedagog-pytest:1");
    }

    #[test]
    fn testing_accepts_in_process_and_rejects_unknown_keys() {
        let t = assignment("testing:\n  framework: pytest:1\n  isolation: in-process\n")
            .unwrap()
            .testing
            .unwrap();
        assert_eq!(t.isolation, Isolation::InProcess);
        assert!(assignment("testing:\n  framework: pytest:1\n  isolate: process\n").is_err());
        assert!(
            assignment("testing:\n  framework: pytest\n").is_err(),
            "version required"
        );
    }

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
    fn network_defaults_to_deny() {
        let yaml = "os: ubuntu-22\nplatform:\n  kind: workspace\n";
        let env: Environment = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(env.network, NetworkSpec::Deny { allow } if allow.is_empty()));
        assert!(env.toolchains.is_empty());
        assert!(env.steps.is_empty());
    }

    #[test]
    fn network_deny_with_allowlist() {
        let yaml = "\
os: ubuntu-22
platform:
  kind: workspace
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
        let yaml = "os: ubuntu-22\nplatform:\n  kind: workspace\nnetwork:\n  mode: deny\n";
        let env: Environment = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(env.network, NetworkSpec::Deny { allow } if allow.is_empty()));
    }
}
