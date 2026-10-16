//! `hammer submit`: send files (or one archive) to jobs as a student's submission.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::blocking::multipart::{Form, Part};
use serde::Deserialize;
use walkdir::WalkDir;

const SKIPPED_DIRS: [&str; 3] = [".git", ".hg", ".svn"];
const SUBMIT_TIMEOUT: Duration = Duration::from_secs(120);

/// jobs' reply to an accepted submission: ids and the receipt.
#[derive(Debug, Deserialize)]
pub struct Submitted {
    pub submission_id: i64,
    pub run_id: i64,
    pub build_id: i64,
    pub queue_position: Option<i64>,
    pub files: Vec<String>,
    #[serde(default)]
    pub ignored: Vec<String>,
    #[serde(default)]
    pub junk: usize,
    pub unpacked: Option<String>,
    pub stripped: Option<String>,
    pub size_bytes: u64,
}

/// The files to send, by the name jobs sees: paths relative to a directory
/// argument, or the file name of a file argument. jobs applies the
/// assignment's rules; this only refuses what can't be sent.
pub fn collect(args: &[PathBuf]) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut files = BTreeMap::new();
    let mut add = |name: String, path: PathBuf| match files.insert(name.clone(), path) {
        Some(_) => Err(format!("{name} is given twice")),
        None => Ok(()),
    };
    for arg in args {
        let meta =
            std::fs::metadata(arg).map_err(|e| format!("cannot read {}: {e}", arg.display()))?;
        if meta.is_file() {
            let name = arg
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .ok_or_else(|| format!("{} has no file name", arg.display()))?;
            add(name, arg.clone())?;
        } else if meta.is_dir() {
            for (rel, path) in walk(arg)? {
                add(rel, path)?;
            }
        } else {
            return Err(format!(
                "{} isn't a regular file or directory",
                arg.display()
            ));
        }
    }
    if files.is_empty() {
        return Err("nothing to submit".to_owned());
    }
    Ok(files)
}

fn walk(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let walker = WalkDir::new(dir)
        .sort_by_file_name()
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir() && SKIPPED_DIRS.iter().any(|s| e.file_name() == *s))
        });
    let mut out = Vec::new();
    let mut refused = Vec::new();
    for entry in walker {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        let rel = entry
            .path()
            .strip_prefix(dir)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if entry.file_type().is_file() {
            out.push((rel, entry.into_path()));
        } else if !entry.file_type().is_dir() {
            refused.push(rel);
        }
    }
    if !refused.is_empty() {
        return Err(format!(
            "only regular files can be submitted; refusing: {}",
            refused.join(", ")
        ));
    }
    Ok(out)
}

/// POSTs the files; jobs' refusals come back as their message.
pub fn send(
    jobs_url: &str,
    assignment: &str,
    student: &str,
    files: &BTreeMap<String, PathBuf>,
) -> Result<Submitted, String> {
    let url = reqwest::Url::parse_with_params(
        &format!(
            "{}/internal/assignments/{assignment}/submissions",
            jobs_url.trim_end_matches('/')
        ),
        &[("student", student)],
    )
    .map_err(|e| format!("invalid jobs URL {jobs_url:?}: {e}"))?;
    let mut form = Form::new();
    for (name, path) in files {
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        form = form.part("file", Part::bytes(bytes).file_name(name.clone()));
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(SUBMIT_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let reply = client
        .post(url.clone())
        .multipart(form)
        .send()
        .map_err(|e| format!("cannot reach jobs at {url}: {e} (is the port-forward running?)"))?;
    let status = reply.status();
    let body = reply.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
            .unwrap_or(body);
        return Err(format!("jobs refused the submission ({status}): {message}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("unexpected reply from jobs: {e}: {body}"))
}

/// The receipt as lines for the terminal.
pub fn describe(s: &Submitted) -> Vec<String> {
    let mut lines = vec![format!(
        "submission {} accepted ({} file(s), {} bytes); grading run {} against build {}{}",
        s.submission_id,
        s.files.len(),
        s.size_bytes,
        s.run_id,
        s.build_id,
        s.queue_position
            .map(|p| format!(", queue position {p}"))
            .unwrap_or_default()
    )];
    if let Some(archive) = &s.unpacked {
        lines.push(format!("unpacked {archive}"));
    }
    if let Some(folder) = &s.stripped {
        lines.push(format!("dropped the wrapping folder {folder}/"));
    }
    lines.push(format!("files: {}", s.files.join(", ")));
    if !s.ignored.is_empty() {
        lines.push(format!(
            "ignored by the assignment's rules: {}",
            s.ignored.join(", ")
        ));
    }
    if s.junk > 0 {
        lines.push(format!(
            "dropped {} OS file(s) (.DS_Store, __MACOSX, ...)",
            s.junk
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hammer-submit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn directories_send_relative_paths_and_files_their_names() {
        let dir = scratch("collect");
        std::fs::create_dir_all(dir.join("work/src")).unwrap();
        std::fs::create_dir_all(dir.join("work/.git")).unwrap();
        std::fs::write(dir.join("work/main.py"), "x").unwrap();
        std::fs::write(dir.join("work/src/util.py"), "y").unwrap();
        std::fs::write(dir.join("work/.git/HEAD"), "z").unwrap();
        std::fs::write(dir.join("README.md"), "r").unwrap();

        let files = collect(&[dir.join("work"), dir.join("README.md")]).unwrap();
        let names: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(names, ["README.md", "main.py", "src/util.py"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_name_twice_is_refused() {
        let dir = scratch("twice");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(dir.join("a/main.py"), "x").unwrap();
        std::fs::write(dir.join("main.py"), "y").unwrap();
        let err = collect(&[dir.join("a"), dir.join("main.py")]).unwrap_err();
        assert!(err.contains("main.py is given twice"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_receipt_mentions_what_was_dropped() {
        let s = Submitted {
            submission_id: 4,
            run_id: 9,
            build_id: 2,
            queue_position: Some(3),
            files: vec!["linked_list.py".into()],
            ignored: vec!["notes.txt".into()],
            junk: 1,
            unpacked: Some("hw1.zip".into()),
            stripped: Some("hw1".into()),
            size_bytes: 120,
        };
        let text = describe(&s).join("\n");
        assert!(text.contains("queue position 3"), "{text}");
        assert!(text.contains("dropped the wrapping folder hw1/"), "{text}");
        assert!(
            text.contains("ignored by the assignment's rules: notes.txt"),
            "{text}"
        );
        assert!(text.contains("dropped 1 OS file(s)"), "{text}");
    }
}
