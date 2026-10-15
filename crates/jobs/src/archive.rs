//! Assignment archives (`.tgz`): one pass that enforces the size limits and
//! path safety and hashes the file tree.
//!
//! An archive's root is the assignment directory (`assignment.yml` at the top).
//! Only regular files and directories are allowed; paths must be relative with
//! no `..`. The tree hash covers each file's path, executable bit and content,
//! so the same files dedupe however they were packed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path};

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

pub const ASSIGNMENT_FILE: &str = "assignment.yml";

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_compressed_bytes: usize,
    pub max_unpacked_bytes: u64,
    pub max_files: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ArchiveError {
    /// Over a size or count limit (HTTP 413).
    TooLarge(String),
    /// Not an acceptable archive (HTTP 400).
    Invalid(String),
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveError::TooLarge(m) | ArchiveError::Invalid(m) => f.write_str(m),
        }
    }
}

/// What intake needs from an archive.
#[derive(Debug)]
pub struct Inspected {
    pub tree_hash: String,
    pub assignment_yaml: String,
    /// Every directory in the archive (explicit or implied by a file path).
    pub dirs: BTreeSet<String>,
}

pub fn inspect(bytes: &[u8], limits: &Limits) -> Result<Inspected, ArchiveError> {
    if bytes.len() > limits.max_compressed_bytes {
        return Err(ArchiveError::TooLarge(format!(
            "archive is {} bytes; the limit is {}",
            bytes.len(),
            limits.max_compressed_bytes
        )));
    }
    let mut hashes = BTreeMap::new();
    let mut dirs = BTreeSet::new();
    let mut assignment_yaml = None;
    walk(bytes, limits, |path, entry| {
        for ancestor in Path::new(path).ancestors().skip(1) {
            let a = ancestor.to_string_lossy();
            if !a.is_empty() {
                dirs.insert(a.into_owned());
            }
        }
        match entry {
            Entry::Dir => {
                dirs.insert(path.to_owned());
            }
            Entry::File {
                executable,
                content,
            } => {
                if path == ASSIGNMENT_FILE {
                    assignment_yaml = Some(String::from_utf8(content.to_vec()).map_err(|_| {
                        ArchiveError::Invalid(format!("{ASSIGNMENT_FILE} isn't UTF-8"))
                    })?);
                }
                let digest = blake3::hash(content).to_hex().to_string();
                hashes.insert(path.to_owned(), (executable, digest));
            }
        }
        Ok(())
    })?;
    let assignment_yaml = assignment_yaml.ok_or_else(|| {
        ArchiveError::Invalid(format!(
            "no {ASSIGNMENT_FILE} at the archive's root (pack the assignment directory itself)"
        ))
    })?;
    Ok(Inspected {
        tree_hash: tree_hash(&hashes),
        assignment_yaml,
        dirs,
    })
}

/// Unpack into `dest` (which must exist), with the same checks as [`inspect`];
/// returns the tree hash of what was written. Files are 0755 if executable,
/// else 0644.
pub fn extract(bytes: &[u8], limits: &Limits, dest: &Path) -> Result<String, ArchiveError> {
    let io = |path: &str, e: std::io::Error| ArchiveError::Invalid(format!("{path}: {e}"));
    let mut hashes = BTreeMap::new();
    walk(bytes, limits, |path, entry| {
        let target = dest.join(path);
        match entry {
            Entry::Dir => std::fs::create_dir_all(&target).map_err(|e| io(path, e)),
            Entry::File {
                executable,
                content,
            } => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| io(path, e))?;
                }
                std::fs::write(&target, content).map_err(|e| io(path, e))?;
                set_mode(&target, if executable { 0o755 } else { 0o644 })
                    .map_err(|e| io(path, e))?;
                let digest = blake3::hash(content).to_hex().to_string();
                hashes.insert(path.to_owned(), (executable, digest));
                Ok(())
            }
        }
    })?;
    Ok(tree_hash(&hashes))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> std::io::Result<()> {
    Ok(())
}

fn tree_hash(files: &BTreeMap<String, (bool, String)>) -> String {
    let mut tree = blake3::Hasher::new();
    for (path, (executable, digest)) in files {
        tree.update(format!("{path}\0{}\0{digest}\n", u8::from(*executable)).as_bytes());
    }
    tree.finalize().to_hex().to_string()
}

pub enum Entry<'a> {
    Dir,
    File { executable: bool, content: &'a [u8] },
}

