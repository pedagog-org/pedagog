//! Courses, and who may do what in them.
//!
//! Staff and admins create courses. In a course, instructors manage
//! everything, graders see the members and every result, and admins act as
//! an instructor everywhere (logged).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pedagog_store::db::accounts as db;
use pedagog_store::{Course, CourseRole};
use serde::Deserialize;

use crate::auth::CurrentUser;
use crate::error::{ApiError, internal, refused};
use crate::state::AppState;
use crate::users::{UserView, forbidden, page, pages, require_admin, valid_email};

/// What a request needs in a course.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Anyone in the course (e.g. its list of assignments).
    Member,
    /// Members, results: instructors and graders.
    See,
    /// Changes: instructors.
    Manage,
}

/// The caller in a course they may act in.
pub struct Access {
    pub course: Course,
    /// `None` for an admin who isn't a member.
    pub role: Option<CourseRole>,
}

impl Access {
    /// The caller's role, as listings show it.
    pub fn label(&self) -> String {
        self.role
            .map(|r| r.to_string())
            .unwrap_or_else(|| "admin".to_owned())
    }

    pub fn manages(&self) -> bool {
        matches!(self.role, None | Some(CourseRole::Instructor))
    }
}

fn allows(role: Option<CourseRole>, need: Need) -> bool {
    matches!(
        (role, need),
        (Some(CourseRole::Instructor), _)
            | (Some(CourseRole::Grader), Need::See | Need::Member)
            | (Some(CourseRole::Student), Need::Member)
    )
}

/// 404 for a course that doesn't exist, saying where it went if it was renamed.
pub async fn no_course(state: &AppState, course: &str) -> ApiError {
    match db::renamed_to(&state.pool, course).await {
        Ok(Some((to, at))) => refused(
            StatusCode::NOT_FOUND,
            format!(
                "no course {course} (renamed to {to} on {})",
                at.get(..10).unwrap_or(&at)
            ),
        ),
        Ok(None) => refused(StatusCode::NOT_FOUND, format!("no course {course}")),
        Err(e) => internal("reading the course", e),
    }
}

/// The course, if the caller may do what `need` says there: 404 if it
/// doesn't exist, 403 if they may not.
pub async fn require(
    state: &AppState,
    course: &str,
    current: &CurrentUser,
    need: Need,
) -> Result<Access, ApiError> {
    let Some(found) = db::course(&state.pool, course)
        .await
        .map_err(|e| internal("reading the course", e))?
    else {
        return Err(no_course(state, course).await);
    };
    let role = db::role_in(&state.pool, course, current.user.id)
        .await
        .map_err(|e| internal("reading your role", e))?;
    if allows(role, need) {
        return Ok(Access {
            course: found,
            role,
        });
    }
    if current.user.is_admin() {
        tracing::info!(admin = current.user.id, %course, ?need, "admin acting in a course");
        return Ok(Access {
            course: found,
            role: if role == Some(CourseRole::Instructor) {
                role
            } else {
                None
            },
        });
    }
    Err(forbidden(&match need {
        Need::Member => format!("members of {course}"),
        Need::See => format!("{course}'s instructors and graders"),
        Need::Manage => format!("{course}'s instructors"),
    }))
}

/// Course ids: lowercase letters, digits and inner dashes, up to 40.
fn valid_course_id(id: &str) -> Result<(), ApiError> {
    let ok = !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(refused(
            StatusCode::BAD_REQUEST,
            format!(
                "{id:?} isn't a course id: use lowercase letters, digits and dashes, e.g. cop3502-f26"
            ),
        ))
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    all: bool,
    search: Option<String>,
    page: Option<i64>,
}

