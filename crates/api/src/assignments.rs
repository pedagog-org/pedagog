//! A course's assignments: upload (passed on to jobs, which builds and
//! validates), list, show, each build's progress, outcome and logs, and making
//! an earlier build current.

use std::collections::HashSet;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pedagog_core::api::{
    AssignmentCounts, BuildPage, BuildSummary, BuildsPage, DateDiff, MadeCurrent, Progress,
    RunState, StaffAssignment, StaffAssignmentPage, Uploaded,
};
use pedagog_core::results::Results;
use pedagog_store::db::{accounts, assignments as db, submissions as subs_db};
use pedagog_store::{Assignment, AssignmentBuild, AssignmentBuildStatus, CourseRole};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::auth::CurrentUser;
use crate::courses::{Need, require};
use crate::error::{ApiError, internal, refused};
use crate::policy::{Policy, policy_of, status_text};
use crate::results::run_state;
use crate::state::AppState;

/// How many recent builds `show` lists.
const RECENT_BUILDS: i64 = 10;
/// The most builds one page of `…/builds` returns.
const MAX_BUILDS_PAGE: i64 = 50;

#[derive(Deserialize)]
pub struct UploadQuery {
    regrade: Option<String>,
    /// Adopt the file's dates (clearing the course's edits) once the build is current.
    #[serde(default)]
    take_dates: bool,
}

fn check_regrade(r: &str) -> Result<(), ApiError> {
    if matches!(r, "never" | "changed" | "always") {
        Ok(())
    } else {
        Err(refused(
            StatusCode::BAD_REQUEST,
            format!("regrade must be never, changed or always, not {r:?}"),
        ))
    }
}

/// jobs is down or unreachable.
fn unreachable(e: reqwest::Error) -> ApiError {
    tracing::error!(error = %e, "jobs is unreachable");
    refused(StatusCode::BAD_GATEWAY, "the build service is unreachable")
}

/// jobs' reply status, as ours.
fn status_of(reply: &reqwest::Response) -> StatusCode {
    StatusCode::from_u16(reply.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY)
}

/// The body is the assignment: a `.tgz` or `.zip`, or its files as multipart
/// (a folder from the browser); jobs checks, packs, stores and builds it.
pub async fn upload(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let mut params = vec![format!("by={}", current.user.id)];
    if let Some(r) = q.regrade {
        check_regrade(&r)?;
        params.push(format!("regrade={r}"));
    }
    if q.take_dates {
        params.push("take_dates=true".to_owned());
    }
    let url = format!(
        "{}/internal/courses/{}/assignments?{}",
        state.config.jobs_url,
        access.course.id,
        params.join("&")
    );
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/gzip")
        .to_owned();
    let reply = state
        .jobs
        .post(url)
        .header(header::CONTENT_TYPE, content_type)
        .body(body)
        .send()
        .await
        .map_err(unreachable)?;
    let status = status_of(&reply);
    let text = reply
        .text()
        .await
        .map_err(|e| internal("reading the build service's reply", e))?;
    tracing::info!(by = current.user.id, %course, %status, "assignment uploaded");
    if !status.is_success() {
        return Ok((status, [(header::CONTENT_TYPE, "application/json")], text).into_response());
    }
    let mut reply: Uploaded = serde_json::from_str(&text)
        .map_err(|e| internal("reading the build service's reply", e))?;
    // The course's date edits against the file's, right away (the build comes later).
    // (An upload that went through but can't be read back just goes without it.)
    if let Ok(a) = find(&state, &access.course.id, &reply.assignment).await {
        let file: pedagog_core::recipe::policy::Dates =
            serde_json::from_value(reply.policy["dates"].clone()).unwrap_or_default();
        let course_dates = policy_of(&state, &a).await?.course_dates;
        reply.date_diff = crate::policy::date_diff(&file, &course_dates)
            .into_iter()
            .filter_map(|d| serde_json::from_value::<DateDiff>(d).ok())
            .collect();
        if !reply.date_diff.is_empty() {
            reply.dates_note = Some(if q.take_dates {
                match reply.build["build_id"].as_i64() {
                    Some(b) if reply.build["status"] == "dispatched" => {
                        format!("the file's dates apply once build {b} is current")
                    }
                    _ => "the file's dates apply now".to_owned(),
                }
            } else {
                "the course's dates stay; --take-dates adopts the file's".to_owned()
            });
        }
    }
    Ok((status, Json(reply)).into_response())
}

