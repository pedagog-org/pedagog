//! `POST /internal/assignments`: accept an assignment archive, check it, and
//! store it in Garage. Dispatching its build comes with the build flow.

use std::path::Path;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use pedagog_core::recipe::assignment::AssignmentYaml;
use pedagog_core::recipe::render::AssignmentFiles;
use pedagog_core::recipe::resolve::resolve;
use pedagog_core::recipe::store::RecipeStore;
use pedagog_store::db::assignments;
use pedagog_store::objects::archive_key;
use serde::Serialize;

use crate::archive::{self, ArchiveError, Limits};
use crate::state::AppState;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Uploaded {
    pub assignment_id: String,
    pub upload_id: i64,
    pub tree_hash: String,
    pub size_bytes: i64,
    /// Things the uploader should know that don't block the upload.
    pub warnings: Vec<String>,
}

/// An upload that passed every check and is ready to store.
#[derive(Debug)]
pub struct Checked {
    pub assignment: AssignmentYaml,
    pub tree_hash: String,
    pub warnings: Vec<String>,
}

/// Why an upload was refused, with its HTTP status.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused {
    pub status: StatusCode,
    pub error: String,
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.error });
        (self.status, Json(body)).into_response()
    }
}

fn refused(status: StatusCode, error: impl Into<String>) -> Refused {
    Refused {
        status,
        error: error.into(),
    }
}

/// Everything about an upload that needs no database or object store: the
/// archive's shape, `assignment.yml`, the files it names, and resolution
/// against jobs' recipes (authoritative over hammer's local check).
pub fn check_upload(bytes: &[u8], limits: &Limits, recipes: &Path) -> Result<Checked, Refused> {
    let inspected = archive::inspect(bytes, limits).map_err(|e| match e {
        ArchiveError::TooLarge(m) => refused(StatusCode::PAYLOAD_TOO_LARGE, m),
        ArchiveError::Invalid(m) => refused(StatusCode::BAD_REQUEST, m),
    })?;
    let unprocessable = |m: String| refused(StatusCode::UNPROCESSABLE_ENTITY, m);
    let assignment: AssignmentYaml = serde_yaml::from_str(&inspected.assignment_yaml)
        .map_err(|e| unprocessable(format!("{}: {e}", archive::ASSIGNMENT_FILE)))?;
    let files = AssignmentFiles::detect(&assignment.files, |d| inspected.dirs.contains(d))
        .map_err(unprocessable)?;
    if let Some(problem) = files.problems(&assignment).into_iter().next() {
        return Err(unprocessable(problem));
    }
    let store = RecipeStore::load(&[recipes.to_path_buf()]).map_err(|errs| {
        refused(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("jobs' recipes failed to load ({} errors)", errs.len()),
        )
    })?;
    resolve(&assignment, &store).map_err(|e| unprocessable(e.to_string()))?;

    let mut warnings = Vec::new();
    let hosts: Vec<String> = assignment
        .environment
        .network
        .host_rules()
        .iter()
        .map(ToString::to_string)
        .collect();
    if !hosts.is_empty() {
        warnings.push(format!(
            "network host rules aren't enforced yet (egress proxy pending); unreachable during validation: {}",
            hosts.join(", ")
        ));
    }
    if files.solution.is_none() {
        warnings.push("no reference solution: validation will be skipped".to_owned());
    }
    Ok(Checked {
        assignment,
        tree_hash: inspected.tree_hash,
        warnings,
    })
}

pub async fn upload(State(state): State<AppState>, body: Bytes) -> Result<Response, Refused> {
    let limits = state.config.limits;
    let recipes = state.config.recipes_dir.clone();
    let bytes = body.clone();
    let checked = tokio::task::spawn_blocking(move || check_upload(&bytes, &limits, &recipes))
        .await
        .map_err(|e| internal("checking the upload", e))??;

    let id = checked.assignment.id.to_string();
    let key = archive_key(&id, &checked.tree_hash);
    state
        .objects
        .put(&key, body.to_vec())
        .await
        .map_err(|e| internal("storing the archive", e))?;
    assignments::upsert_assignment(&state.pool, &id, &checked.assignment.name)
        .await
        .map_err(|e| internal("recording the assignment", e))?;
    let size = i64::try_from(body.len()).unwrap_or(i64::MAX);
    let upload = assignments::record_upload(&state.pool, &id, &checked.tree_hash, &key, size)
        .await
        .map_err(|e| internal("recording the upload", e))?;
    tracing::info!(assignment = %id, upload = upload.id, tree = %checked.tree_hash, "upload stored");

    let reply = Uploaded {
        assignment_id: id,
        upload_id: upload.id,
        tree_hash: checked.tree_hash,
        size_bytes: size,
        warnings: checked.warnings,
    };
    Ok((StatusCode::CREATED, Json(reply)).into_response())
}