/// Visits every entry with a normalized relative path (no leading `./`, no
/// trailing `/`), enforcing the limits and rejecting anything unsafe.
pub fn walk(
    bytes: &[u8],
    limits: &Limits,
    mut visit: impl FnMut(&str, Entry<'_>) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    let invalid = |m: String| ArchiveError::Invalid(m);
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let entries = archive
        .entries()
        .map_err(|e| invalid(format!("not a gzipped tar archive: {e}")))?;
    let mut seen = BTreeSet::new();
    let mut unpacked: u64 = 0;
    let mut content = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(|e| invalid(format!("corrupt archive: {e}")))?;
        let raw = entry
            .path()
            .map_err(|e| invalid(format!("unreadable path in archive: {e}")))?
            .into_owned();
        let path = normalize(&raw)
            .ok_or_else(|| invalid(format!("unsafe path in archive: {}", raw.display())))?;
        if path.is_empty() {
            continue; // the root itself ("./")
        }
        if seen.len() >= limits.max_files {
            return Err(ArchiveError::TooLarge(format!(
                "archive has more than {} entries",
                limits.max_files
            )));
        }
        if !seen.insert(path.clone()) {
            return Err(invalid(format!("duplicate path in archive: {path}")));
        }
        match entry.header().entry_type() {
            EntryType::Directory => visit(&path, Entry::Dir)?,
            EntryType::Regular | EntryType::Continuous => {
                let size = entry
                    .header()
                    .size()
                    .map_err(|e| invalid(format!("{path}: bad size: {e}")))?;
                unpacked = unpacked.saturating_add(size);
                if unpacked > limits.max_unpacked_bytes {
                    return Err(ArchiveError::TooLarge(format!(
                        "archive unpacks to more than {} bytes",
                        limits.max_unpacked_bytes
                    )));
                }
                content.clear();
                entry
                    .read_to_end(&mut content)
                    .map_err(|e| invalid(format!("{path}: unreadable: {e}")))?;
                let mode = entry.header().mode().unwrap_or(0o644);
                visit(
                    &path,
                    Entry::File {
                        executable: mode & 0o111 != 0,
                        content: &content,
                    },
                )?;
            }
            other => {
                return Err(invalid(format!(
                    "{path}: only regular files and directories are allowed (found {other:?})"
                )));
            }
        }
    }
    Ok(())
}

