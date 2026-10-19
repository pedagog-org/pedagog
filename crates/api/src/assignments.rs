//! A course's assignments: upload (passed on to jobs, which builds and
//! validates), list, show, and a build's progress and outcome (read from
//! Postgres).

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use pedagog_store::db::assignments as db;
use pedagog_store::{Assignment, AssignmentBuild};
use serde::{Deserialize, Serialize};

use crate::auth::CurrentUser;
use crate::courses::{Need, require};
use crate::error::{ApiError, internal, refused};
use crate::state::AppState;

/// How many recent builds `show` lists.
const RECENT_BUILDS: i64 = 10;
/// How much of a failed build's log a build's status carries.
const ERROR_TAIL: usize = 4000;

#[derive(Deserialize)]
pub struct UploadQuery {
    #[serde(default)]
    force: bool,
    regrade: Option<String>,
}

/// The body is the assignment archive; jobs checks, stores and builds it, and
/// its reply comes back as is.
pub async fn upload(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let mut params = vec![];
    if q.force {
        params.push("force=true".to_owned());
    }
    if let Some(r) = q.regrade {
        if !matches!(r.as_str(), "never" | "changed" | "always") {
            return Err(refused(
                StatusCode::BAD_REQUEST,
                format!("regrade must be never, changed or always, not {r:?}"),
            ));
        }
        params.push(format!("regrade={r}"));
    }
    let url = format!(
        "{}/internal/courses/{}/assignments{}",
        state.config.jobs_url,
        access.course.id,
        if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        }
    );
    let reply = state
        .jobs
        .post(url)
        .header(header::CONTENT_TYPE, "application/gzip")
        .body(body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "jobs is unreachable");
            refused(StatusCode::BAD_GATEWAY, "the build service is unreachable")
        })?;
    let status = StatusCode::from_u16(reply.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = reply
        .text()
        .await
        .map_err(|e| internal("reading the build service's reply", e))?;
    tracing::info!(by = current.user.id, %course, %status, "assignment uploaded");
    Ok((status, [(header::CONTENT_TYPE, "application/json")], text).into_response())
}

#[derive(Serialize)]
struct BuildSummary {
    id: i64,
    build_status: String,
    validation_status: String,
    finished: bool,
    current: bool,
}

impl BuildSummary {
    fn of(b: &AssignmentBuild, current: Option<i64>) -> Self {
        Self {
            id: b.id,
            build_status: b.build_status.to_string(),
            validation_status: b.validation_status.to_string(),
            finished: b.finished,
            current: current == Some(b.id),
        }
    }
}

async fn find(state: &AppState, course: &str, slug: &str) -> Result<Assignment, ApiError> {
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

/// Everyone in the course sees its assignments; staff also see each one's
/// latest build.
pub async fn list(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Member).await?;
    let staff = access.role != Some(pedagog_store::CourseRole::Student);
    let mut out = Vec::new();
    for a in db::assignments_in(&state.pool, &access.course.id)
        .await
        .map_err(|e| internal("reading the assignments", e))?
    {
        let mut view = serde_json::json!({ "slug": a.slug, "name": a.name });
        if staff {
            let latest = db::builds_of(&state.pool, a.id, 1)
                .await
                .map_err(|e| internal("reading the builds", e))?;
            view["current_build"] = serde_json::json!(a.current_build_id);
            view["latest_build"] = serde_json::json!(
                latest
                    .first()
                    .map(|b| BuildSummary::of(b, a.current_build_id))
            );
        }
        out.push(view);
    }
    Ok(Json(out).into_response())
}

/// An assignment and its recent builds (staff).
pub async fn show(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    require(&state, &course, &current, Need::See).await?;
    let a = find(&state, &course, &slug).await?;
    let builds = db::builds_of(&state.pool, a.id, RECENT_BUILDS)
        .await
        .map_err(|e| internal("reading the builds", e))?;
    Ok(Json(serde_json::json!({
        "course": a.course_id,
        "slug": a.slug,
        "name": a.name,
        "current_build": a.current_build_id,
        "builds": builds.iter().map(|b| BuildSummary::of(b, a.current_build_id)).collect::<Vec<_>>(),
    }))
    .into_response())
}

/// One build's progress and outcome (staff): the same shape as jobs'.
pub async fn build(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug, build)): Path<(String, String, i64)>,
) -> Result<Response, ApiError> {
    require(&state, &course, &current, Need::See).await?;
    let a = find(&state, &course, &slug).await?;
    let b = db::get_build(&state.pool, a.id, build)
        .await
        .map_err(|e| internal("reading the build", e))?
        .ok_or_else(|| {
            refused(
                StatusCode::NOT_FOUND,
                format!("{course}/{slug} has no build {build}"),
            )
        })?;
    Ok(Json(serde_json::json!({
        "id": b.id,
        "course": a.course_id,
        "assignment": a.slug,
        "image_ref": b.image_ref,
        "build_status": b.build_status.to_string(),
        "validation_status": b.validation_status.to_string(),
        "finished": b.finished,
        "current": a.current_build_id == Some(b.id),
        "error": b.error.map(|e| e[e.len().saturating_sub(ERROR_TAIL)..].to_owned()),
        "validation_problems": b.validation_problems,
        "results": b.results.and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok()),
    }))
    .into_response())
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
        let r = send(prof, "?force=true&regrade=changed&junk=1").await;
        assert_eq!(r.status(), 201);
        assert_eq!(
            json(r).await["course"],
            "cop3502-f26",
            "jobs' reply comes back"
        );
        let seen = jobs.lock().unwrap().clone();
        assert_eq!(
            seen,
            [(
                "/internal/courses/cop3502-f26/assignments?force=true&regrade=changed".to_owned(),
                3
            )]
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn staff_see_builds_and_students_only_the_list(pool: PgPool) -> sqlx::Result<()> {
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
            },
        )
        .await?;
        finish_build(&pool, b, false, Some("kaniko: exit 1")).await?;
        let (url, outbox, _) = serve_full(pool.clone()).await;

        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        let listed =
            json(get(&format!("{url}/courses/cop3502-f26/assignments"), &alice).await).await;
        assert_eq!(
            listed,
            serde_json::json!([{"slug": "linked-list", "name": "Linked List"}])
        );
        assert_eq!(
            get(
                &format!("{url}/courses/cop3502-f26/assignments/linked-list"),
                &alice
            )
            .await
            .status(),
            403
        );

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
}
