use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

const ID_PATTERN: &str = r"^[a-z][a-z0-9-]*$";
const PKG_PATTERN: &str = r"^[a-z0-9][a-z0-9+.-]*$";

static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(ID_PATTERN).unwrap());
static PKG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(PKG_PATTERN).unwrap());

// ---- Id ---------------------------------------------------------------------

// e.g. "ubuntu-22", "gcc", "code-server"
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct Id(String);

impl Id {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Id {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        if ID_RE.is_match(&s) {
            Ok(Id(s))
        } else {
            Err(format!("invalid id {s:?}: must match {ID_PATTERN}"))
        }
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// ---- PkgName ----------------------------------------------------------------

// e.g. "gcc-13", "g++", "libssl-dev", "python3.12"
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct PkgName(String);

impl PkgName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PkgName {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        if PKG_RE.is_match(&s) {
            Ok(PkgName(s))
        } else {
            Err(format!("invalid package name {s:?}: must match {PKG_PATTERN}"))
        }
    }
}

impl std::fmt::Display for PkgName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// ---- Version ----------------------------------------------------------------

// Dot-separated numeric segments, coerced to semver internally.
// e.g. "13" -> 13.0.0, "3.12" -> 3.12.0
#[derive(Debug, Clone, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct Version {
    raw: String,
    #[serde(skip)]
    semver: semver::Version,
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.semver == other.semver
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.semver.cmp(&other.semver)
    }
}

impl Version {
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn semver(&self) -> &semver::Version {
        &self.semver
    }
}

impl TryFrom<String> for Version {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let segments: Vec<&str> = s.split('.').collect();
        for seg in &segments {
            if seg.is_empty() || !seg.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!(
                    "invalid version {s:?}: must be dot-separated numeric segments, e.g. \"13\" or \"3.12\""
                ));
            }
        }
        let padded = match segments.len() {
            1 => format!("{}.0.0", segments[0]),
            2 => format!("{}.{}.0", segments[0], segments[1]),
            _ => s.clone(),
        };
        let semver = semver::Version::parse(&padded)
            .map_err(|e| format!("invalid version {s:?}: {e}"))?;
        Ok(Version { raw: s, semver })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.raw.fmt(f)
    }
}

// ---- Versioned / MaybeVersioned ---------------------------------------------

fn split_versioned(s: &str) -> Result<(Id, Option<Version>), String> {
    match s.find(':') {
        Some(pos) => {
            let id = Id::try_from(s[..pos].to_owned())?;
            let version = Version::try_from(s[pos + 1..].to_owned())?;
            Ok((id, Some(version)))
        }
        None => {
            let id = Id::try_from(s.to_owned())?;
            Ok((id, None))
        }
    }
}

// e.g. "gdb:14", "python:3.12" — version required
#[derive(Debug, Clone)]
pub struct Versioned {
    pub id: Id,
    pub version: Version,
}

impl TryFrom<String> for Versioned {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        match split_versioned(&s)? {
            (id, Some(version)) => Ok(Versioned { id, version }),
            (_, None) => Err(format!("{s:?} must include a version, e.g. \"gdb:14\"")),
        }
    }
}

impl<'de> Deserialize<'de> for Versioned {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Versioned::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for Versioned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.id, self.version)
    }
}

// e.g. "gdb:14", "gdb" (version omitted)
#[derive(Debug, Clone)]
pub struct MaybeVersioned {
    pub id: Id,
    pub version: Option<Version>,
}

impl TryFrom<String> for MaybeVersioned {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let (id, version) = split_versioned(&s)?;
        Ok(MaybeVersioned { id, version })
    }
}

impl<'de> Deserialize<'de> for MaybeVersioned {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        MaybeVersioned::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for MaybeVersioned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.version {
            Some(v) => write!(f, "{}:{}", self.id, v),
            None => self.id.fmt(f),
        }
    }
}

// ---- Step -------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Install {
        #[serde(default)]
        name: Option<String>,
        packages: Vec<PkgName>,
    },
    Run {
        #[serde(default)]
        name: Option<String>,
        run: Vec<String>,
    },
}

// ---- HookDef -------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct HookDef {
    pub steps: Vec<Step>,
}

// ---- ParamVal ---------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ParamVal {
    Bool(bool),
    Int(i64),
    Str(String),
    List(Vec<ParamVal>),
    Map(HashMap<String, ParamVal>),
}

// ---- deserialize_one_or_many ------------------------------------------------