/// Your courses; with `all` (admins), every course, a page at a time.
pub async fn list(
    State(state): State<AppState>,
    current: CurrentUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    if q.all {
        require_admin(&current)?;
        let (limit, offset, page) = page(q.page);
        let (courses, total) = db::all_courses(&state.pool, q.search.as_deref(), limit, offset)
            .await
            .map_err(|e| internal("reading courses", e))?;
        return Ok(Json(serde_json::json!({
            "courses": courses.iter().map(|c| serde_json::json!({"id": c.id, "name": c.name})).collect::<Vec<_>>(),
            "total": total,
            "page": page,
            "pages": pages(total),
        }))
        .into_response());
    }
    let mine = db::courses_for(&state.pool, current.user.id)
        .await
        .map_err(|e| internal("reading your courses", e))?;
    Ok(Json(serde_json::json!({
        "courses": mine.iter().map(|(c, role)| serde_json::json!({
            "id": c.id, "name": c.name, "role": role.to_string(),
        })).collect::<Vec<_>>(),
    }))
    .into_response())
}

#[derive(Deserialize)]
pub struct NewCourse {
    id: String,
    name: String,
    /// Who teaches it; the creator if none.
    #[serde(default)]
    instructors: Vec<String>,
}

pub async fn create(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(body): Json<NewCourse>,
) -> Result<Response, ApiError> {
    if !current.user.creates_courses() {
        return Err(forbidden("staff and admins"));
    }
    valid_course_id(&body.id)?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(refused(StatusCode::BAD_REQUEST, "a course needs a name"));
    }
    let emails = if body.instructors.is_empty() {
        vec![current.user.email.clone()]
    } else {
        body.instructors
            .iter()
            .map(|e| valid_email(e))
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut ids = Vec::new();
    for email in &emails {
        let user = db::upsert_user(&state.pool, email, "")
            .await
            .map_err(|e| internal("adding an instructor", e))?;
        ids.push(user.id);
    }
    if !db::create_course(&state.pool, &body.id, name, current.user.id, &ids)
        .await
        .map_err(|e| internal("creating the course", e))?
    {
        return Err(refused(
            StatusCode::CONFLICT,
            format!("{} already exists", body.id),
        ));
    }
    tracing::info!(by = current.user.id, course = %body.id, "course created");
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": body.id, "name": name, "instructors": emails })),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct RenameBody {
    /// A new slug.
    id: Option<String>,
    name: Option<String>,
}

