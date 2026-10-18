//! `POST /internal/assignments/{id}/submissions?student=<id>`: accept a
//! submission (several files, or one archive), normalize it with the
//! assignment's `submission:` rules, store it, and queue its grading run
//! against the current build.
//!
//! `GET /internal/submissions/{id}` and
//! `GET /internal/assignments/{id}/submissions?student=<id>` report them.
//!
//! Normalization, in order: unpack a single archive (unless `archives: keep`),
//! drop OS clutter, strip one wrapping folder (`strip: auto`), apply
//! include/exclude, check required paths, check the size. Each step is
//! reported on the receipt.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::Path;

use axum::extract::{Multipart, Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use pedagog_core::recipe::platform::PlatformKind;
use pedagog_core::recipe::submission::{Archives, Strip, SubmissionRules, Unmatched, is_junk};
use pedagog_store::QueuedRun;
use pedagog_store::SubmissionSource;
use pedagog_store::db::{assignments as builds_db, queue, submissions as db};
use pedagog_store::objects::submission_key;
use serde::{Deserialize, Serialize};

use crate::archive::{self, ArchiveError, Entry, Limits, TreeFile};
use crate::http::error::{Refused, internal, refused};
use crate::settings::{RunSettings, settings_of};
use crate::state::AppState;

/// Size and count limits for one submission (jobs' ceiling; an assignment's
/// `max_size` can only lower the size).
#[derive(Clone, Copy, Debug)]
pub struct SubmissionLimits {
    pub max_bytes: u64,
    pub max_files: usize,
}

/// One uploaded part: a relative path and its bytes.
#[derive(Debug)]
pub struct Part {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// What intake did with a submission (stored, and returned to the submitter).
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    /// The files submitted, after normalization.
    pub files: Vec<String>,
    /// Files the rules left out.
    pub ignored: Vec<String>,
    /// OS clutter dropped (`__MACOSX/`, `.DS_Store`, ...).
    pub junk: usize,
    /// The archive that was unpacked, if any.
    pub unpacked: Option<String>,
    /// The wrapping folder that was dropped, if any.
    pub stripped: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug)]
pub struct Normalized {
    pub tree: BTreeMap<String, TreeFile>,
    pub receipt: Receipt,
}

const ARCHIVE_SUFFIXES: [&str; 3] = [".zip", ".tar.gz", ".tgz"];

