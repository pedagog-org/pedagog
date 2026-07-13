use serde::Deserialize;

use super::id::Id;

// Dot-separated numeric segments, coerced to semver internally.
// e.g. "13" -> 13.0.0, "3.12" -> 3.12.0
#[derive(Debug, Clone, Eq, Deserialize)]
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

impl std::hash::Hash for Version {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.semver.hash(state);
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
            3 => s.clone(),
            _ => return Err(format!(
                "invalid version {s:?}: must have 1-3 numeric segments"
            )),
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

fn split_versioned<T>(s: &str) -> Result<(T, Option<Version>), String>
where
    T: TryFrom<String, Error = String>,
{
    match s.find(':') {
        Some(pos) => {
            let id = T::try_from(s[..pos].to_owned())?;
            let version = Version::try_from(s[pos + 1..].to_owned())?;
            Ok((id, Some(version)))
        }
        None => {
            let id = T::try_from(s.to_owned())?;
            Ok((id, None))
        }
    }
}

// e.g. "gdb:14", "python:3.12" — version required
#[derive(Debug, Clone)]
pub struct Versioned<T = Id> {
    pub id: T,
    pub version: Version,
}

impl<T> TryFrom<String> for Versioned<T>
where
    T: TryFrom<String, Error = String>,
{
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        match split_versioned::<T>(&s)? {
            (id, Some(version)) => Ok(Versioned { id, version }),
            (_, None) => Err(format!("{s:?} must include a version, e.g. \"gdb:14\"")),
        }
    }
}

impl<'de, T> Deserialize<'de> for Versioned<T>
where
    T: TryFrom<String, Error = String>,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Versioned::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl<T: std::fmt::Display> std::fmt::Display for Versioned<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.id, self.version)
    }
}

// e.g. "gdb:14", "gdb" (version omitted)
#[derive(Debug, Clone)]
pub struct MaybeVersioned<T = Id> {
    pub id: T,
    pub version: Option<Version>,
}

impl<T> TryFrom<String> for MaybeVersioned<T>
where
    T: TryFrom<String, Error = String>,
{
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let (id, version) = split_versioned::<T>(&s)?;
        Ok(MaybeVersioned { id, version })
    }
}

impl<'de, T> Deserialize<'de> for MaybeVersioned<T>
where
    T: TryFrom<String, Error = String>,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        MaybeVersioned::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl<T: std::fmt::Display> std::fmt::Display for MaybeVersioned<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.version {
            Some(v) => write!(f, "{}:{}", self.id, v),
            None => self.id.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_valid() {
        assert!(Version::try_from("13".to_owned()).is_ok());
        assert!(Version::try_from("3.12".to_owned()).is_ok());
        assert!(Version::try_from("1.79.0".to_owned()).is_ok());
    }

    #[test]
    fn version_invalid() {
        assert!(Version::try_from("".to_owned()).is_err());
        assert!(Version::try_from("a.b".to_owned()).is_err());
        assert!(Version::try_from("1.".to_owned()).is_err());
        assert!(Version::try_from("1.2.3.4".to_owned()).is_err());
    }

    #[test]
    fn version_ordering() {
        let v1 = Version::try_from("1".to_owned()).unwrap();
        let v2 = Version::try_from("2".to_owned()).unwrap();
        let v1_9 = Version::try_from("1.9".to_owned()).unwrap();
        let v1_10 = Version::try_from("1.10".to_owned()).unwrap();

        assert!(v1 < v2);
        assert!(v1_9 < v1_10);
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

    #[test]
    fn version_serde_path() {
        let v = Version::try_from("3.12".to_owned()).unwrap();
        assert_eq!(v.as_str(), "3.12");
        assert_eq!(v.semver().major, 3);
        assert_eq!(v.semver().minor, 12);
    }

    #[test]
    fn versioned_valid() {
        let v = Versioned::<Id>::try_from("gdb:14".to_owned()).unwrap();
        assert_eq!(v.id.as_str(), "gdb");
        assert_eq!(v.version.as_str(), "14");
    }

    #[test]
    fn versioned_requires_version() {
        assert!(Versioned::<Id>::try_from("gdb".to_owned()).is_err());
    }

    #[test]
    fn versioned_display() {
        let v = Versioned::<Id>::try_from("python:3.12".to_owned()).unwrap();
        assert_eq!(v.to_string(), "python:3.12");
    }

    #[test]
    fn versioned_invalid() {
        assert!(Versioned::<Id>::try_from("GDB:14".to_owned()).is_err());
        assert!(Versioned::<Id>::try_from("gdb:bad".to_owned()).is_err());
    }

    #[test]
    fn maybe_versioned_with_version() {
        let v = MaybeVersioned::<Id>::try_from("gdb:14".to_owned()).unwrap();
        assert_eq!(v.id.as_str(), "gdb");
        assert_eq!(v.version.unwrap().as_str(), "14");
    }

    #[test]
    fn maybe_versioned_without_version() {
        let v = MaybeVersioned::<Id>::try_from("gdb".to_owned()).unwrap();
        assert_eq!(v.id.as_str(), "gdb");
        assert!(v.version.is_none());
    }

    #[test]
    fn maybe_versioned_display() {
        let v = MaybeVersioned::<Id>::try_from("python:3.12".to_owned()).unwrap();
        assert_eq!(v.to_string(), "python:3.12");

        let v = MaybeVersioned::<Id>::try_from("gdb".to_owned()).unwrap();
        assert_eq!(v.to_string(), "gdb");
    }

    #[test]
    fn maybe_versioned_invalid() {
        assert!(MaybeVersioned::<Id>::try_from("GDB:14".to_owned()).is_err());
        assert!(MaybeVersioned::<Id>::try_from("gdb:bad".to_owned()).is_err());
    }
}