/// A build as staff lists show it.
pub fn summary(b: &AssignmentBuild, current: Option<i64>) -> BuildSummary {
    let is_current = current == Some(b.id);
    BuildSummary {
        id: b.id,
        build_status: b.build_status.to_string(),
        validation_status: b.validation_status.to_string(),
        finished: b.finished,
        current: is_current,
        started_at: b.started_at.clone(),
        uploaded_by: b.uploaded_by_email.clone(),
        rebuild_of: b.rebuild_of,
        image_deleted: b.image_deleted,
        usable: b.finished
            && b.build_status == AssignmentBuildStatus::Succeeded
            && b.validation_status.is_usable()
            && !is_current,
    }
}

pub(crate) async fn find(
    state: &AppState,
    course: &str,
    slug: &str,
) -> Result<Assignment, ApiError> {
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

/// Where the course's active students stand on `a`.
pub async fn counts(state: &AppState, a: &Assignment) -> Result<AssignmentCounts, ApiError> {
    let students: HashSet<i64> = accounts::active_students(&state.pool, &a.course_id)
        .await
        .map_err(|e| internal("reading the students", e))?
        .into_iter()
        .collect();
    let counting: Vec<i64> = subs_db::counting(&state.pool, a.id)
        .await
        .map_err(|e| internal("reading what counts", e))?
        .into_iter()
        .filter(|c| students.contains(&c.user_id))
        .map(|c| c.submission_id)
        .collect();
    let not_graded = subs_db::latest_runs(&state.pool, &counting)
        .await
        .map_err(|e| internal("reading the runs", e))?
        .into_iter()
        .filter(|r| run_state(r.status) == RunState::Error)
        .count();
    let regrading = db::regrading(&state.pool, a.id)
        .await
        .map_err(|e| internal("reading the regrade", e))?
        .map(|(done, total)| Progress { done, total });
    Ok(AssignmentCounts {
        students: students.len() as i64,
        submitted: counting.len() as i64,
        not_graded: not_graded as i64,
        regrading,
    })
}

/// `a` as staff see it in lists, with its policy.
pub async fn staff_assignment(
    state: &AppState,
    a: &Assignment,
) -> Result<(StaffAssignment, Policy), ApiError> {
    let p = policy_of(state, a).await?;
    let dates = p.dates();
    let now = OffsetDateTime::now_utc();
    let latest = db::builds_of(&state.pool, a.id, 1)
        .await
        .map_err(|e| internal("reading the builds", e))?;
    let view = StaffAssignment {
        slug: a.slug.clone(),
        name: a.name.clone(),
        dates: crate::student::dates_view(&dates),
        window: dates.window(now),
        status: status_text(&dates, now),
        current_build: a.current_build_id,
        latest_build: latest.first().map(|b| summary(b, a.current_build_id)),
        counts: counts(state, a).await?,
    };
    Ok((view, p))
}

/// Everyone in the course sees its assignments: students with their dates and
/// own standing, staff with each one's builds and where its students stand.
pub async fn list(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Member).await?;
    let all = db::assignments_in(&state.pool, &access.course.id)
        .await
        .map_err(|e| internal("reading the assignments", e))?;
    if access.role == Some(CourseRole::Student) {
        let mut out = Vec::new();
        for a in &all {
            out.push(
                crate::student::assignment(&state, a, current.user.id)
                    .await?
                    .0,
            );
        }
        return Ok(Json(out).into_response());
    }
    let mut out = Vec::new();
    for a in &all {
        out.push(staff_assignment(&state, a).await?.0);
    }
    Ok(Json(out).into_response())
}