fn internal(what: &str, e: impl std::fmt::Display) -> Refused {
    tracing::error!(error = %e, "{what} failed");
    refused(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("{what} failed: {e}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::archive::tests::{Item, LIMITS, tgz};

    fn recipes() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes")
    }

    /// The real example assignment, as hammer would pack it.
    fn example(extra: &[(&str, &[u8])], yml: Option<&str>) -> Vec<u8> {
        let root = recipes().join("examples/linked-list-python");
        let original = std::fs::read(root.join("assignment.yml")).unwrap();
        let mut owned: Vec<(String, Vec<u8>)> = vec![(
            "assignment.yml".into(),
            yml.map(|y| y.as_bytes().to_vec()).unwrap_or(original),
        )];
        for dir in ["starter", "tests", "solution"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap() {
                let path = entry.unwrap().path();
                let name = format!("{dir}/{}", path.file_name().unwrap().to_string_lossy());
                owned.push((name, std::fs::read(&path).unwrap()));
            }
        }
        owned.extend(extra.iter().map(|(p, c)| ((*p).to_owned(), c.to_vec())));
        let items: Vec<Item<'_>> = owned.iter().map(|(p, c)| Item::File(p, c, 0o644)).collect();
        tgz(&items)
    }

    #[test]
    fn accepts_the_example_assignment() {
        let checked = check_upload(&example(&[], None), &LIMITS, &recipes()).unwrap();
        assert_eq!(checked.assignment.id.to_string(), "linked-list-python");
        assert!(checked.warnings.is_empty(), "{:?}", checked.warnings);
    }

    #[test]
    fn refusals_carry_the_right_status() {
        let status = |bytes: &[u8]| check_upload(bytes, &LIMITS, &recipes()).unwrap_err().status;
        assert_eq!(status(b"junk"), StatusCode::BAD_REQUEST);
        let mut tiny = LIMITS;
        tiny.max_compressed_bytes = 10;
        assert_eq!(
            check_upload(&example(&[], None), &tiny, &recipes())
                .unwrap_err()
                .status,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            status(&example(&[], Some("id: x\n"))),
            StatusCode::UNPROCESSABLE_ENTITY,
            "assignment.yml doesn't parse"
        );
        let unknown_toolchain = "id: x\nname: X\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: interactive\n  toolchains: [cobol:1]\n";
        assert_eq!(
            status(&example(&[], Some(unknown_toolchain))),
            StatusCode::UNPROCESSABLE_ENTITY,
            "doesn't resolve against jobs' recipes"
        );
    }

    #[test]
    fn testing_without_tests_is_refused() {
        let yml = "id: x\nname: X\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: interactive\n  toolchains: [python:3.12]\ntesting:\n  framework: pytest:1\nfiles:\n  tests: hidden\n";
        let err = check_upload(&example(&[], Some(yml)), &LIMITS, &recipes()).unwrap_err();
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.error.contains("hidden/ not found"), "{}", err.error);
    }

    #[test]
    fn host_rules_and_missing_solution_are_warnings() {
        let yml = "id: x\nname: X\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: interactive\n  network:\n    mode: deny\n    allow:\n      - host: pypi.org\n        ports: [443]\n";
        let bytes = tgz(&[Item::File("assignment.yml", yml.as_bytes(), 0o644)]);
        let checked = check_upload(&bytes, &LIMITS, &recipes()).unwrap();
        assert_eq!(checked.warnings.len(), 2, "{:?}", checked.warnings);
        assert!(checked.warnings[0].contains("pypi.org:443"));
        assert!(checked.warnings[1].contains("validation will be skipped"));
    }
}
