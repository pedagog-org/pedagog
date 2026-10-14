use std::fmt;

use crate::recipe::platform::PlatformKind;
use crate::recipe::primitives::{OsId, ToolchainId, ToolchainRef, Version};
use crate::recipe::render::ToolchainOrigin;

#[derive(Debug)]
pub enum ResolveError {
    OsNotFound(OsId),
    PlatformNotFound { kind: PlatformKind, os: OsId },
    ToolchainNotFound { tc: ToolchainRef, os: OsId, origin: ToolchainOrigin },
    AddonNotDeclared { parent: ToolchainRef, addon: ToolchainRef, declared: Vec<Version> },
    VersionConflict { id: ToolchainId, entries: Vec<ConflictEntry> },
    InvalidParams(String),
    Interpolation { context: String, message: String },
    InvalidPort { raw: String },
}

/// One version of a toolchain that was selected alongside other versions of it.
#[derive(Debug)]
pub struct ConflictEntry {
    pub version: Version,
    pub origins: Vec<ToolchainOrigin>,
    /// For each parent that chose this version as an addon: the versions it declares.
    pub allowed_by_parent: Vec<(ToolchainRef, Vec<Version>)>,
    /// The version's recipe declares it can install side by side.
    pub coexist: bool,
    /// Every assignment entry for this version is marked `!` / `allow_coexist`.
    pub allow_coexist: bool,
}

impl ConflictEntry {
    fn resolved(&self) -> bool {
        self.coexist || self.allow_coexist
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::OsNotFound(os) => write!(f, "os {:?} not found", os.as_str()),
            ResolveError::PlatformNotFound { kind, os } => write!(
                f,
                "no {:?} platform recipe found for os {:?}",
                kind.as_str(),
                os.as_str()
            ),
            ResolveError::ToolchainNotFound { tc, os, origin } => {
                write!(f, "toolchain {tc} ")?;
                if let ToolchainOrigin::AddonOf(_) = origin {
                    write!(f, "({origin}) ")?;
                }
                write!(f, "not found for os {:?}", os.as_str())
            }
            ResolveError::AddonNotDeclared { parent, addon, declared } => {
                write!(f, "{parent} does not declare {addon} as a compatible addon")?;
                if declared.is_empty() {
                    write!(f, " ({parent} offers no {} addon)", addon.id)
                } else {
                    write!(f, " (declared {} versions: {})", addon.id, versions(declared))
                }
            }
            ResolveError::VersionConflict { id, entries } => write_conflict(f, id, entries),
            ResolveError::InvalidParams(msg) => write!(f, "invalid platform params: {msg}"),
            ResolveError::Interpolation { context, message } => {
                write!(f, "interpolation error in {context}: {message}")
            }
            ResolveError::InvalidPort { raw } => {
                write!(f, "invalid port {raw:?}: expected a number from 1 to 65535")
            }
        }
    }
}

impl std::error::Error for ResolveError {}

impl ResolveError {
    /// A suggested fix, kept out of `Display` so callers can present it separately.
    pub fn help(&self) -> Option<String> {
        match self {
            ResolveError::VersionConflict { id, entries } => Some(conflict_help(id, entries)),
            _ => None,
        }
    }
}

fn write_conflict(f: &mut fmt::Formatter<'_>, id: &ToolchainId, entries: &[ConflictEntry]) -> fmt::Result {
    write!(
        f,
        "toolchain `{id}` is selected at {} versions, and not all of them allow coexisting:",
        entries.len()
    )?;
    let labels: Vec<String> = entries.iter().map(|e| format!("{id}:{}", e.version)).collect();
    let width = labels.iter().map(String::len).max().unwrap_or(0);
    for (entry, label) in entries.iter().zip(&labels) {
        let via: Vec<String> = entry
            .origins
            .iter()
            .map(|origin| match origin {
                ToolchainOrigin::AddonOf(parent) => {
                    let allowed = entry
                        .allowed_by_parent
                        .iter()
                        .find(|(p, _)| p == parent)
                        .map(|(_, v)| versions(v))
                        .unwrap_or_default();
                    format!("{origin} (allows {allowed})")
                }
                ToolchainOrigin::TopLevel => origin.to_string(),
            })
            .collect();
        let yes_no = |b: bool| if b { "yes" } else { "no" };
        write!(
            f,
            "\n  {label:<width$}  {}  coexist: recipe={} assignment={}",
            via.join("; "),
            yes_no(entry.coexist),
            yes_no(entry.allow_coexist)
        )?;
    }
    match common_versions(entries) {
        Some(common) if common.is_empty() => write!(f, "\n  versions allowed by every parent: none"),
        Some(common) => write!(f, "\n  versions allowed by every parent: {}", versions(&common)),
        None => Ok(()),
    }
}

fn conflict_help(id: &ToolchainId, entries: &[ConflictEntry]) -> String {
    let unresolved: Vec<String> = entries
        .iter()
        .filter(|e| !e.resolved())
        .map(|e| format!("{id}:{}", e.version))
        .collect();
    let marked: Vec<String> = unresolved.iter().map(|l| format!("{l}!")).collect();
    let coexist_fix = format!(
        "set `coexist: true` in the {} recipe(s), or allow it in the assignment by marking every such entry: {}",
        unresolved.join(", "),
        marked.join(", ")
    );
    match common_versions(entries) {
        None => format!("pick one {id} version, or {coexist_fix}"),
        Some(common) => match common.as_slice() {
            [] => format!("no single {id} version fits every parent; change a parent toolchain, or {coexist_fix}"),
            [only] => format!("use {id}:{only} everywhere, or {coexist_fix}"),
            many => format!("use one of {id} {} everywhere, or {coexist_fix}", versions(many)),
        },
    }
}