/// Change a course's slug and/or display name (instructors and admins).
pub async fn rename(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<Response, ApiError> {
    require(&state, &course, &current, Need::Manage).await?;
    if let Some(id) = &body.id {
        valid_course_id(id)?;
    }
    let name = body.name.as_deref().map(str::trim);
    if name == Some("") {
        return Err(refused(StatusCode::BAD_REQUEST, "a course needs a name"));
    }
    if body.id.is_none() && name.is_none() {
        return Err(refused(StatusCode::BAD_REQUEST, "give a new id or name"));
    }
    match db::rename_course(&state.pool, &course, body.id.as_deref(), name)
        .await
        .map_err(|e| internal("renaming the course", e))?
    {
        db::Renamed::Done(c) => {
            tracing::info!(by = current.user.id, from = %course, to = %c.id, "course renamed");
            Ok(
                Json(serde_json::json!({ "id": c.id, "name": c.name, "was": course }))
                    .into_response(),
            )
        }
        db::Renamed::Taken => Err(refused(
            StatusCode::CONFLICT,
            format!("{} already exists", body.id.unwrap_or_default()),
        )),
        db::Renamed::Missing => Err(no_course(&state, &course).await),
    }
}

/// A course's staff and how many students it has.
pub async fn show(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::See).await?;
    let members = db::members(&state.pool, &course, None, true)
        .await
        .map_err(|e| internal("reading the members", e))?;
    let active = |role: CourseRole| {
        members
            .iter()
            .filter(move |m| m.role == role && m.dropped_at.is_none())
    };
    Ok(Json(serde_json::json!({
        "id": access.course.id,
        "name": access.course.name,
        "role": access.label(),
        "instructors": active(CourseRole::Instructor).map(|m| UserView::from(&m.user)).collect::<Vec<_>>(),
        "graders": active(CourseRole::Grader).map(|m| UserView::from(&m.user)).collect::<Vec<_>>(),
        "students": active(CourseRole::Student).count(),
        "dropped": members.iter().filter(|m| m.dropped_at.is_some()).count(),
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    use pedagog_store::GlobalRole;
    use sqlx::PgPool;

    use crate::testing::{get, json, login, post, serve};

    #[test]
    fn instructors_manage_and_graders_see() {
        use CourseRole::*;
        assert!(allows(Some(Instructor), Need::Manage));
        assert!(allows(Some(Grader), Need::See));
        assert!(!allows(Some(Grader), Need::Manage));
        assert!(!allows(Some(Student), Need::See));
        assert!(allows(Some(Student), Need::Member));
        assert!(!allows(None, Need::See));
        assert!(!allows(None, Need::Member));
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn staff_create_courses_and_teach_them(pool: PgPool) -> sqlx::Result<()> {
        db::set_role(&pool, "prof@ufl.edu", GlobalRole::Staff).await?;
        db::upsert_user(&pool, "grad@ufl.edu", "").await?;
        let (url, outbox) = serve(pool.clone()).await;
        let prof = login(&url, &outbox, "prof@ufl.edu").await;

        let created = post(
            &format!("{url}/courses"),
            serde_json::json!({"id": "cop3502-f26", "name": "Programming 1"}),
            Some(&prof),
        )
        .await;
        assert_eq!(created.status(), 201);
        assert_eq!(
            json(created).await["instructors"],
            serde_json::json!(["prof@ufl.edu"])
        );
        let again = post(
            &format!("{url}/courses"),
            serde_json::json!({"id": "cop3502-f26", "name": "x"}),
            Some(&prof),
        )
        .await;
        assert_eq!(again.status(), 409);
        let bad = post(
            &format!("{url}/courses"),
            serde_json::json!({"id": "COP 3502", "name": "x"}),
            Some(&prof),
        )
        .await;
        assert_eq!(bad.status(), 400);

        let mine = json(get(&format!("{url}/courses"), &prof).await).await;
        assert_eq!(mine["courses"][0]["role"], "instructor");
        let shown = json(get(&format!("{url}/courses/cop3502-f26"), &prof).await).await;
        assert_eq!(
            (
                shown["role"].as_str(),
                shown["instructors"][0]["email"].as_str(),
                shown["students"].as_i64()
            ),
            (Some("instructor"), Some("prof@ufl.edu"), Some(0))
        );

        // An instructor who isn't staff can't create courses.
        let course = db::course(&pool, "cop3502-f26").await?.unwrap();
        let grad = db::user_by_email(&pool, "grad@ufl.edu").await?.unwrap();
        let p = db::user_by_email(&pool, "prof@ufl.edu").await?.unwrap();
        db::add_member(
            &pool,
            &course.id,
            grad.id,
            CourseRole::Instructor,
            pedagog_store::JoinSource::Manual,
            db::Actor {
                id: p.id,
                admin: false,
            },
        )
        .await?
        .unwrap();
        let grad = login(&url, &outbox, "grad@ufl.edu").await;
        let refused = post(
            &format!("{url}/courses"),
            serde_json::json!({"id": "mine", "name": "x"}),
            Some(&grad),
        )
        .await;
        assert_eq!(refused.status(), 403);
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn admins_set_up_courses_and_see_them_all(pool: PgPool) -> sqlx::Result<()> {
        db::set_role(&pool, "admin@ufl.edu", GlobalRole::Admin).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let admin = login(&url, &outbox, "admin@ufl.edu").await;
        for (id, name) in [
            ("cop3502-f25", "Programming 1"),
            ("cop3502-f26", "Programming 1"),
            ("cap4053-s26", "AI for Games"),
        ] {
            let r = post(
                &format!("{url}/courses"),
                serde_json::json!({"id": id, "name": name, "instructors": ["Prof@ufl.edu"]}),
                Some(&admin),
            )
            .await;
            assert_eq!(r.status(), 201);
        }
        let mine = json(get(&format!("{url}/courses"), &admin).await).await;
        assert_eq!(
            mine["courses"],
            serde_json::json!([]),
            "naming instructors leaves the admin out"
        );
        let all = json(get(&format!("{url}/courses?all=true&search=COP3502"), &admin).await).await;
        assert_eq!(
            (
                all["courses"].as_array().unwrap().len(),
                all["total"].as_i64(),
                all["pages"].as_i64()
            ),
            (2, Some(2), Some(1))
        );
        let shown = json(get(&format!("{url}/courses/cap4053-s26"), &admin).await).await;
        assert_eq!(shown["role"], "admin");

        let prof = login(&url, &outbox, "prof@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}/courses?all=true"), &prof)
                .await
                .status(),
            403
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn instructors_rename_courses_and_old_slugs_say_where_they_went(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let prof = db::upsert_user(&pool, "prof@ufl.edu", "").await?;
        db::create_course(&pool, "cop3502-f26", "COP3502", prof.id, &[prof.id]).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let token = login(&url, &outbox, "prof@ufl.edu").await;
        let patch = |course: &str, body: serde_json::Value| {
            let (url, token) = (url.clone(), token.clone());
            let course = course.to_owned();
            async move {
                reqwest::Client::new()
                    .patch(format!("{url}/courses/{course}"))
                    .bearer_auth(token)
                    .header("content-type", "application/json")
                    .body(body.to_string())
                    .send()
                    .await
                    .unwrap()
            }
        };
        let r = patch(
            "cop3502-f26",
            serde_json::json!({"id": "cop3502-fall26", "name": "Programming 1"}),
        )
        .await;
        assert_eq!(r.status(), 200);
        assert_eq!(json(r).await["id"], "cop3502-fall26");
        let shown = json(get(&format!("{url}/courses/cop3502-fall26"), &token).await).await;
        assert_eq!(
            (shown["name"].as_str(), shown["role"].as_str()),
            (Some("Programming 1"), Some("instructor"))
        );
        let gone = get(&format!("{url}/courses/cop3502-f26"), &token).await;
        assert_eq!(gone.status(), 404);
        assert!(
            json(gone).await["error"]
                .as_str()
                .unwrap()
                .contains("renamed to cop3502-fall26")
        );
        assert_eq!(
            patch("cop3502-fall26", serde_json::json!({"id": "Bad Id"}))
                .await
                .status(),
            400
        );
        assert_eq!(
            patch("cop3502-fall26", serde_json::json!({}))
                .await
                .status(),
            400
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn outsiders_and_students_are_kept_out(pool: PgPool) -> sqlx::Result<()> {
        let prof = db::upsert_user(&pool, "prof@ufl.edu", "").await?;
        let other = db::upsert_user(&pool, "other@ufl.edu", "").await?;
        let alice = db::upsert_user(&pool, "alice@ufl.edu", "").await?;
        db::create_course(&pool, "cop3502-f26", "COP3502", prof.id, &[prof.id]).await?;
        db::create_course(&pool, "cis4930-f26", "CIS4930", other.id, &[other.id]).await?;
        db::enroll(
            &pool,
            "cop3502-f26",
            alice.id,
            prof.id,
            pedagog_store::JoinSource::Import,
        )
        .await?;
        let (url, outbox) = serve(pool.clone()).await;

        let student = login(&url, &outbox, "alice@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}/courses/cop3502-f26"), &student)
                .await
                .status(),
            403
        );
        let mine = json(get(&format!("{url}/courses"), &student).await).await;
        assert_eq!(mine["courses"][0]["role"], "student");
        let other = login(&url, &outbox, "other@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}/courses/cop3502-f26"), &other)
                .await
                .status(),
            403,
            "instructor of another course"
        );
        assert_eq!(
            get(&format!("{url}/courses/nope"), &other).await.status(),
            404
        );
        Ok(())
    }
}
