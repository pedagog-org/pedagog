//! `POST /internal/courses/{course}/assignments`: accept an
//! assignment archive for a course, check it, store it in Garage, and dispatch
//! its build.
//! `GET /internal/courses/{course}/assignments/{slug}/builds/{build}`: an
//! attempt's progress and outcome.

use std::path::Path;
use std::str::FromStr;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use pedagog_core::recipe::assignment::{AssignmentYaml, Isolation};
use pedagog_core::recipe::render::containerfile::Containerfile;
use pedagog_core::recipe::render::{AssignmentFiles, FromSource, Needed, Render, RenderOptions};
use pedagog_core::recipe::resolve::resolve;
use pedagog_core::recipe::store::RecipeStore;
use pedagog_store::db::assignments as db;
use pedagog_store::objects::archive_key;
use pedagog_store::{Assignment, Notify};
use serde::{Deserialize, Serialize};

use crate::archive::{self, ArchiveError, Limits};
use crate::assignments::{self, Decision, Plan};
use crate::http::error::{Refused, internal, refused};
use crate::settings::RunSettings;
use crate::state::AppState;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Uploaded {
    pub course: String,
    /// The assignment's slug (its id in assignment.yml).
    pub assignment: String,
    pub upload_id: i64,
    pub tree_hash: String,
    pub size_bytes: i64,
    /// Things the uploader should know that don't block the upload.
    pub warnings: Vec<String>,
    pub build: Decision,
    /// The file's `assignment:` policy (dates, visibility defaults).
    pub policy: serde_json::Value,
}

#[derive(Deserialize)]
pub struct UploadQuery {
    /// Regrade every student's latest submission once this build becomes
    /// current, telling students `never`, when their results `changed`, or `always`.
    regrade: Option<String>,
    /// Clear the course's date edits once this build becomes current.
    #[serde(default)]
    take_dates: bool,
}

/// An upload that passed every check and is ready to store.
#[derive(Debug)]
pub struct Checked {
    pub assignment: AssignmentYaml,
    pub tree_hash: String,
    /// Rendered FROM the prebuilt OS image in `registry`.
    pub containerfile: String,
    /// The ingredients its build `vend`s.
    pub ingredients: Vec<Needed>,
    pub has_solution: bool,
    pub warnings: Vec<String>,
}

/// Everything about an upload that needs no database or object store: the
/// archive's shape, `assignment.yml`, the files it names, and resolution
/// against jobs' recipes (authoritative over hammer's local check).
pub fn check_upload(
    bytes: &[u8],
    limits: &Limits,
    recipes: &Path,
    registry: &str,
) -> Result<Checked, Refused> {
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
    let spec = resolve(&assignment, &store)
        .map_err(|e| unprocessable(e.to_string()))?
        .with_files(files.clone());
    let containerfile = Containerfile::render(
        &spec,
        &RenderOptions {
            registry: Some(registry.to_owned()),
            from: FromSource::PrebuiltBase,
        },
    )
    .to_string();
    let ingredients = spec.needed_ingredients(FromSource::PrebuiltBase);

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
    if assignment
        .testing
        .as_ref()
        .is_some_and(|t| t.isolation == Isolation::InProcess)
    {
        warnings.push(
            "testing.isolation is in-process: graded student code runs inside the test harness, so it can read the tests and alter its own results".to_owned(),
        );
    }
    Ok(Checked {
        assignment,
        tree_hash: inspected.tree_hash,
        containerfile,
        ingredients,
        has_solution: files.solution.is_some(),
        warnings,
    })
}

/// The course's assignment named by a URL, or 404.
pub(crate) async fn find_assignment(
    state: &AppState,
    course: &str,
    slug: &str,
) -> Result<Assignment, Refused> {
    db::assignment(&state.pool, course, slug)
        .await
        .map_err(|e| internal("reading the assignment", e))?
        .ok_or_else(|| {
            refused(
                StatusCode::NOT_FOUND,
                format!("{course} has no assignment {slug}"),
            )
        })
}