pub fn normalize(
    parts: Vec<Part>,
    rules: &SubmissionRules,
    limits: &SubmissionLimits,
) -> Result<Normalized, Refused> {
    let bad = |m: String| refused(StatusCode::BAD_REQUEST, m);
    let unprocessable = |m: String| refused(StatusCode::UNPROCESSABLE_ENTITY, m);
    let too_large = |m: String| refused(StatusCode::PAYLOAD_TOO_LARGE, m);
    let mut receipt = Receipt::default();

    let single_archive = match parts.as_slice() {
        [only] if rules.archives == Archives::Unpack => {
            let lower = only.name.to_ascii_lowercase();
            ARCHIVE_SUFFIXES
                .iter()
                .find(|s| lower.ends_with(*s))
                .copied()
        }
        _ => None,
    };
    let mut tree = BTreeMap::new();
    if let Some(suffix) = single_archive {
        let Some(part) = parts.into_iter().next() else {
            return Err(bad("empty submission".to_owned()));
        };
        receipt.unpacked = Some(part.name.clone());
        tree = if suffix == ".zip" {
            unzip(&part.bytes, limits)?
        } else {
            untar(&part.bytes, limits)?
        };
    } else {
        if parts.len() > limits.max_files {
            return Err(too_large(format!(
                "more than {} files submitted",
                limits.max_files
            )));
        }
        for part in parts {
            let path = archive::normalize(Path::new(&part.name))
                .filter(|p| !p.is_empty())
                .ok_or_else(|| bad(format!("unsafe file name: {:?}", part.name)))?;
            let file = TreeFile {
                executable: false,
                content: part.bytes,
            };
            if tree.insert(path.clone(), file).is_some() {
                return Err(bad(format!("{path} was submitted twice")));
            }
        }
    }

    let before = tree.len();
    tree.retain(|path, _| !is_junk(path));
    receipt.junk = before - tree.len();

    // Not when the required paths are already where they belong (`src/main.c`).
    let satisfied_as_is =
        !rules.required.is_empty() && rules.required.iter().all(|r| tree.contains_key(r.as_str()));
    if rules.strip == Strip::Auto
        && !satisfied_as_is
        && let Some(folder) = wrapping_folder(&tree)
    {
        let prefix = format!("{folder}/");
        tree = tree
            .into_iter()
            .map(|(p, f)| (p[prefix.len()..].to_owned(), f))
            .collect();
        receipt.stripped = Some(folder);
    }

    let (kept, ignored): (BTreeMap<_, _>, BTreeMap<_, _>) =
        tree.into_iter().partition(|(p, _)| rules.accepts(p));
    receipt.ignored = ignored.into_keys().collect();
    if rules.unmatched == Unmatched::Refuse && !receipt.ignored.is_empty() {
        return Err(unprocessable(format!(
            "not accepted by this assignment: {}",
            receipt.ignored.join(", ")
        )));
    }
    if kept.is_empty() {
        return Err(unprocessable(if receipt.ignored.is_empty() {
            "nothing to submit".to_owned()
        } else {
            format!(
                "nothing to submit: every file was left out by the rules ({})",
                receipt.ignored.join(", ")
            )
        }));
    }
    let missing: Vec<&str> = rules
        .required
        .iter()
        .map(|r| r.as_str())
        .filter(|r| !kept.contains_key(*r))
        .collect();
    if !missing.is_empty() {
        return Err(unprocessable(format!(
            "missing required files: {}",
            missing.join(", ")
        )));
    }
    receipt.size_bytes = kept.values().map(|f| f.content.len() as u64).sum();
    let max = rules
        .max_size
        .map_or(limits.max_bytes, |m| m.0.min(limits.max_bytes));
    if receipt.size_bytes > max {
        return Err(too_large(format!(
            "submission is {} bytes; the limit is {max}",
            receipt.size_bytes
        )));
    }
    receipt.files = kept.keys().cloned().collect();
    Ok(Normalized {
        tree: kept,
        receipt,
    })
}

/// The single top-level folder every file sits in, if there is one.
fn wrapping_folder(tree: &BTreeMap<String, TreeFile>) -> Option<String> {
    let mut folder: Option<&str> = None;
    for path in tree.keys() {
        let (first, _) = path.split_once('/')?;
        match folder {
            None => folder = Some(first),
            Some(f) if f == first => {}
            Some(_) => return None,
        }
    }
    folder.map(str::to_owned)
}

fn untar(bytes: &[u8], limits: &SubmissionLimits) -> Result<BTreeMap<String, TreeFile>, Refused> {
    let walk_limits = Limits {
        max_compressed_bytes: usize::MAX,
        max_unpacked_bytes: limits.max_bytes,
        max_files: limits.max_files,
    };
    let mut tree = BTreeMap::new();
    archive::walk(bytes, &walk_limits, |path, entry| {
        if let Entry::File {
            executable,
            content,
        } = entry
        {
            tree.insert(
                path.to_owned(),
                TreeFile {
                    executable,
                    content: content.to_vec(),
                },
            );
        }
        Ok(())
    })
    .map_err(archive_refused)?;
    Ok(tree)
}

