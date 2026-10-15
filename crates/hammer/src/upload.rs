//! `hammer upload`: pack an assignment directory and POST it to jobs.

use std::path::Path;
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Deserialize;
use tar::{Builder, EntryType, Header};
use walkdir::WalkDir;

pub const JOBS_URL_ENV: &str = "PEDAGOG_JOBS_URL";
pub const DEFAULT_JOBS_URL: &str = "http://localhost:18081";

/// VCS metadata never belongs in an assignment.
const SKIPPED_DIRS: [&str; 3] = [".git", ".hg", ".svn"];
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// jobs' reply to an accepted upload.
#[derive(Debug, Deserialize)]
pub struct Uploaded {
    pub assignment_id: String,
    pub upload_id: i64,
    pub tree_hash: String,
    pub size_bytes: i64,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub build: Build,
}

/// What jobs did about building the upload.
#[derive(Debug, Deserialize)]
pub struct Build {
    /// `dispatched` or `already_built`.
    pub status: String,
    pub build_id: i64,
}

/// A `.tgz` of `dir`'s regular files, rooted at `dir`. Deterministic: sorted
/// entries, zero timestamps and owners, and modes reduced to 0644/0755.
pub fn pack(dir: &Path) -> Result<Vec<u8>, String> {
    let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut refused = Vec::new();
    let walker = WalkDir::new(dir)
        .sort_by_file_name()
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir() && SKIPPED_DIRS.iter().any(|s| e.file_name() == *s))
        });
    for entry in walker {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        let rel = entry
            .path()
            .strip_prefix(dir)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let kind = entry.file_type();
        let mut header = Header::new_gnu();
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        if kind.is_dir() {
            header.set_entry_type(EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            builder
                .append_data(&mut header, &rel, &[][..])
                .map_err(|e| format!("cannot pack {rel}: {e}"))?;
        } else if kind.is_file() {
            let content =
                std::fs::read(entry.path()).map_err(|e| format!("cannot read {rel}: {e}"))?;
            header.set_entry_type(EntryType::Regular);
            header.set_mode(if is_executable(entry.path()) {
                0o755
            } else {
                0o644
            });
            header.set_size(content.len() as u64);
            builder
                .append_data(&mut header, &rel, content.as_slice())
                .map_err(|e| format!("cannot pack {rel}: {e}"))?;
        } else {
            refused.push(rel);
        }
    }
    if !refused.is_empty() {
        return Err(format!(
            "only regular files and directories can be uploaded; refusing: {}",
            refused.join(", ")
        ));
    }
    let gz = builder.into_inner().map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_: &Path) -> bool {
    false
}

/// POSTs the archive; jobs' refusals come back as their message.
pub fn send(jobs_url: &str, archive: Vec<u8>, force: bool) -> Result<Uploaded, String> {
    let url = format!(
        "{}/internal/assignments{}",
        jobs_url.trim_end_matches('/'),
        if force { "?force=true" } else { "" }
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(UPLOAD_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let reply = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/gzip")
        .body(archive)
        .send()
        .map_err(|e| format!("cannot reach jobs at {url}: {e} (is the port-forward running?)"))?;
    let status = reply.status();
    let body = reply.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
            .unwrap_or(body);
        return Err(format!("jobs refused the upload ({status}): {message}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("unexpected reply from jobs: {e}: {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Read;
    use std::path::PathBuf;

    use flate2::read::GzDecoder;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hammer-upload-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::create_dir_all(dir.join(".git/objects")).unwrap();
        std::fs::write(dir.join("assignment.yml"), "id: x\n").unwrap();
        std::fs::write(dir.join("tests/test_x.py"), "def test(): pass\n").unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: main\n").unwrap();
        dir
    }

    fn entries(archive: &[u8]) -> Vec<(String, u32, u64)> {
        let mut tar = tar::Archive::new(GzDecoder::new(archive));
        tar.entries()
            .unwrap()
            .map(|e| {
                let mut e = e.unwrap();
                let mut sink = Vec::new();
                e.read_to_end(&mut sink).unwrap();
                let h = e.header();
                (
                    e.path().unwrap().to_string_lossy().into_owned(),
                    h.mode().unwrap(),
                    h.mtime().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn packs_regular_files_deterministically_without_vcs_metadata() {
        let dir = scratch("pack");
        let a = pack(&dir).unwrap();
        let b = pack(&dir).unwrap();
        let listed = entries(&a);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(a, b, "same directory, same bytes");
        let paths: Vec<&str> = listed.iter().map(|(p, ..)| p.as_str()).collect();
        assert_eq!(paths, ["assignment.yml", "tests", "tests/test_x.py"]);
        assert!(listed.iter().all(|(_, _, mtime)| *mtime == 0));
        assert!(
            listed
                .iter()
                .all(|(_, mode, _)| *mode == 0o644 || *mode == 0o755)
        );
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_executable_bit_and_refuses_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("links");
        let script = dir.join("tests/run.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let modes = entries(&pack(&dir).unwrap());
        assert!(modes.contains(&("tests/run.sh".into(), 0o755, 0)));

        std::os::unix::fs::symlink("/etc/passwd", dir.join("tests/leak")).unwrap();
        let err = pack(&dir).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("refusing: tests/leak"), "{err}");
    }
}