pub async fn upload(
    State(state): State<AppState>,
    UrlPath(course): UrlPath<String>,
    Query(query): Query<UploadQuery>,
    body: Bytes,
) -> Result<Response, Refused> {
    if pedagog_store::db::accounts::course(&state.pool, &course)
        .await
        .map_err(|e| internal("reading the course", e))?
        .is_none()
    {
        return Err(refused(
            StatusCode::NOT_FOUND,
            format!("no course {course}"),
        ));
    }
    let regrade = query
        .regrade
        .as_deref()
        .map(|n| {
            Notify::from_str(n).map_err(|_| {
                refused(
                    StatusCode::BAD_REQUEST,
                    format!("regrade must be never, changed or always, not {n:?}"),
                )
            })
        })
        .transpose()?;
    let limits = state.config.limits;
    let recipes = state.config.recipes_dir.clone();
    let registry = state.config.registry.clone();
    let bytes = body.clone();
    let checked =
        tokio::task::spawn_blocking(move || check_upload(&bytes, &limits, &recipes, &registry))
            .await
            .map_err(|e| internal("checking the upload", e))??;

    let slug = checked.assignment.id.to_string();
    let key = archive_key(&course, &slug, &checked.tree_hash);
    state
        .objects
        .put(&key, body.to_vec())
        .await
        .map_err(|e| internal("storing the archive", e))?;
    let assignment = db::upsert_assignment(&state.pool, &course, &slug, &checked.assignment.name)
        .await
        .map_err(|e| internal("recording the assignment", e))?;
    let size = i64::try_from(body.len()).unwrap_or(i64::MAX);
    let upload = db::record_upload(&state.pool, assignment.id, &checked.tree_hash, &key, size)
        .await
        .map_err(|e| internal("recording the upload", e))?;
    tracing::info!(%course, assignment = %slug, upload = upload.id, tree = %checked.tree_hash, "upload stored");

    let tree_hash = checked.tree_hash.clone();
    let warnings = checked.warnings.clone();
    let policy = serde_json::to_value(&checked.assignment.assignment)
        .map_err(|e| internal("reading the policy", e))?;
    let plan = Plan::from_checked(checked, &assignment, upload.id, key)
        .map_err(|e| internal("planning the build", e))?;
    let build = assignments::dispatch(&state, plan)
        .await
        .map_err(|e| internal("dispatching the build", e))?;
    let Decision::Dispatched { build_id } = build;
    if let Some(notify) = regrade {
        db::request_regrade(&state.pool, build_id, notify)
            .await
            .map_err(|e| internal("requesting the regrade", e))?;
    }
    if query.take_dates {
        db::request_take_dates(&state.pool, build_id)
            .await
            .map_err(|e| internal("taking the file's dates", e))?;
    }

    let reply = Uploaded {
        course,
        assignment: slug,
        upload_id: upload.id,
        tree_hash,
        size_bytes: size,
        warnings,
        build,
        policy,
    };
    Ok((StatusCode::CREATED, Json(reply)).into_response())
}

/// How much of a failed build's log the status endpoint returns.
const ERROR_TAIL: usize = 4000;

#[derive(Debug, Serialize)]
pub struct BuildStatus {
    pub id: i64,
    pub course: String,
    pub assignment: String,
    pub image_ref: String,
    pub build_status: String,
    pub validation_status: String,
    pub finished: bool,
    /// Whether this is the assignment's current (usable) build.
    pub current: bool,
    pub error: Option<String>,
    pub validation_problems: Vec<String>,
    /// The parsed results.yaml, when trusted.
    pub results: Option<serde_json::Value>,
}

pub async fn build_status(
    State(state): State<AppState>,
    UrlPath((course, slug, build)): UrlPath<(String, String, i64)>,
) -> Result<Response, Refused> {
    let assignment = find_assignment(&state, &course, &slug).await?;
    let row = db::get_build(&state.pool, assignment.id, build)
        .await
        .map_err(|e| internal("reading the build", e))?
        .ok_or_else(|| {
            refused(
                StatusCode::NOT_FOUND,
                format!("{course}/{slug} has no build {build}"),
            )
        })?;
    let reply = BuildStatus {
        id: row.id,
        current: assignment.current_build_id == Some(row.id),
        course,
        assignment: slug,
        image_ref: row.image_ref,
        build_status: row.build_status.to_string(),
        validation_status: row.validation_status.to_string(),
        finished: row.finished,
        error: row
            .error
            .map(|e| e[e.len().saturating_sub(ERROR_TAIL)..].to_owned()),
        validation_problems: row.validation_problems,
        results: row.results.and_then(|r| serde_json::from_str(&r).ok()),
    };
    Ok(Json(reply).into_response())
}