/// An assignment: for staff its overview, for students their standing and
/// what a submission must be.
pub async fn show(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Member).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    if access.role == Some(CourseRole::Student) {
        let (assignment, _) = crate::student::assignment(&state, &a, current.user.id).await?;
        return Ok(Json(pedagog_core::api::StudentAssignmentPage {
            assignment,
            rules: crate::student::rules(&state, &a).await?,
        })
        .into_response());
    }
    let (assignment, p) = staff_assignment(&state, &a).await?;
    let builds = db::builds_of(&state.pool, a.id, RECENT_BUILDS)
        .await
        .map_err(|e| internal("reading the builds", e))?;
    let current_since = db::current_since(&state.pool, a.id)
        .await
        .map_err(|e| internal("reading the assignment", e))?;
    Ok(Json(StaffAssignmentPage {
        course: a.course_id.clone(),
        assignment,
        current_since,
        released_at: p.released_at.clone(),
        students_see_raw_errors: p.tests.as_ref().is_some_and(|r| !r.sanitizes_errors),
        builds: builds
            .iter()
            .map(|b| summary(b, a.current_build_id))
            .collect(),
    })
    .into_response())
}

#[derive(Deserialize)]
pub struct BuildsQuery {
    before: Option<i64>,
    limit: Option<i64>,
}

/// `GET …/builds`: the assignment's builds, newest first, a page at a time.
pub async fn builds(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Query(q): Query<BuildsQuery>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::See).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let limit = q.limit.unwrap_or(MAX_BUILDS_PAGE).clamp(1, MAX_BUILDS_PAGE);
    let builds = db::builds_before(&state.pool, a.id, q.before, limit)
        .await
        .map_err(|e| internal("reading the builds", e))?;
    let next = (builds.len() as i64 == limit)
        .then(|| builds.last().map(|b| b.id))
        .flatten();
    Ok(Json(BuildsPage {
        builds: builds
            .iter()
            .map(|b| summary(b, a.current_build_id))
            .collect(),
        next,
    })
    .into_response())
}

async fn the_build(
    state: &AppState,
    a: &Assignment,
    build: i64,
) -> Result<AssignmentBuild, ApiError> {
    db::get_build(&state.pool, a.id, build)
        .await
        .map_err(|e| internal("reading the build", e))?
        .ok_or_else(|| {
            refused(
                StatusCode::NOT_FOUND,
                format!("{}/{} has no build {build}", a.course_id, a.slug),
            )
        })
}

/// One build's progress and outcome (staff).
pub async fn build(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug, build)): Path<(String, String, i64)>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::See).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let b = the_build(&state, &a, build).await?;
    let results: Option<Results> = b
        .results
        .as_deref()
        .and_then(|r| serde_json::from_str(r).ok());
    Ok(Json(BuildPage {
        build: summary(&b, a.current_build_id),
        course: a.course_id.clone(),
        assignment: a.slug.clone(),
        image_ref: b.image_ref.clone(),
        error: b.error.clone(),
        validation_problems: b.validation_problems.clone(),
        validation_warnings: results.as_ref().map(Results::warnings).unwrap_or_default(),
        build_ms: b.build_ms,
        validation_ms: b.validation_ms,
        build_log_lines: b.build_log_lines,
        validation_log_lines: b.validation_log_lines,
        tests: results
            .as_ref()
            .map(pedagog_core::view::reference_tests)
            .unwrap_or_default(),
        score: results.as_ref().map(|r| r.summary.score),
        total: results.as_ref().map(|r| r.summary.total),
    })
    .into_response())
}

#[derive(Deserialize)]
pub struct LogQuery {
    phase: String,
    /// Only the last this many lines.
    tail: Option<usize>,
}

