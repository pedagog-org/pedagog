use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

const ID_PATTERN: &str = r"^[a-z][a-z0-9-]*$";
const PKG_PATTERN: &str = r"^[a-z0-9][a-z0-9+.-]*$";

#[allow(clippy::expect_used)]
static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(ID_PATTERN).expect("valid ID pattern"));
#[allow(clippy::expect_used)]
static PKG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(PKG_PATTERN).expect("valid pkg pattern"));

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

pub type OsId = Id;
pub type PlatformId = Id;
pub type ToolchainId = Id;
pub type AssignmentId = Id;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_valid() {
        assert!(Id::try_from("a".to_owned()).is_ok());
        assert!(Id::try_from("ubuntu-22".to_owned()).is_ok());
        assert!(Id::try_from("code-server".to_owned()).is_ok());
    }

    #[test]
    fn id_invalid() {
        assert!(Id::try_from("".to_owned()).is_err());
        assert!(Id::try_from("1gcc".to_owned()).is_err());
        assert!(Id::try_from("GCC".to_owned()).is_err());
        assert!(Id::try_from("gcc_13".to_owned()).is_err());
        assert!(Id::try_from("gcc 13".to_owned()).is_err());
    }

    #[test]
    fn id_display() {
        let id = Id::try_from("ubuntu-22".to_owned()).unwrap();
        assert_eq!(id.to_string(), "ubuntu-22");
        assert_eq!(id.as_str(), "ubuntu-22");
    }

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
        assert!(PkgName::try_from("".to_owned()).is_err());
        assert!(PkgName::try_from("+gcc".to_owned()).is_err());
        assert!(PkgName::try_from("GCC".to_owned()).is_err());
        assert!(PkgName::try_from("gcc 13".to_owned()).is_err());
    }

    #[test]
    fn pkgname_display() {
        let p = PkgName::try_from("g++".to_owned()).unwrap();
        assert_eq!(p.to_string(), "g++");
        assert_eq!(p.as_str(), "g++");
    }
}
