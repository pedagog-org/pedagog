//! `hammer upload`: pack an assignment directory and POST it to a course,
//! through the api (as you) or straight to jobs (operators).

use std::path::Path;
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use pedagog_core::changes::Notify;
use serde::Deserialize;
use tar::{Builder, EntryType, Header};
use walkdir::WalkDir;

/// Where uploads and build statuses go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// The api, with the session's token.
    Api { base: String, token: String },
    /// jobs' internal endpoints (no login; operators and e2e).
    Jobs(String),
}

impl Endpoint {
    /// `path` (e.g. `/courses/c/assignments`) on this endpoint.
    pub fn url(&self, path: &str) -> String {
        match self {
            Endpoint::Api { base, .. } => format!("{}{path}", base.trim_end_matches('/')),
            Endpoint::Jobs(base) => format!("{}/internal{path}", base.trim_end_matches('/')),
        }
    }

    fn authorized(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        match self {
            Endpoint::Api { token, .. } => req.bearer_auth(token),
            Endpoint::Jobs(_) => req,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Endpoint::Api { .. } => "the api",
            Endpoint::Jobs(_) => "jobs",
        }
    }
}

/// VCS metadata never belongs in an assignment.
const SKIPPED_DIRS: [&str; 3] = [".git", ".hg", ".svn"];
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// jobs' reply to an accepted upload.
#[derive(Debug, Deserialize)]
pub struct Uploaded {
    pub course: String,
    /// The assignment's slug.
    pub assignment: String,
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

/// An attempt's progress and outcome (`GET .../builds/<id>`).
#[derive(Debug, Deserialize)]
pub struct Status {
    pub build_status: String,
    pub validation_status: String,
    pub finished: bool,
    pub image_ref: String,
    pub error: Option<String>,
    #[serde(default)]
    pub validation_problems: Vec<String>,
    pub results: Option<serde_json::Value>,
}

const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Polls until the attempt finishes, calling `progress` whenever its stage changes.
pub fn wait(
    to: &Endpoint,
    course: &str,
    assignment: &str,
    build: i64,
    mut progress: impl FnMut(&Status),
) -> Result<Status, String> {
    let url = to.url(&format!(
        "/courses/{course}/assignments/{assignment}/builds/{build}"
    ));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut last = String::new();
    loop {
        let reply = to
            .authorized(client.get(&url))
            .send()
            .map_err(|e| format!("cannot reach {} at {url}: {e}", to.name()))?;
        if !reply.status().is_success() {
            return Err(format!(
                "{} answered {} for {url}",
                to.name(),
                reply.status()
            ));
        }
        let body = reply.text().map_err(|e| e.to_string())?;
        let status: Status = serde_json::from_str(&body)
            .map_err(|e| format!("unexpected reply from {}: {e}: {body}", to.name()))?;
        let stage = format!("{}/{}", status.build_status, status.validation_status);
        if stage != last {
            progress(&status);
            last = stage;
        }
        if status.finished {
            return Ok(status);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One line for a finished attempt, and whether it succeeded.
pub fn outcome(status: &Status) -> (bool, String) {
    let summary = status
        .results
        .as_ref()
        .and_then(|r| {
            let s = r.get("summary")?;
            let tests = r.get("tests")?.as_array()?.len();
            Some(format!(
                "{} / {} ({tests} tests)",
                s.get("score")?,
                s.get("total")?
            ))
        })
        .unwrap_or_default();
    match (
        status.build_status.as_str(),
        status.validation_status.as_str(),
    ) {
        ("superseded", _) => (false, "superseded by a newer upload".into()),
        ("failed", _) => (false, "build failed".into()),
        (_, "passed") => (true, format!("validation passed: {summary}")),
        (_, "skipped") => (
            true,
            "built; validation skipped (no reference solution)".into(),
        ),
        (_, "failed") => (false, format!("validation failed: {summary}")),
        (_, "invalid") => (false, "validation produced no trustworthy results".into()),
        (b, v) => (false, format!("finished as {b}/{v}")),
    }
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

/// POSTs the archive to `course`; refusals come back as their message.
/// `regrade` asks for a regrade, telling students per its `Notify`.
pub fn send(
    to: &Endpoint,
    course: &str,
    archive: Vec<u8>,
    force: bool,
    regrade: Option<Notify>,
) -> Result<Uploaded, String> {
    let mut params = Vec::new();
    if force {
        params.push(("force", "true"));
    }
    let notify = regrade.map(|n| n.to_string());
    if let Some(notify) = &notify {
        params.push(("regrade", notify.as_str()));
    }
    let base = to.url(&format!("/courses/{course}/assignments"));
    let url = reqwest::Url::parse_with_params(&base, &params)
        .map_err(|e| format!("invalid URL {base:?}: {e}"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(UPLOAD_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let reply = to
        .authorized(client.post(url.clone()))
        .header(reqwest::header::CONTENT_TYPE, "application/gzip")
        .body(archive)
        .send()
        .map_err(|e| {
            format!(
                "cannot reach {} at {url}: {e} (is the port-forward running?)",
                to.name()
            )
        })?;
    let status = reply.status();
    let body = reply.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
            .unwrap_or(body);
        return Err(format!(
            "{} refused the upload ({status}): {message}",
            to.name()
        ));
    }
    serde_json::from_str(&body)
        .map_err(|e| format!("unexpected reply from {}: {e}: {body}", to.name()))
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

    fn status(build: &str, validation: &str, results: Option<serde_json::Value>) -> Status {
        Status {
            build_status: build.into(),
            validation_status: validation.into(),
            finished: true,
            image_ref: "reg/x:1".into(),
            error: None,
            validation_problems: vec![],
            results,
        }
    }

    #[test]
    fn outcomes_read_like_a_verdict() {
        let results =
            serde_json::json!({"summary": {"score": 10.0, "total": 10.0}, "tests": [{}, {}]});
        assert_eq!(
            outcome(&status("succeeded", "passed", Some(results))),
            (true, "validation passed: 10.0 / 10.0 (2 tests)".to_string())
        );
        assert!(outcome(&status("succeeded", "skipped", None)).0);
        assert!(!outcome(&status("succeeded", "invalid", None)).0);
        assert!(!outcome(&status("failed", "pending", None)).0);
        assert!(!outcome(&status("superseded", "running", None)).0);
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