/// `GET …/builds/{id}/log?phase=build|validation[&tail=n]`: a phase's log as
/// text (staff), from jobs.
pub async fn log(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug, build)): Path<(String, String, i64)>,
    Query(q): Query<LogQuery>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::See).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    the_build(&state, &a, build).await?;
    if !matches!(q.phase.as_str(), "build" | "validation") {
        return Err(refused(
            StatusCode::BAD_REQUEST,
            format!("phase must be build or validation, not {:?}", q.phase),
        ));
    }
    let url = format!(
        "{}/internal/builds/{build}/log/{}{}",
        state.config.jobs_url,
        q.phase,
        q.tail.map(|n| format!("?tail={n}")).unwrap_or_default()
    );
    let reply = state.jobs.get(url).send().await.map_err(unreachable)?;
    let status = status_of(&reply);
    let content_type = reply
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain; charset=utf-8")
        .to_owned();
    let bytes = reply
        .bytes()
        .await
        .map_err(|e| internal("reading the log", e))?;
    Ok((status, [(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

#[derive(Deserialize)]
pub struct CurrentBody {
    /// Regrade every student's latest submission with it, telling them
    /// `never`, when their results `changed`, or `always`.
    regrade: Option<String>,
}

/// `POST …/builds/{id}/current` (instructors): make an earlier build current,
/// cancelling any build in flight; rebuilt first if its image was cleaned up.
pub async fn make_current(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug, build)): Path<(String, String, i64)>,
    Json(body): Json<CurrentBody>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let mut params = vec![format!("by={}", current.user.id)];
    if let Some(r) = body.regrade {
        check_regrade(&r)?;
        params.push(format!("regrade={r}"));
    }
    let url = format!(
        "{}/internal/courses/{}/assignments/{}/builds/{build}/current?{}",
        state.config.jobs_url,
        a.course_id,
        a.slug,
        params.join("&")
    );
    let reply = state.jobs.post(url).send().await.map_err(unreachable)?;
    let status = status_of(&reply);
    let text = reply
        .text()
        .await
        .map_err(|e| internal("reading the build service's reply", e))?;
    tracing::info!(by = current.user.id, %course, %slug, build, %status, "make current");
    if !status.is_success() {
        return Ok((status, [(header::CONTENT_TYPE, "application/json")], text).into_response());
    }
    let made: MadeCurrent = serde_json::from_str(&text)
        .map_err(|e| internal("reading the build service's reply", e))?;
    Ok(Json(made).into_response())
}

#[cfg(test)]
mod tests {
    use pedagog_store::db::accounts::{self, Actor};
    use pedagog_store::{BuildSettings, CourseRole, Isolation, JoinSource, PlatformKind};
    use sqlx::PgPool;

    use crate::testing::{get, json, login, serve_full};

    async fn course_with_staff(pool: &PgPool) -> sqlx::Result<()> {
        let prof = accounts::upsert_user(pool, "prof@ufl.edu", "").await?;
        accounts::create_course(pool, "cop3502-f26", "Prog 1", prof.id, &[prof.id]).await?;
        let me = Actor {
            id: prof.id,
            admin: false,
        };
        for (email, role) in [
            ("ta@ufl.edu", CourseRole::Grader),
            ("alice@ufl.edu", CourseRole::Student),
        ] {
            let u = accounts::upsert_user(pool, email, "").await?;
            accounts::add_member(pool, "cop3502-f26", u.id, role, JoinSource::Manual, me)
                .await?
                .unwrap();
        }
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn uploads_go_to_jobs_for_instructors_only(pool: PgPool) -> sqlx::Result<()> {
        course_with_staff(&pool).await?;
        let (url, outbox, jobs) = serve_full(pool.clone()).await;
        let send = |token: String, q: &'static str| {
            let url = url.clone();
            async move {
                reqwest::Client::new()
                    .post(format!("{url}/courses/cop3502-f26/assignments{q}"))
                    .bearer_auth(token)
                    .body(vec![1u8, 2, 3])
                    .send()
                    .await
                    .unwrap()
            }
        };
        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        assert_eq!(send(ta, "").await.status(), 403);
        assert!(jobs.lock().unwrap().is_empty());

        let prof = login(&url, &outbox, "prof@ufl.edu").await;
        let r = send(prof, "?regrade=changed&junk=1").await;
        assert_eq!(r.status(), 201);
        assert_eq!(
            json(r).await["course"],
            "cop3502-f26",
            "jobs' reply comes back"
        );
        let seen = jobs.lock().unwrap().clone();
        let prof_id = accounts::upsert_user(&pool, "prof@ufl.edu", "").await?.id;
        assert_eq!(
            seen,
            [(
                format!(
                    "/internal/courses/cop3502-f26/assignments?by={prof_id}&regrade=changed application/gzip"
                ),
                3
            )]
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn a_folder_upload_reaches_jobs_as_multipart(pool: PgPool) -> sqlx::Result<()> {
        course_with_staff(&pool).await?;
        let (url, outbox, jobs) = serve_full(pool.clone()).await;
        let prof = login(&url, &outbox, "prof@ufl.edu").await;
        let body = "--XyZ\r\nContent-Disposition: form-data; name=\"f\"; \
                    filename=\"hw/assignment.yml\"\r\n\r\nid: x\r\n--XyZ--\r\n";
        let r = reqwest::Client::new()
            .post(format!("{url}/courses/cop3502-f26/assignments"))
            .bearer_auth(prof)
            .header("content-type", "multipart/form-data; boundary=XyZ")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201);
        let seen = jobs.lock().unwrap().clone();
        assert!(
            seen[0].0.ends_with(" multipart/form-data; boundary=XyZ"),
            "the boundary goes along: {seen:?}"
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn staff_see_builds_and_students_their_own_view(pool: PgPool) -> sqlx::Result<()> {
        use pedagog_store::db::assignments::*;
        course_with_staff(&pool).await?;
        let a = upsert_assignment(&pool, "cop3502-f26", "linked-list", "Linked List").await?;
        let up = record_upload(&pool, a.id, "tree", "k", 1).await?;
        let settings = BuildSettings {
            platform: PlatformKind::Upload,
            isolation: Isolation::Process,
            egress: r#"{"mode":"deny"}"#.into(),
            submission: "{}".into(),
        };
        let b = start_build(
            &pool,
            &NewBuild {
                assignment_id: a.id,
                upload_id: up.id,
                image_hash: "h",
                image_ref: "reg/pedagog/assignments/cop3502-f26/linked-list:h",
                settings: &settings,
                policy: "{}",
                uploaded_by: None,
                rebuild_of: None,
            },
        )
        .await?;
        finish_build(&pool, b, false, Some("kaniko: exit 1")).await?;
        let (url, outbox, _) = serve_full(pool.clone()).await;

        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        let listed =
            json(get(&format!("{url}/courses/cop3502-f26/assignments"), &alice).await).await;
        assert_eq!(
            (
                &listed[0]["slug"],
                &listed[0]["window"],
                &listed[0]["accepting"]
            ),
            (
                &serde_json::json!("linked-list"),
                &serde_json::json!("no_dates"),
                &serde_json::json!(false)
            )
        );
        assert!(
            listed[0].get("latest_build").is_none(),
            "builds stay with staff"
        );
        let page = json(
            get(
                &format!("{url}/courses/cop3502-f26/assignments/linked-list"),
                &alice,
            )
            .await,
        )
        .await;
        assert_eq!(page["rules"], serde_json::Value::Null, "no usable build");
        assert!(page.get("builds").is_none());

        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        let listed = json(get(&format!("{url}/courses/cop3502-f26/assignments"), &ta).await).await;
        assert_eq!(listed[0]["latest_build"]["build_status"], "failed");
        let shown = json(
            get(
                &format!("{url}/courses/cop3502-f26/assignments/linked-list"),
                &ta,
            )
            .await,
        )
        .await;
        assert_eq!(
            (
                shown["builds"][0]["id"].as_i64(),
                shown["current_build"].clone()
            ),
            (Some(b), serde_json::Value::Null)
        );
        let detail = json(
            get(
                &format!("{url}/courses/cop3502-f26/assignments/linked-list/builds/{b}"),
                &ta,
            )
            .await,
        )
        .await;
        assert_eq!(
            (
                detail["error"].as_str(),
                detail["finished"].as_bool(),
                detail["assignment"].as_str()
            ),
            (Some("kaniko: exit 1"), Some(true), Some("linked-list"))
        );
        assert_eq!(
            get(&format!("{url}/courses/cop3502-f26/assignments/nope"), &ta)
                .await
                .status(),
            404
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn staff_get_the_overview_builds_logs_and_make_current(pool: PgPool) -> sqlx::Result<()> {
        use pedagog_store::ValidationStatus;
        use pedagog_store::db::assignments::*;
        course_with_staff(&pool).await?;
        let a = upsert_assignment(&pool, "cop3502-f26", "linked-list", "Linked List").await?;
        let up = record_upload(&pool, a.id, "tree", "k", 1).await?;
        let settings = BuildSettings {
            platform: PlatformKind::Upload,
            isolation: Isolation::Process,
            egress: r#"{"mode":"deny"}"#.into(),
            submission: "{}".into(),
        };
        let prof = accounts::upsert_user(&pool, "prof@ufl.edu", "").await?;
        let new = |image: &'static str| NewBuild {
            assignment_id: a.id,
            upload_id: up.id,
            image_hash: image,
            image_ref: image,
            settings: &settings,
            policy: "{}",
            uploaded_by: Some(prof.id),
            rebuild_of: None,
        };
        let results = r#"{"summary": {"score": 2, "total": 2}, "tests": [
            {"node": "t::a", "name": "append", "status": "pass", "score": 2, "max": 2,
             "visibility": {"score": "visible", "feedback": "visible", "stdout": "visible", "error": "visible"}}]}"#;
        let mut ids = vec![];
        for image in ["a", "b", "c"] {
            let b = start_build(&pool, &new(image)).await?;
            finish_build(&pool, b, true, None).await?;
            start_validation(&pool, b).await?;
            finish_validation(&pool, b, ValidationStatus::Passed, Some(results), &[]).await?;
            ids.push(b);
        }
        log_stored(&pool, ids[2], LogPhase::Build, 12).await?;
        let flying = start_build(&pool, &new("d")).await?;
        let (url, outbox, jobs) = serve_full(pool.clone()).await;
        let base = format!("{url}/courses/cop3502-f26/assignments/linked-list");

        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        let page: pedagog_core::api::StaffAssignmentPage =
            serde_json::from_value(json(get(&base, &ta).await).await).unwrap();
        assert_eq!(page.assignment.current_build, Some(ids[2]));
        assert!(page.current_since.is_some());
        assert!(page.students_see_raw_errors, "this helper doesn't sanitize");
        assert_eq!(page.assignment.counts.students, 1);
        assert_eq!(page.builds[0].id, flying);
        let older = &page.builds[2];
        assert_eq!(
            (older.id, older.usable, older.uploaded_by.as_deref()),
            (ids[1], true, Some("prof@ufl.edu"))
        );

        let first: pedagog_core::api::BuildsPage =
            serde_json::from_value(json(get(&format!("{base}/builds?limit=2"), &ta).await).await)
                .unwrap();
        assert_eq!(
            first.builds.iter().map(|b| b.id).collect::<Vec<_>>(),
            [flying, ids[2]]
        );
        let rest: pedagog_core::api::BuildsPage = serde_json::from_value(
            json(get(&format!("{base}/builds?before={}", ids[2]), &ta).await).await,
        )
        .unwrap();
        assert_eq!(
            rest.builds.iter().map(|b| b.id).collect::<Vec<_>>(),
            [ids[1], ids[0]]
        );
        assert_eq!(rest.next, None);

        let build: pedagog_core::api::BuildPage = serde_json::from_value(
            json(get(&format!("{base}/builds/{}", ids[2]), &ta).await).await,
        )
        .unwrap();
        assert_eq!(build.build_log_lines, Some(12));
        assert_eq!(build.tests.len(), 1);
        assert!(build.tests[0].as_expected);
        let log = get(
            &format!("{base}/builds/{}/log?phase=build&tail=200", ids[2]),
            &ta,
        )
        .await;
        assert_eq!(log.status(), 200);
        assert_eq!(
            log.text().await.unwrap(),
            format!("build log of {} (tail=200)\n", ids[2])
        );
        assert_eq!(
            get(&format!("{base}/builds/{}/log?phase=other", ids[2]), &ta)
                .await
                .status(),
            400
        );

        let make = |token: String, build: i64| {
            let base = base.clone();
            async move {
                reqwest::Client::new()
                    .post(format!("{base}/builds/{build}/current"))
                    .bearer_auth(token)
                    .json(&serde_json::json!({"regrade": "changed"}))
                    .send()
                    .await
                    .unwrap()
            }
        };
        assert_eq!(
            make(ta.clone(), ids[0]).await.status(),
            403,
            "graders don't"
        );
        let prof_token = login(&url, &outbox, "prof@ufl.edu").await;
        let made = make(prof_token.clone(), ids[0]).await;
        assert_eq!(made.status(), 200);
        assert_eq!(json(made).await["status"], "current");
        assert!(jobs.lock().unwrap().iter().any(|(path, _)| path
            == &format!(
                "/internal/courses/cop3502-f26/assignments/linked-list/builds/{}/current?by={}&regrade=changed",
                ids[0], prof.id
            )));

        let home = json(get(&format!("{url}/me/courses"), &prof_token).await).await;
        let items = &home["courses"][0]["staff_items"];
        assert_eq!(items[0]["kind"], "building", "{items}");
        assert_eq!(items[0]["build"], flying);
        Ok(())
    }
}