fn unzip(bytes: &[u8], limits: &SubmissionLimits) -> Result<BTreeMap<String, TreeFile>, Refused> {
    let bad = |m: String| refused(StatusCode::BAD_REQUEST, m);
    let too_large = |m: String| refused(StatusCode::PAYLOAD_TOO_LARGE, m);
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| bad(format!("not a zip archive: {e}")))?;
    if zip.len() > limits.max_files {
        return Err(too_large(format!(
            "archive has more than {} entries",
            limits.max_files
        )));
    }
    let mut tree = BTreeMap::new();
    let mut unpacked: u64 = 0;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| bad(format!("unreadable zip entry: {e}")))?;
        let name = entry
            .name()
            .map_err(|e| bad(format!("unreadable zip entry name: {e}")))?
            .into_owned();
        if entry.is_symlink() {
            return Err(bad(format!(
                "{name}: only regular files and directories are allowed (found a symlink)"
            )));
        }
        if entry.is_dir() {
            continue;
        }
        let path = archive::normalize(Path::new(&name))
            .filter(|p| !p.is_empty() && !name.contains('\\'))
            .ok_or_else(|| bad(format!("unsafe path in archive: {name}")))?;
        let remaining = limits.max_bytes.saturating_sub(unpacked);
        let mut content = Vec::new();
        (&mut entry)
            .take(remaining.saturating_add(1))
            .read_to_end(&mut content)
            .map_err(|e| bad(format!("{name}: unreadable: {e}")))?;
        unpacked = unpacked.saturating_add(content.len() as u64);
        if unpacked > limits.max_bytes {
            return Err(too_large(format!(
                "archive unpacks to more than {} bytes",
                limits.max_bytes
            )));
        }
        let executable = entry.unix_mode().is_some_and(|m| m & 0o111 != 0);
        if tree
            .insert(
                path.clone(),
                TreeFile {
                    executable,
                    content,
                },
            )
            .is_some()
        {
            return Err(bad(format!("duplicate path in archive: {path}")));
        }
    }
    Ok(tree)
}

fn archive_refused(e: ArchiveError) -> Refused {
    match e {
        ArchiveError::TooLarge(m) => refused(StatusCode::PAYLOAD_TOO_LARGE, m),
        ArchiveError::Invalid(m) => refused(StatusCode::BAD_REQUEST, m),
    }
}

#[derive(Deserialize)]
pub struct SubmitQuery {
    student: String,
}

#[derive(Debug, Serialize)]
pub struct Submitted {
    pub submission_id: i64,
    pub run_id: i64,
    pub build_id: i64,
    pub tree_hash: String,
    /// 1-based position in the grading queue.
    pub queue_position: Option<i64>,
    #[serde(flatten)]
    pub receipt: Receipt,
}

const MAX_STUDENT_ID: usize = 128;

pub async fn submit(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<SubmitQuery>,
    multipart: Multipart,
) -> Result<Response, Refused> {
    let student = query.student.trim().to_owned();
    if student.is_empty() || student.len() > MAX_STUDENT_ID || student.chars().any(char::is_control)
    {
        return Err(refused(
            StatusCode::BAD_REQUEST,
            format!("student must be 1–{MAX_STUDENT_ID} printable characters"),
        ));
    }
    let (build_id, settings) = current_settings(&state, &id).await?;
    if settings.platform != PlatformKind::Upload {
        return Err(refused(
            StatusCode::CONFLICT,
            format!(
                "{id} is a {} assignment: its submissions come from the session workspace, which arrives with sessions",
                settings.platform
            ),
        ));
    }
    let parts = read_parts(multipart).await?;

    let limits = state.config.submission_limits;
    let rules = settings.submission;
    let (normalized, packed) = tokio::task::spawn_blocking(move || {
        let normalized = normalize(parts, &rules, &limits)?;
        let packed =
            archive::pack(&normalized.tree).map_err(|e| internal("packing the submission", e))?;
        Ok::<_, Refused>((normalized, packed))
    })
    .await
    .map_err(|e| internal("normalizing the submission", e))??;
    let (bytes, tree_hash) = packed;

    let key = submission_key(&id, &tree_hash);
    let size = i64::try_from(normalized.receipt.size_bytes).unwrap_or(i64::MAX);
    state
        .objects
        .put(&key, bytes)
        .await
        .map_err(|e| internal("storing the submission", e))?;
    let receipt_json = serde_json::to_string(&normalized.receipt)
        .map_err(|e| internal("recording the receipt", e))?;
    let accepted = db::record_submission(
        &state.pool,
        &db::NewSubmission {
            assignment_id: &id,
            student_id: &student,
            build_id,
            source: SubmissionSource::Upload,
            tree_hash: &tree_hash,
            object_key: &key,
            size_bytes: size,
            receipt: &receipt_json,
        },
    )
    .await
    .map_err(|e| internal("recording the submission", e))?;
    let queue_position = queue::position(&state.pool, QueuedRun::Grading(accepted.run_id))
        .await
        .map_err(|e| internal("reading the queue", e))?;
    tracing::info!(
        assignment = %id, student = %student, submission = accepted.submission_id,
        run = accepted.run_id, build = build_id, files = normalized.receipt.files.len(),
        "submission accepted"
    );
    let reply = Submitted {
        submission_id: accepted.submission_id,
        run_id: accepted.run_id,
        build_id,
        tree_hash,
        queue_position,
        receipt: normalized.receipt,
    };
    Ok((StatusCode::CREATED, Json(reply)).into_response())
}