/// A safe relative path in `/`-separated form, or `None` if it's absolute or escapes.
fn normalize(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(p) => parts.push(p.to_str()?.to_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, Header};

    pub const LIMITS: Limits = Limits {
        max_compressed_bytes: 1 << 20,
        max_unpacked_bytes: 1 << 20,
        max_files: 100,
    };

    pub enum Item<'a> {
        File(&'a str, &'a [u8], u32),
        Dir(&'a str),
        Symlink(&'a str, &'a str),
        RawPath(&'a str, &'a [u8]),
    }

    pub fn tgz(items: &[Item<'_>]) -> Vec<u8> {
        let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for item in items {
            let mut header = Header::new_gnu();
            match item {
                Item::File(path, content, mode) => {
                    header.set_size(content.len() as u64);
                    header.set_mode(*mode);
                    header.set_entry_type(EntryType::Regular);
                    builder.append_data(&mut header, path, *content).unwrap();
                }
                Item::Dir(path) => {
                    header.set_size(0);
                    header.set_mode(0o755);
                    header.set_entry_type(EntryType::Directory);
                    builder.append_data(&mut header, path, &[][..]).unwrap();
                }
                Item::Symlink(path, target) => {
                    header.set_size(0);
                    header.set_entry_type(EntryType::Symlink);
                    builder.append_link(&mut header, path, target).unwrap();
                }
                Item::RawPath(path, content) => {
                    // Bypasses Builder's path checks to forge a hostile archive.
                    header.set_size(content.len() as u64);
                    header.set_mode(0o644);
                    header.set_entry_type(EntryType::Regular);
                    let name = &mut header.as_old_mut().name;
                    name[..path.len()].copy_from_slice(path.as_bytes());
                    header.set_cksum();
                    builder.append(&header, *content).unwrap();
                }
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    const YML: &[u8] = b"id: ll\nname: Linked List\n";

    #[test]
    fn inspects_a_valid_archive() {
        let bytes = tgz(&[
            Item::File("assignment.yml", YML, 0o644),
            Item::Dir("starter"),
            Item::File("tests/test_ll.py", b"def test(): pass\n", 0o644),
            Item::File("solution/run.sh", b"#!/bin/sh\n", 0o755),
        ]);
        let got = inspect(&bytes, &LIMITS).unwrap();
        assert_eq!(got.assignment_yaml, std::str::from_utf8(YML).unwrap());
        let dirs: Vec<&str> = got.dirs.iter().map(String::as_str).collect();
        assert_eq!(dirs, ["solution", "starter", "tests"]);
        assert_eq!(got.tree_hash.len(), 64);
    }

    #[test]
    fn tree_hash_ignores_packing_but_not_content_or_exec_bit() {
        let a = tgz(&[
            Item::File("assignment.yml", YML, 0o644),
            Item::File("tests/a.py", b"x", 0o644),
        ]);
        // Same files, different order, explicit dirs, "./" prefixes, other mode bits.
        let b = tgz(&[
            Item::Dir("./tests"),
            Item::File("./tests/a.py", b"x", 0o600),
            Item::File("./assignment.yml", YML, 0o640),
        ]);
        let changed = tgz(&[
            Item::File("assignment.yml", YML, 0o644),
            Item::File("tests/a.py", b"y", 0o644),
        ]);
        let exec = tgz(&[
            Item::File("assignment.yml", YML, 0o644),
            Item::File("tests/a.py", b"x", 0o755),
        ]);
        let hash = |bytes: &[u8]| inspect(bytes, &LIMITS).unwrap().tree_hash;
        assert_eq!(hash(&a), hash(&b));
        assert_ne!(hash(&a), hash(&changed));
        assert_ne!(hash(&a), hash(&exec));
    }

    #[test]
    fn requires_assignment_yml_at_the_root() {
        let bytes = tgz(&[Item::File("ll/assignment.yml", YML, 0o644)]);
        let err = inspect(&bytes, &LIMITS).unwrap_err();
        assert!(matches!(&err, ArchiveError::Invalid(m) if m.contains("no assignment.yml")));
    }

    #[test]
    fn rejects_unsafe_entries() {
        for (items, needle) in [
            (
                vec![Item::Symlink("starter/x", "/etc/passwd")],
                "only regular files",
            ),
            (vec![Item::RawPath("../escape", b"x")], "unsafe path"),
            (vec![Item::RawPath("/abs", b"x")], "unsafe path"),
            (
                vec![
                    Item::File("assignment.yml", YML, 0o644),
                    Item::File("./assignment.yml", YML, 0o644),
                ],
                "duplicate path",
            ),
        ] {
            let err = inspect(&tgz(&items), &LIMITS).unwrap_err();
            assert!(
                matches!(&err, ArchiveError::Invalid(m) if m.contains(needle)),
                "{err}"
            );
        }
        assert!(matches!(
            inspect(b"not a tarball", &LIMITS),
            Err(ArchiveError::Invalid(_))
        ));
    }

    #[test]
    fn extract_writes_the_tree_inspect_hashed() {
        let bytes = tgz(&[
            Item::File("assignment.yml", YML, 0o644),
            Item::File("solution/run.sh", b"#!/bin/sh\n", 0o700),
            Item::Dir("starter"),
        ]);
        let dest = std::env::temp_dir().join(format!("jobs-extract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(&dest).unwrap();
        let tree = extract(&bytes, &LIMITS, &dest).unwrap();
        assert_eq!(tree, inspect(&bytes, &LIMITS).unwrap().tree_hash);
        assert_eq!(std::fs::read(dest.join("assignment.yml")).unwrap(), YML);
        assert!(dest.join("starter").is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dest.join("solution/run.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn enforces_limits() {
        let big = vec![b'a'; 2048];
        let bytes = tgz(&[
            Item::File("assignment.yml", YML, 0o644),
            Item::File("data.bin", &big, 0o644),
        ]);
        let tight = |f: fn(&mut Limits)| {
            let mut l = LIMITS;
            f(&mut l);
            inspect(&bytes, &l).unwrap_err()
        };
        assert!(matches!(
            tight(|l| l.max_compressed_bytes = 10),
            ArchiveError::TooLarge(_)
        ));
        assert!(matches!(
            tight(|l| l.max_unpacked_bytes = 1000),
            ArchiveError::TooLarge(_)
        ));
        assert!(matches!(
            tight(|l| l.max_files = 1),
            ArchiveError::TooLarge(_)
        ));
    }
}
