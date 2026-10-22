//! The `submission:` block: which files make up a submission, whatever its
//! source. Intake applies these rules to every upload; uploads of the
//! assignment check that they're consistent.

use std::fmt;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize, Serializer};

/// OS clutter dropped from every submission before the rules apply.
pub const JUNK: [&str; 3] = ["__MACOSX", ".DS_Store", "Thumbs.db"];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(try_from = "RawRules")]
pub struct SubmissionRules {
    /// Absent: every file is included.
    pub include: Option<Patterns>,
    pub exclude: Patterns,
    pub unmatched: Unmatched,
    pub archives: Archives,
    pub strip: Strip,
    pub required: Vec<RelPath>,
    pub max_size: Option<ByteSize>,
}

impl SubmissionRules {
    /// Whether a file at this relative path is kept: included (or no
    /// `include`) and not excluded. Junk is handled separately (`is_junk`).
    pub fn accepts(&self, path: &str) -> bool {
        self.include.as_ref().is_none_or(|p| p.matches(path)) && !self.exclude.matches(path)
    }
}

/// Whether a relative path is OS clutter, or lies inside it: including the
/// `._name` AppleDouble files macOS tar and zip write beside every file.
pub fn is_junk(path: &str) -> bool {
    path.split('/')
        .any(|seg| JUNK.contains(&seg) || seg.starts_with("._"))
}

/// Serialized back to the `submission:` form it was read from, so a parsed
/// block can be stored and read again.
impl Serialize for SubmissionRules {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RawRules {
            include: self.include.as_ref().map(|p| p.sources.clone()),
            exclude: self.exclude.sources.clone(),
            unmatched: self.unmatched,
            archives: self.archives,
            strip: self.strip,
            required: self.required.clone(),
            max_size: self.max_size,
        }
        .serialize(serializer)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawRules {
    include: Option<Vec<String>>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    unmatched: Unmatched,
    #[serde(default)]
    archives: Archives,
    #[serde(default)]
    strip: Strip,
    #[serde(default)]
    required: Vec<RelPath>,
    max_size: Option<ByteSize>,
}

impl TryFrom<RawRules> for SubmissionRules {
    type Error = String;

    fn try_from(raw: RawRules) -> Result<Self, Self::Error> {
        let rules = SubmissionRules {
            include: raw.include.map(Patterns::new).transpose()?,
            exclude: Patterns::new(raw.exclude)?,
            unmatched: raw.unmatched,
            archives: raw.archives,
            strip: raw.strip,
            required: raw.required,
            max_size: raw.max_size,
        };
        for path in &rules.required {
            if is_junk(path.as_str()) {
                return Err(format!(
                    "required path {path:?} is OS clutter, which is always dropped"
                ));
            }
            if !rules.accepts(path.as_str()) {
                return Err(format!(
                    "required path {path:?} is left out by include/exclude"
                ));
            }
        }
        Ok(rules)
    }
}

/// Globs over relative paths, gitignore-style: a pattern without `/` (other
/// than a trailing one) matches a name at any depth, a leading `/` or an inner
/// `/` anchors it at the root, and matching a directory matches everything in it.
#[derive(Debug, Clone)]
pub struct Patterns {
    sources: Vec<String>,
    set: GlobSet,
}

impl Default for Patterns {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            set: GlobSet::empty(),
        }
    }
}

impl Patterns {
    pub fn new(sources: Vec<String>) -> Result<Self, String> {
        let mut set = GlobSetBuilder::new();
        for src in &sources {
            let unslashed = src.trim_end_matches('/');
            let anchored = unslashed.contains('/');
            let trimmed = unslashed.trim_start_matches('/');
            if trimmed.is_empty() {
                return Err(format!("empty pattern {src:?}"));
            }
            let glob = if anchored {
                trimmed.to_owned()
            } else {
                format!("**/{trimmed}")
            };
            let glob = GlobBuilder::new(&glob)
                .literal_separator(true)
                .build()
                .map_err(|e| format!("invalid pattern {src:?}: {}", e.kind()))?;
            set.add(glob);
        }
        let set = set.build().map_err(|e| format!("invalid patterns: {e}"))?;
        Ok(Self { sources, set })
    }