/// A grading run as the status endpoints report it.
#[derive(Debug, Serialize)]
pub struct RunStatus {
    pub id: i64,
    pub reason: String,
    pub build_id: i64,
    pub status: String,
    /// 1-based position in the queue while queued.
    pub queue_position: Option<i64>,
    pub isolation: Option<String>,
    pub score: Option<f64>,
    pub total: Option<f64>,
    pub problems: Vec<String>,
    pub error: Option<String>,
    pub log_tail: Option<String>,
    /// Regrades: what changed since the previous graded run.
    pub changes: Option<serde_json::Value>,
    /// Whether the student should be told about this run.
    pub notify: bool,
    pub results: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct SubmissionStatus {
    pub id: i64,
    pub assignment_id: String,
    pub student_id: String,
    /// The build current when it was accepted.
    pub build_id: i64,
    pub source: String,
    pub tree_hash: String,
    pub size_bytes: i64,
    pub receipt: serde_json::Value,
    /// Latest first; the first is the one shown.
    pub runs: Vec<RunStatus>,
}

/// `GET /internal/submissions/{id}`: a submission and all its runs.
/// Internal callers see everything; what students see is filtered by the api.
pub async fn submission_status(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> Result<Response, Refused> {
    let submission = db::get_submission(&state.pool, id)
        .await
        .map_err(|e| internal("reading the submission", e))?
        .ok_or_else(|| refused(StatusCode::NOT_FOUND, format!("no submission {id}")))?;
    Ok(Json(status_of(&state, submission).await?).into_response())
}

#[derive(Deserialize)]
pub struct StudentQuery {
    student: String,
}

/// `GET /internal/assignments/{id}/submissions?student=<id>`: a student's
/// submissions to an assignment, latest first, each with its runs.
pub async fn student_submissions(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<StudentQuery>,
) -> Result<Response, Refused> {
    let submissions = db::submissions_for_student(&state.pool, &id, &query.student)
        .await
        .map_err(|e| internal("reading the submissions", e))?;
    let mut out = Vec::with_capacity(submissions.len());
    for submission in submissions {
        out.push(status_of(&state, submission).await?);
    }
    Ok(Json(out).into_response())
}

async fn status_of(
    state: &AppState,
    submission: pedagog_store::Submission,
) -> Result<SubmissionStatus, Refused> {
    let json = |text: Option<String>| text.and_then(|t| serde_json::from_str(&t).ok());
    let mut runs = Vec::new();
    for run in db::runs_for_submission(&state.pool, submission.id)
        .await
        .map_err(|e| internal("reading the runs", e))?
    {
        let queue_position = if run.status == pedagog_store::RunStatus::Queued {
            queue::position(&state.pool, QueuedRun::Grading(run.id))
                .await
                .map_err(|e| internal("reading the queue", e))?
        } else {
            None
        };
        runs.push(RunStatus {
            id: run.id,
            reason: run.reason.to_string(),
            build_id: run.build_id,
            status: run.status.to_string(),
            queue_position,
            isolation: run.isolation.map(|i| i.to_string()),
            score: run.score,
            total: run.total,
            problems: run.problems,
            error: run.error,
            log_tail: run.log_tail,
            changes: json(run.changes),
            notify: run.notify,
            results: json(run.results),
        });
    }
    Ok(SubmissionStatus {
        id: submission.id,
        assignment_id: submission.assignment_id,
        student_id: submission.student_id,
        build_id: submission.build_id,
        source: submission.source.to_string(),
        tree_hash: submission.tree_hash,
        size_bytes: submission.size_bytes,
        receipt: json(Some(submission.receipt)).unwrap_or_default(),
        runs,
    })
}

/// The assignment's current build and its run settings (from the build row;
/// no archive is read).
async fn current_settings(state: &AppState, id: &str) -> Result<(i64, RunSettings), Refused> {
    let build_id = builds_db::current_build_id(&state.pool, id)
        .await
        .map_err(|e| internal("reading the assignment", e))?
        .ok_or_else(|| {
            refused(
                StatusCode::CONFLICT,
                format!("{id} has no usable build yet: upload it and wait for validation"),
            )
        })?;
    let build = builds_db::build_by_id(&state.pool, build_id)
        .await
        .map_err(|e| internal("reading the build", e))?
        .ok_or_else(|| internal("reading the build", format!("build {build_id} is missing")))?;
    let settings = settings_of(state, &build)
        .await
        .map_err(|e| internal("reading the build's settings", e))?;
    Ok((build_id, settings))
}

async fn read_parts(mut multipart: Multipart) -> Result<Vec<Part>, Refused> {
    let bad = |m: String| refused(StatusCode::BAD_REQUEST, m);
    let mut parts = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad(format!("unreadable upload: {e}")))?
    {
        let name = field
            .file_name()
            .or_else(|| field.name())
            .map(str::to_owned)
            .ok_or_else(|| bad("every part needs a file name".to_owned()))?;
        let bytes = field
            .bytes()
            .await
            .map_err(|e| bad(format!("{name}: unreadable: {e}")))?;
        parts.push(Part {
            name,
            bytes: bytes.to_vec(),
        });
    }
    if parts.is_empty() {
        return Err(bad("no files submitted".to_owned()));
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    use crate::archive::tests::{Item, tgz};

    const LIMITS: SubmissionLimits = SubmissionLimits {
        max_bytes: 1 << 20,
        max_files: 100,
    };

    fn rules(yaml: &str) -> SubmissionRules {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn part(name: &str, bytes: &[u8]) -> Part {
        Part {
            name: name.to_owned(),
            bytes: bytes.to_vec(),
        }
    }

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn files(n: &Normalized) -> Vec<&str> {
        n.tree.keys().map(String::as_str).collect()
    }

    #[test]
    fn files_keep_their_relative_paths() {
        let n = normalize(
            vec![part("linked_list.py", b"x"), part("src/util.py", b"y")],
            &rules("{}"),
            &LIMITS,
        )
        .unwrap();
        assert_eq!(files(&n), ["linked_list.py", "src/util.py"]);
        assert_eq!(n.receipt.size_bytes, 2);
        assert_eq!(n.receipt.unpacked, None);
    }

    #[test]
    fn a_zip_from_finder_is_unpacked_unwrapped_and_cleaned() {
        let zip = zip_of(&[
            ("hw1/linked_list.py", b"code"),
            ("hw1/.DS_Store", b"junk"),
            ("__MACOSX/hw1/._linked_list.py", b"junk"),
        ]);
        let n = normalize(vec![part("hw1.zip", &zip)], &rules("{}"), &LIMITS).unwrap();
        assert_eq!(files(&n), ["linked_list.py"]);
        assert_eq!(n.receipt.junk, 2);
        assert_eq!(n.receipt.stripped.as_deref(), Some("hw1"));
        assert_eq!(n.receipt.unpacked.as_deref(), Some("hw1.zip"));
    }

    #[test]
    fn a_tarball_is_unpacked_with_exec_bits() {
        let tgz = tgz(&[
            Item::File("run.sh", b"#!/bin/sh", 0o755),
            Item::File("main.c", b"int main;", 0o644),
        ]);
        let n = normalize(vec![part("work.tar.gz", &tgz)], &rules("{}"), &LIMITS).unwrap();
        assert!(n.tree["run.sh"].executable);
        assert!(!n.tree["main.c"].executable);
        assert_eq!(n.receipt.stripped, None, "two top-level entries");
    }

    #[test]
    fn archives_keep_submits_the_archive_itself() {
        let zip = zip_of(&[("a.py", b"x")]);
        let n = normalize(
            vec![part("work.zip", &zip)],
            &rules("archives: keep"),
            &LIMITS,
        )
        .unwrap();
        assert_eq!(files(&n), ["work.zip"]);
        // Several parts are files even when one is an archive.
        let n = normalize(
            vec![part("data.zip", &zip), part("main.py", b"x")],
            &rules("{}"),
            &LIMITS,
        )
        .unwrap();
        assert_eq!(files(&n), ["data.zip", "main.py"]);
    }

    #[test]
    fn strip_keeps_a_folder_the_required_paths_need() {
        let r = rules("required: [src/main.c]");
        let n = normalize(vec![part("src/main.c", b"x")], &r, &LIMITS).unwrap();
        assert_eq!(files(&n), ["src/main.c"]);
        assert_eq!(n.receipt.stripped, None);
        let n = normalize(
            vec![part("src/main.c", b"x")],
            &rules("strip: false"),
            &LIMITS,
        )
        .unwrap();
        assert_eq!(files(&n), ["src/main.c"]);
        let n = normalize(vec![part("src/main.c", b"x")], &rules("{}"), &LIMITS).unwrap();
        assert_eq!(files(&n), ["main.c"]);
    }

    #[test]
    fn unmatched_files_are_dropped_or_refused() {
        let parts = || vec![part("linked_list.py", b"x"), part("notes.txt", b"y")];
        let n = normalize(parts(), &rules("include: ['*.py']"), &LIMITS).unwrap();
        assert_eq!(files(&n), ["linked_list.py"]);
        assert_eq!(n.receipt.ignored, ["notes.txt"]);
        let err = normalize(
            parts(),
            &rules("include: ['*.py']\nunmatched: refuse"),
            &LIMITS,
        )
        .unwrap_err();
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.error.contains("notes.txt"), "{}", err.error);
    }

    #[test]
    fn required_files_must_be_present() {
        let err = normalize(
            vec![part("other.py", b"x")],
            &rules("required: [linked_list.py]"),
            &LIMITS,
        )
        .unwrap_err();
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            err.error.contains("missing required files: linked_list.py"),
            "{}",
            err.error
        );
    }

    #[test]
    fn nothing_left_is_refused() {
        let err = normalize(vec![part(".DS_Store", b"x")], &rules("{}"), &LIMITS).unwrap_err();
        assert_eq!(err.error, "nothing to submit");
        let err = normalize(
            vec![part("a.txt", b"x")],
            &rules("include: ['*.py']"),
            &LIMITS,
        )
        .unwrap_err();
        assert!(
            err.error.contains("left out by the rules (a.txt)"),
            "{}",
            err.error
        );
    }

    #[test]
    fn sizes_are_capped_by_the_rules_and_the_ceiling() {
        let big = vec![b'a'; 2000];
        let err =
            normalize(vec![part("a.py", &big)], &rules("max_size: 1KiB"), &LIMITS).unwrap_err();
        assert_eq!(err.status, StatusCode::PAYLOAD_TOO_LARGE);
        let tight = SubmissionLimits {
            max_bytes: 100,
            ..LIMITS
        };
        let err =
            normalize(vec![part("a.py", &big)], &rules("max_size: 1MiB"), &tight).unwrap_err();
        assert!(err.error.contains("the limit is 100"), "{}", err.error);
        let zip = zip_of(&[("a.py", &big)]);
        let err = normalize(vec![part("a.zip", &zip)], &rules("{}"), &tight).unwrap_err();
        assert_eq!(
            err.status,
            StatusCode::PAYLOAD_TOO_LARGE,
            "zip bombs stop early"
        );
    }

    #[test]
    fn unsafe_names_are_refused() {
        for name in ["../escape.py", "/etc/passwd", ""] {
            let err = normalize(vec![part(name, b"x")], &rules("{}"), &LIMITS).unwrap_err();
            assert_eq!(err.status, StatusCode::BAD_REQUEST, "{name:?}");
        }
        let zip = zip_of(&[("../../escape.py", b"x")]);
        let err = normalize(vec![part("a.zip", &zip)], &rules("{}"), &LIMITS).unwrap_err();
        assert!(err.error.contains("unsafe path"), "{}", err.error);
        let err = normalize(
            vec![part("a.py", b"x"), part("./a.py", b"y")],
            &rules("{}"),
            &LIMITS,
        )
        .unwrap_err();
        assert!(err.error.contains("twice"), "{}", err.error);
    }

    #[test]
    fn a_corrupt_archive_is_a_bad_request() {
        let err = normalize(vec![part("a.zip", b"not a zip")], &rules("{}"), &LIMITS).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        let err = normalize(vec![part("a.tgz", b"nope")], &rules("{}"), &LIMITS).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }
}
