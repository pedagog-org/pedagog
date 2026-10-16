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

/// A grading run's status (`GET /internal/submissions/<id>`, one of its runs).
#[derive(Debug, Deserialize)]
pub struct Run {
    pub id: i64,
    pub status: String,
    pub queue_position: Option<i64>,
    pub score: Option<f64>,
    pub total: Option<f64>,
    #[serde(default)]
    pub problems: Vec<String>,
    pub error: Option<String>,
    pub results: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SubmissionStatus {
    runs: Vec<Run>,
}

const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Polls until the run is graded or errored, calling `progress` whenever its
/// status (or queue position) changes.
pub fn wait(
    jobs_url: &str,
    submission: i64,
    run: i64,
    mut progress: impl FnMut(&Run),
) -> Result<Run, String> {
    let url = format!(
        "{}/internal/submissions/{submission}",
        jobs_url.trim_end_matches('/')
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut last = String::new();
    loop {
        let reply = client
            .get(&url)
            .send()
            .map_err(|e| format!("cannot reach jobs at {url}: {e}"))?;
        if !reply.status().is_success() {
            return Err(format!("jobs answered {} for {url}", reply.status()));
        }
        let body = reply.text().map_err(|e| e.to_string())?;
        let status: SubmissionStatus = serde_json::from_str(&body)
            .map_err(|e| format!("unexpected reply from jobs: {e}: {body}"))?;
        let current = status
            .runs
            .into_iter()
            .find(|r| r.id == run)
            .ok_or_else(|| format!("submission {submission} has no run {run}"))?;
        let stage = format!("{}/{:?}", current.status, current.queue_position);
        if stage != last {
            progress(&current);
            last = stage;
        }
        if matches!(current.status.as_str(), "graded" | "error") {
            return Ok(current);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// A finished run as lines for the terminal, and whether it was graded.
pub fn outcome(run: &Run) -> (bool, Vec<String>) {
    if run.status != "graded" {
        let mut lines = vec![format!(
            "grading failed{}",
            run.error
                .as_ref()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        )];
        lines.extend(run.problems.iter().cloned());
        return (false, lines);
    }
    let mut lines = vec![format!(
        "graded: {} / {}",
        run.score.unwrap_or_default(),
        run.total.unwrap_or_default()
    )];
    let tests = run
        .results
        .as_ref()
        .and_then(|r| r.get("tests")?.as_array().cloned())
        .unwrap_or_default();
    for t in tests {
        let field = |k: &str| t.get(k).cloned().unwrap_or_default();
        if field("status").as_str() == Some("pass") {
            continue;
        }
        lines.push(format!(
            "  {} ({}): {} / {}",
            field("name").as_str().unwrap_or("?"),
            field("status").as_str().unwrap_or("?"),
            field("score"),
            field("max")
        ));
    }
    (true, lines)
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
    fn outcomes_list_tests_that_didnt_pass() {
        let run: Run = serde_json::from_value(serde_json::json!({
            "id": 3, "status": "graded", "queue_position": null, "score": 9.0, "total": 10.0,
            "results": {"tests": [
                {"name": "append", "status": "pass", "score": 2.0, "max": 2.0},
                {"name": "pop-front", "status": "fail", "score": 0.0, "max": 1.0}
            ]}
        }))
        .unwrap();
        let (graded, lines) = outcome(&run);
        assert!(graded);
        assert_eq!(lines, ["graded: 9 / 10", "  pop-front (fail): 0.0 / 1.0"]);

        let run: Run = serde_json::from_value(serde_json::json!({
            "id": 4, "status": "error", "problems": ["test_ll.py couldn't be loaded"]
        }))
        .unwrap();
        let (graded, lines) = outcome(&run);
        assert!(!graded);
        assert_eq!(lines, ["grading failed", "test_ll.py couldn't be loaded"]);
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