    /// The path, or one of the directories containing it, matches a pattern.
    pub fn matches(&self, path: &str) -> bool {
        let mut prefix = 0;
        for seg in path.split('/') {
            prefix += seg.len();
            if self.set.is_match(&path[..prefix]) {
                return true;
            }
            prefix += 1;
        }
        false
    }

    pub fn sources(&self) -> &[String] {
        &self.sources
    }
}

/// What happens to files the rules leave out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Unmatched {
    /// Left out and listed on the receipt.
    #[default]
    Drop,
    /// The submission is refused, listing them.
    Refuse,
}

/// What happens to a single uploaded archive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Archives {
    /// Its contents become the submission.
    #[default]
    Unpack,
    /// It's submitted as a file.
    Keep,
}

/// Whether a folder wrapping the whole submission is dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "serde_yaml::Value")]
pub enum Strip {
    /// Drop it when every file sits under one top-level folder.
    #[default]
    Auto,
    Off,
}

impl Serialize for Strip {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Strip::Auto => serializer.serialize_str("auto"),
            Strip::Off => serializer.serialize_bool(false),
        }
    }
}

impl TryFrom<serde_yaml::Value> for Strip {
    type Error = String;

    fn try_from(v: serde_yaml::Value) -> Result<Self, Self::Error> {
        match v {
            serde_yaml::Value::String(s) if s == "auto" => Ok(Strip::Auto),
            serde_yaml::Value::Bool(false) => Ok(Strip::Off),
            other => Err(format!("strip must be `auto` or `false`, not {other:?}")),
        }
    }
}

/// A relative file path inside a submission: no leading `/`, no empty, `.` or
/// `..` segments, no backslashes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelPath(String);

impl From<RelPath> for String {
    fn from(p: RelPath) -> Self {
        p.0
    }
}

impl RelPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RelPath {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let ok = !s.is_empty()
            && !s.contains('\\')
            && s.split('/')
                .all(|seg| !seg.is_empty() && seg != "." && seg != "..");
        if ok {
            Ok(RelPath(s))
        } else {
            Err(format!(
                "invalid path {s:?}: must be relative, without '.', '..', empty segments or backslashes"
            ))
        }
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A size written as bytes (`1048576`) or with a unit (`5MiB`, `500KB`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(try_from = "serde_yaml::Value", into = "u64")]
pub struct ByteSize(pub u64);

impl From<ByteSize> for u64 {
    fn from(b: ByteSize) -> Self {
        b.0
    }
}

impl TryFrom<serde_yaml::Value> for ByteSize {
    type Error = String;

    fn try_from(v: serde_yaml::Value) -> Result<Self, Self::Error> {
        let parsed = match &v {
            serde_yaml::Value::Number(n) => n.as_u64(),
            serde_yaml::Value::String(s) => parse_size(s),
            _ => None,
        };
        match parsed {
            Some(0) | None => Err(format!(
                "invalid size {v:?}: use a positive number of bytes or a unit like 5MiB"
            )),
            Some(n) => Ok(ByteSize(n)),
        }
    }
}

fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let mult: u64 = match unit.trim() {
        "" | "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        _ => return None,
    };
    num.parse::<u64>().ok()?.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(yaml: &str) -> Result<SubmissionRules, String> {
        serde_yaml::from_str(yaml).map_err(|e| e.to_string())
    }

    #[test]
    fn empty_block_takes_the_defaults() {
        let r = rules("{}").unwrap();
        assert!(r.include.is_none());
        assert_eq!(
            (r.unmatched, r.archives, r.strip),
            (Unmatched::Drop, Archives::Unpack, Strip::Auto)
        );
        assert!(r.accepts("anything/at/all.txt"));
    }

    #[test]
    fn patterns_without_a_slash_match_at_any_depth() {
        let r = rules("include: ['*.py']\nexclude: [__pycache__, 'tests/**']").unwrap();
        assert!(r.accepts("linked_list.py"));
        assert!(r.accepts("src/pkg/linked_list.py"));
        assert!(!r.accepts("notes.txt"));
        assert!(!r.accepts("src/__pycache__/x.py"), "excluded directory");
        assert!(!r.accepts("tests/test_mine.py"));
    }