/// Deserializes a single value or a sequence into a Vec.
/// Use with `#[serde(deserialize_with = "deserialize_one_or_many")]`.
pub(crate) fn deserialize_one_or_many<'de, T, D>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        One(T),
        Many(Vec<T>),
    }

    match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(x) => Ok(vec![x]),
        OneOrMany::Many(xs) => Ok(xs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Id -----------------------------------------------------------------

    #[test]
    fn id_valid() {
        assert!(Id::try_from("a".to_owned()).is_ok());
        assert!(Id::try_from("ubuntu-22".to_owned()).is_ok());
        assert!(Id::try_from("code-server".to_owned()).is_ok());
    }

    #[test]
    fn id_invalid() {
        assert!(Id::try_from("".to_owned()).is_err());         // empty
        assert!(Id::try_from("1gcc".to_owned()).is_err());     // starts with digit
        assert!(Id::try_from("GCC".to_owned()).is_err());      // uppercase
        assert!(Id::try_from("gcc_13".to_owned()).is_err());   // underscore
        assert!(Id::try_from("gcc 13".to_owned()).is_err());   // space
    }

    #[test]
    fn id_display() {
        let id = Id::try_from("ubuntu-22".to_owned()).unwrap();
        assert_eq!(id.to_string(), "ubuntu-22");
        assert_eq!(id.as_str(), "ubuntu-22");
    }

    // ---- PkgName ------------------------------------------------------------

    #[test]
    fn pkgname_valid() {
        assert!(PkgName::try_from("gcc-13".to_owned()).is_ok());
        assert!(PkgName::try_from("g++".to_owned()).is_ok());
        assert!(PkgName::try_from("libssl-dev".to_owned()).is_ok());
        assert!(PkgName::try_from("python3.12".to_owned()).is_ok());
        assert!(PkgName::try_from("2048-game".to_owned()).is_ok());
    }

    #[test]
    fn pkgname_invalid() {
        assert!(PkgName::try_from("".to_owned()).is_err());       // empty
        assert!(PkgName::try_from("+gcc".to_owned()).is_err());   // starts with +
        assert!(PkgName::try_from("GCC".to_owned()).is_err());    // uppercase
        assert!(PkgName::try_from("gcc 13".to_owned()).is_err()); // space
    }

    #[test]
    fn pkgname_display() {
        let p = PkgName::try_from("g++".to_owned()).unwrap();
        assert_eq!(p.to_string(), "g++");
        assert_eq!(p.as_str(), "g++");
    }

    // ---- Version ------------------------------------------------------------

    #[test]
    fn version_valid() {
        assert!(Version::try_from("13".to_owned()).is_ok());
        assert!(Version::try_from("3.12".to_owned()).is_ok());
        assert!(Version::try_from("1.79.0".to_owned()).is_ok());
    }

    #[test]
    fn version_invalid() {
        assert!(Version::try_from("".to_owned()).is_err());        // empty
        assert!(Version::try_from("a.b".to_owned()).is_err());     // non-numeric
        assert!(Version::try_from("1.".to_owned()).is_err());      // trailing dot
        assert!(Version::try_from("1.2.3.4".to_owned()).is_err()); // too many segments
    }

    #[test]
    fn version_ordering() {
        let v1 = Version::try_from("1".to_owned()).unwrap();
        let v2 = Version::try_from("2".to_owned()).unwrap();
        let v1_9 = Version::try_from("1.9".to_owned()).unwrap();
        let v1_10 = Version::try_from("1.10".to_owned()).unwrap();

        assert!(v1 < v2);
        assert!(v1_9 < v1_10); // semver ordering, not lexicographic
    }

    #[test]
    fn version_equality_semver_based() {
        let a = Version::try_from("13".to_owned()).unwrap();
        let b = Version::try_from("13.0".to_owned()).unwrap();
        let c = Version::try_from("13.0.0".to_owned()).unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn version_display_preserves_raw() {
        let v = Version::try_from("13".to_owned()).unwrap();
        assert_eq!(v.to_string(), "13");

        let v = Version::try_from("3.12".to_owned()).unwrap();
        assert_eq!(v.to_string(), "3.12");
    }

    // TryFrom<String> is the serde deserialization path (#[serde(try_from = "String")])
    #[test]
    fn version_serde_path() {
        let v = Version::try_from("3.12".to_owned()).unwrap();
        assert_eq!(v.as_str(), "3.12");
        assert_eq!(v.semver().major, 3);
        assert_eq!(v.semver().minor, 12);
    }

    // ---- Versioned ----------------------------------------------------------

    #[test]
    fn versioned_valid() {
        let v = Versioned::try_from("gdb:14".to_owned()).unwrap();
        assert_eq!(v.id.as_str(), "gdb");
        assert_eq!(v.version.as_str(), "14");
    }

    #[test]
    fn versioned_requires_version() {
        assert!(Versioned::try_from("gdb".to_owned()).is_err());
    }

    #[test]
    fn versioned_display() {
        let v = Versioned::try_from("python:3.12".to_owned()).unwrap();
        assert_eq!(v.to_string(), "python:3.12");
    }

    #[test]
    fn versioned_invalid() {
        assert!(Versioned::try_from("GDB:14".to_owned()).is_err()); // uppercase id
        assert!(Versioned::try_from("gdb:bad".to_owned()).is_err()); // non-numeric version
    }

    // ---- MaybeVersioned -----------------------------------------------------

    #[test]
    fn maybe_versioned_with_version() {
        let v = MaybeVersioned::try_from("gdb:14".to_owned()).unwrap();
        assert_eq!(v.id.as_str(), "gdb");
        assert_eq!(v.version.unwrap().as_str(), "14");
    }

    #[test]
    fn maybe_versioned_without_version() {
        let v = MaybeVersioned::try_from("gdb".to_owned()).unwrap();
        assert_eq!(v.id.as_str(), "gdb");
        assert!(v.version.is_none());
    }

    #[test]
    fn maybe_versioned_display() {
        let v = MaybeVersioned::try_from("python:3.12".to_owned()).unwrap();
        assert_eq!(v.to_string(), "python:3.12");

        let v = MaybeVersioned::try_from("gdb".to_owned()).unwrap();
        assert_eq!(v.to_string(), "gdb");
    }

    #[test]
    fn maybe_versioned_invalid() {
        assert!(MaybeVersioned::try_from("GDB:14".to_owned()).is_err()); // uppercase id
        assert!(MaybeVersioned::try_from("gdb:bad".to_owned()).is_err()); // non-numeric version
    }
}