impl Plan {
    pub fn from_checked(
        checked: Checked,
        assignment: &Assignment,
        upload_id: i64,
        object_key: String,
    ) -> anyhow::Result<Self> {
        Ok(Plan {
            assignment_id: assignment.id,
            course: assignment.course_id.clone(),
            slug: assignment.slug.clone(),
            upload_id,
            tree_hash: checked.tree_hash,
            object_key,
            containerfile: checked.containerfile,
            ingredients: checked.ingredients,
            has_solution: checked.has_solution,
            settings: RunSettings::of(&checked.assignment),
            policy: serde_json::to_string(&checked.assignment.assignment)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::archive::tests::{Item, LIMITS, tgz};

    const REGISTRY: &str = "registry.test:5000";

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
        let checked = check_upload(&example(&[], None), &LIMITS, &recipes(), REGISTRY).unwrap();
        assert_eq!(checked.assignment.id.to_string(), "linked-list-python");
        assert!(
            checked
                .containerfile
                .starts_with("FROM registry.test:5000/pedagog/ubuntu:22\n")
        );
        assert!(
            checked
                .containerfile
                .contains("COPY --chown=grader:grader tests/ /pedagog/tests/")
        );
        assert!(checked.warnings.is_empty(), "{:?}", checked.warnings);
    }

    #[test]
    fn refusals_carry_the_right_status() {
        let status = |bytes: &[u8]| {
            check_upload(bytes, &LIMITS, &recipes(), REGISTRY)
                .unwrap_err()
                .status
        };
        assert_eq!(status(b"junk"), StatusCode::BAD_REQUEST);
        let mut tiny = LIMITS;
        tiny.max_compressed_bytes = 10;
        assert_eq!(
            check_upload(&example(&[], None), &tiny, &recipes(), REGISTRY)
                .unwrap_err()
                .status,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            status(&example(&[], Some("id: x\n"))),
            StatusCode::UNPROCESSABLE_ENTITY,
            "assignment.yml doesn't parse"
        );
        let unknown_toolchain = "id: x\nname: X\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: workspace\n  toolchains: [cobol:1]\n";
        assert_eq!(
            status(&example(&[], Some(unknown_toolchain))),
            StatusCode::UNPROCESSABLE_ENTITY,
            "doesn't resolve against jobs' recipes"
        );
    }

    #[test]
    fn testing_without_tests_is_refused() {
        let yml = "id: x\nname: X\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: workspace\n  toolchains: [python:3.12]\ntesting:\n  framework: pytest:1\nfiles:\n  tests: hidden\n";
        let err =
            check_upload(&example(&[], Some(yml)), &LIMITS, &recipes(), REGISTRY).unwrap_err();
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.error.contains("hidden/ not found"), "{}", err.error);
    }

    #[test]
    fn host_rules_and_missing_solution_are_warnings() {
        let yml = "id: x\nname: X\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: workspace\n  network:\n    mode: deny\n    allow:\n      - host: pypi.org\n        ports: [443]\n";
        let bytes = tgz(&[Item::File("assignment.yml", yml.as_bytes(), 0o644)]);
        let checked = check_upload(&bytes, &LIMITS, &recipes(), REGISTRY).unwrap();
        assert_eq!(checked.warnings.len(), 2, "{:?}", checked.warnings);
        assert!(checked.warnings[0].contains("pypi.org:443"));
        assert!(checked.warnings[1].contains("validation will be skipped"));
    }

    #[test]
    fn in_process_isolation_is_a_warning() {
        let original =
            std::fs::read_to_string(recipes().join("examples/linked-list-python/assignment.yml"))
                .unwrap();
        let yml = original.replace(
            "framework: pytest:2",
            "framework: pytest:2\n  isolation: in-process",
        );
        let checked =
            check_upload(&example(&[], Some(&yml)), &LIMITS, &recipes(), REGISTRY).unwrap();
        assert_eq!(checked.warnings.len(), 1, "{:?}", checked.warnings);
        assert!(
            checked.warnings[0].contains("can read the tests"),
            "{:?}",
            checked.warnings
        );
    }
}