    #[test]
    fn patterns_with_a_slash_anchor_at_the_root_and_cover_directories() {
        let r = rules("include: [/src, 'docs/*.md', lib/]").unwrap();
        assert!(r.accepts("src/main.c"));
        assert!(r.accepts("src/deep/util.c"));
        assert!(!r.accepts("other/src/main.c"), "leading slash anchors");
        assert!(r.accepts("docs/README.md"));
        assert!(!r.accepts("docs/img/a.png"));
        assert!(
            r.accepts("vendor/lib/x.c"),
            "a trailing slash alone doesn't anchor"
        );
    }

    #[test]
    fn star_doesnt_cross_directories() {
        let r = rules("include: ['src/*.c']").unwrap();
        assert!(r.accepts("src/a.c"));
        assert!(!r.accepts("src/sub/a.c"));
    }

    #[test]
    fn required_paths_must_survive_the_rules() {
        let err = rules("include: ['*.py']\nrequired: [README.md]").unwrap_err();
        assert!(
            err.contains("README.md") && err.contains("left out"),
            "{err}"
        );
        let err = rules("required: [.DS_Store]").unwrap_err();
        assert!(err.contains("clutter"), "{err}");
        assert!(rules("include: ['*.py']\nrequired: [linked_list.py]").is_ok());
    }

    #[test]
    fn required_paths_are_relative_and_clean() {
        for bad in ["/abs.py", "../up.py", "a//b.py", "./a.py", "a\\\\b.py"] {
            assert!(rules(&format!("required: ['{bad}']")).is_err(), "{bad}");
        }
        assert!(rules("required: ['My Program.java']").is_ok());
    }

    #[test]
    fn strip_is_auto_or_false() {
        assert_eq!(rules("strip: false").unwrap().strip, Strip::Off);
        assert_eq!(rules("strip: auto").unwrap().strip, Strip::Auto);
        assert!(rules("strip: true").is_err());
        assert!(rules("strip: always").is_err());
    }

    #[test]
    fn sizes_take_units() {
        let size = |s: &str| rules(&format!("max_size: {s}")).map(|r| r.max_size.unwrap().0);
        assert_eq!(size("1024").unwrap(), 1024);
        assert_eq!(size("5MiB").unwrap(), 5 << 20);
        assert_eq!(size("500KB").unwrap(), 500_000);
        assert_eq!(size("'2 GiB'").unwrap(), 2 << 30);
        assert!(size("0").is_err());
        assert!(size("5 parsecs").is_err());
        assert!(size("-1").is_err());
    }

    #[test]
    fn unknown_fields_and_bad_globs_are_errors() {
        assert!(rules("sources: [upload]").is_err());
        assert!(rules("include: ['a[']").is_err());
        assert!(rules("unmatched: ignore").is_err());
    }

    #[test]
    fn rules_round_trip_through_json() {
        let yaml = "include: ['*.py']\nexclude: [__pycache__]\nunmatched: refuse\narchives: keep\nstrip: false\nrequired: [linked_list.py]\nmax_size: 5MiB\n";
        let rules = rules(yaml).unwrap();
        let json = serde_json::to_string(&rules).unwrap();
        let back: SubmissionRules = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        assert!(back.accepts("a.py") && !back.accepts("__pycache__/a.py"));
        assert_eq!(
            (back.unmatched, back.archives, back.strip),
            (Unmatched::Refuse, Archives::Keep, Strip::Off)
        );
        assert_eq!(back.max_size, Some(ByteSize(5 << 20)));
        let defaults = serde_json::to_string(&SubmissionRules::default()).unwrap();
        assert!(
            serde_json::from_str::<SubmissionRules>(&defaults)
                .unwrap()
                .include
                .is_none()
        );
    }

    #[test]
    fn junk_is_recognized_at_any_depth() {
        assert!(is_junk("__MACOSX/src/._a.py"));
        assert!(is_junk("src/.DS_Store"));
        assert!(is_junk("._hw1"), "macOS tar's AppleDouble files");
        assert!(is_junk("hw1/src/._a.py"));
        assert!(!is_junk("src/.a.py"));
        assert!(!is_junk("src/a.py"));
    }
}