/// Versions every addon parent allows; `None` when no entry came from a parent.
fn common_versions(entries: &[ConflictEntry]) -> Option<Vec<Version>> {
    let mut sets = entries.iter().flat_map(|e| e.allowed_by_parent.iter().map(|(_, v)| v));
    let first = sets.next()?.clone();
    Some(sets.fold(first, |acc, set| acc.into_iter().filter(|v| set.contains(v)).collect()))
}

fn versions(vs: &[Version]) -> String {
    vs.iter().map(Version::as_str).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::primitives::Id;

    fn tc(s: &str) -> ToolchainRef {
        ToolchainRef::try_from(s.to_owned()).unwrap()
    }

    fn vs(list: &[&str]) -> Vec<Version> {
        list.iter().map(|v| Version::try_from((*v).to_owned()).unwrap()).collect()
    }

    fn os() -> OsId {
        Id::try_from("ubuntu-22".to_owned()).unwrap()
    }

    fn addon_entry(version: &str, parent: &str, allowed: &[&str], coexist: bool) -> ConflictEntry {
        ConflictEntry {
            version: vs(&[version])[0].clone(),
            origins: vec![ToolchainOrigin::AddonOf(tc(parent))],
            allowed_by_parent: vec![(tc(parent), vs(allowed))],
            coexist,
            allow_coexist: false,
        }
    }

    #[test]
    fn toolchain_not_found_names_ref_and_os() {
        let err = ResolveError::ToolchainNotFound {
            tc: tc("gcc:13"),
            os: os(),
            origin: ToolchainOrigin::TopLevel,
        };
        assert_eq!(err.to_string(), r#"toolchain gcc:13 not found for os "ubuntu-22""#);
    }

    #[test]
    fn addon_not_found_names_parent() {
        let err = ResolveError::ToolchainNotFound {
            tc: tc("gdb:14"),
            os: os(),
            origin: ToolchainOrigin::AddonOf(tc("gcc:13")),
        };
        assert_eq!(
            err.to_string(),
            r#"toolchain gdb:14 (addon of gcc:13) not found for os "ubuntu-22""#
        );
    }

    #[test]
    fn addon_not_declared_lists_declared_versions() {
        let err = ResolveError::AddonNotDeclared {
            parent: tc("gcc:13"),
            addon: tc("gdb:16"),
            declared: vs(&["14", "15"]),
        };
        assert_eq!(
            err.to_string(),
            "gcc:13 does not declare gdb:16 as a compatible addon (declared gdb versions: 14, 15)"
        );
    }

    #[test]
    fn conflict_without_overlap() {
        let err = ResolveError::VersionConflict {
            id: Id::try_from("gdb".to_owned()).unwrap(),
            entries: vec![
                addon_entry("15", "gcc:13", &["14", "15"], false),
                addon_entry("13", "clang:17", &["12", "13"], true),
            ],
        };
        assert_eq!(
            err.to_string(),
            "toolchain `gdb` is selected at 2 versions, and not all of them allow coexisting:
  gdb:15  addon of gcc:13 (allows 14, 15)  coexist: recipe=no assignment=no
  gdb:13  addon of clang:17 (allows 12, 13)  coexist: recipe=yes assignment=no
  versions allowed by every parent: none"
        );
        assert_eq!(
            err.help().unwrap(),
            "no single gdb version fits every parent; change a parent toolchain, or set `coexist: true` in the gdb:15 recipe(s), or allow it in the assignment by marking every such entry: gdb:15!"
        );
    }

    #[test]
    fn conflict_with_overlap_suggests_common_version() {
        let err = ResolveError::VersionConflict {
            id: Id::try_from("gdb".to_owned()).unwrap(),
            entries: vec![
                addon_entry("15", "gcc:13", &["14", "15"], false),
                addon_entry("13", "clang:17", &["13", "14"], false),
            ],
        };
        let text = err.to_string();
        assert!(text.ends_with("versions allowed by every parent: 14"), "{text}");
        assert_eq!(
            err.help().unwrap(),
            "use gdb:14 everywhere, or set `coexist: true` in the gdb:15, gdb:13 recipe(s), or allow it in the assignment by marking every such entry: gdb:15!, gdb:13!"
        );
    }

    #[test]
    fn conflict_between_direct_listings_has_no_overlap_line() {
        let entry = |v: &str| ConflictEntry {
            version: vs(&[v])[0].clone(),
            origins: vec![ToolchainOrigin::TopLevel],
            allowed_by_parent: vec![],
            coexist: false,
            allow_coexist: false,
        };
        let err = ResolveError::VersionConflict {
            id: Id::try_from("python".to_owned()).unwrap(),
            entries: vec![entry("3.11"), entry("3.12")],
        };
        let text = err.to_string();
        assert!(text.contains("  python:3.11  listed directly  coexist: recipe=no assignment=no\n"), "{text}");
        assert!(!text.contains("every parent"), "{text}");
        assert_eq!(
            err.help().unwrap(),
            "pick one python version, or set `coexist: true` in the python:3.11, python:3.12 recipe(s), or allow it in the assignment by marking every such entry: python:3.11!, python:3.12!"
        );
    }
}
