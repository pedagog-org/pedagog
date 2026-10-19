//! Admins, courses, course staff and rosters.
//!
//! Admins manage admins, create courses and choose their instructors; an
//! instructor (or an admin) manages a course's roster. Admins count as staff
//! in every course.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pedagog_store::db::accounts as db;
use pedagog_store::{Course, Role, User};
use serde::{Deserialize, Serialize};

use crate::auth::CurrentUser;
use crate::error::{ApiError, internal, refused};
use crate::roster;
use crate::state::AppState;

fn forbidden(what: &str) -> ApiError {
    refused(StatusCode::FORBIDDEN, format!("only {what} can do that"))
}

pub fn require_admin(current: &CurrentUser) -> Result<(), ApiError> {
    if current.user.is_admin {
        Ok(())
    } else {
        Err(forbidden("admins"))
    }
}

/// The course, if the caller is its staff (or an admin); 404 if it doesn't
/// exist, 403 otherwise.
pub async fn require_staff(
    state: &AppState,
    course: &str,
    current: &CurrentUser,
) -> Result<Course, ApiError> {
    let found = db::course(&state.pool, course)
        .await
        .map_err(|e| internal("reading the course", e))?
        .ok_or_else(|| refused(StatusCode::NOT_FOUND, format!("no course {course}")))?;
    if current.user.is_admin {
        return Ok(found);
    }
    match db::role_in(&state.pool, course, current.user.id)
        .await
        .map_err(|e| internal("reading your role", e))?
    {
        Some(Role::Staff) => Ok(found),
        _ => Err(forbidden(&format!("{course}'s instructors"))),
    }
}

#[derive(Serialize)]
pub struct UserView {
    email: String,
    name: String,
    is_admin: bool,
}

impl From<User> for UserView {
    fn from(u: User) -> Self {
        Self {
            email: u.email,
            name: u.name,
            is_admin: u.is_admin,
        }
    }
}

#[derive(Deserialize)]
pub struct EmailBody {
    email: String,
}

pub async fn list_admins(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    let admins = db::admins(&state.pool)
        .await
        .map_err(|e| internal("reading admins", e))?;
    Ok(Json(admins.into_iter().map(UserView::from).collect::<Vec<_>>()).into_response())
}

pub async fn add_admin(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(body): Json<EmailBody>,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    let user = db::set_admin(&state.pool, &valid_email(&body.email)?, true)
        .await
        .map_err(|e| internal("adding the admin", e))?;
    tracing::info!(by = current.user.id, admin = user.id, "admin added");
    Ok((StatusCode::CREATED, Json(UserView::from(user))).into_response())
}

pub async fn remove_admin(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(email): Path<String>,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    let admins = db::admins(&state.pool)
        .await
        .map_err(|e| internal("reading admins", e))?;
    let email = email.trim().to_lowercase();
    if !admins.iter().any(|a| a.email == email) {
        return Err(refused(
            StatusCode::NOT_FOUND,
            format!("{email} isn't an admin"),
        ));
    }
    if admins.len() == 1 {
        return Err(refused(StatusCode::CONFLICT, "that's the last admin"));
    }
    db::set_admin(&state.pool, &email, false)
        .await
        .map_err(|e| internal("removing the admin", e))?;
    tracing::info!(by = current.user.id, %email, "admin removed");
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn valid_email(email: &str) -> Result<String, ApiError> {
    let email = email.trim().to_lowercase();
    match email.split_once('@') {
        Some((local, domain))
            if !local.is_empty() && domain.contains('.') && !email.contains(' ') =>
        {
            Ok(email)
        }
        _ => Err(refused(
            StatusCode::BAD_REQUEST,
            format!("{email:?} isn't an email address"),
        )),
    }
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

#[derive(Serialize)]
pub struct CourseView {
    id: String,
    name: String,
    /// The caller's role (`staff`, `student`), or `admin` for courses an admin
    /// doesn't teach.
    role: String,
}

pub async fn list_courses(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    let mine = db::courses_for(&state.pool, current.user.id)
        .await
        .map_err(|e| internal("reading your courses", e))?;
    let mut courses: Vec<CourseView> = mine
        .into_iter()
        .map(|(c, role)| CourseView {
            id: c.id,
            name: c.name,
            role: role.to_string(),
        })
        .collect();
    if current.user.is_admin {
        for c in db::all_courses(&state.pool)
            .await
            .map_err(|e| internal("reading courses", e))?
        {
            if !courses.iter().any(|m| m.id == c.id) {
                courses.push(CourseView {
                    id: c.id,
                    name: c.name,
                    role: "admin".to_owned(),
                });
            }
        }
        courses.sort_by(|a, b| a.id.cmp(&b.id));
    }
    Ok(Json(courses).into_response())
}

#[derive(Deserialize)]
pub struct NewCourse {
    id: String,
    name: String,
    #[serde(default)]
    instructors: Vec<String>,
}

pub async fn create_course(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(body): Json<NewCourse>,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    valid_course_id(&body.id)?;
    let instructors = body
        .instructors
        .iter()
        .map(|e| valid_email(e))
        .collect::<Result<Vec<_>, _>>()?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(refused(StatusCode::BAD_REQUEST, "a course needs a name"));
    }
    if !db::create_course(&state.pool, &body.id, name)
        .await
        .map_err(|e| internal("creating the course", e))?
    {
        return Err(refused(
            StatusCode::CONFLICT,
            format!("{} already exists", body.id),
        ));
    }
    for email in &instructors {
        let user = db::upsert_user(&state.pool, email, "")
            .await
            .map_err(|e| internal("adding an instructor", e))?;
        db::add_staff(&state.pool, &body.id, user.id)
            .await
            .map_err(|e| internal("adding an instructor", e))?;
    }
    tracing::info!(by = current.user.id, course = %body.id, "course created");
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": body.id, "name": name, "instructors": instructors })),
    )
        .into_response())
}

pub async fn show_course(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
) -> Result<Response, ApiError> {
    let found = require_staff(&state, &course, &current).await?;
    let staff = db::staff(&state.pool, &course)
        .await
        .map_err(|e| internal("reading the staff", e))?;
    let students = db::roster(&state.pool, &course)
        .await
        .map_err(|e| internal("reading the roster", e))?
        .len();
    Ok(Json(serde_json::json!({
        "id": found.id,
        "name": found.name,
        "instructors": staff.into_iter().map(UserView::from).collect::<Vec<_>>(),
        "students": students,
    }))
    .into_response())
}

pub async fn add_instructor(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, email)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    require_staff(&state, &course, &current).await?;
    let user = db::upsert_user(&state.pool, &valid_email(&email)?, "")
        .await
        .map_err(|e| internal("adding the instructor", e))?;
    db::add_staff(&state.pool, &course, user.id)
        .await
        .map_err(|e| internal("adding the instructor", e))?;
    Ok(Json(UserView::from(user)).into_response())
}

pub async fn remove_instructor(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, email)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    require_staff(&state, &course, &current).await?;
    let Some(user) = db::user_by_email(&state.pool, &email)
        .await
        .map_err(|e| internal("reading the user", e))?
    else {
        return Err(refused(
            StatusCode::NOT_FOUND,
            format!("{email} isn't an instructor of {course}"),
        ));
    };
    if !db::remove_staff(&state.pool, &course, user.id)
        .await
        .map_err(|e| internal("removing the instructor", e))?
    {
        return Err(refused(
            StatusCode::NOT_FOUND,
            format!("{email} isn't an instructor of {course}"),
        ));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Serialize)]
pub struct Imported {
    added: usize,
    already: usize,
    skipped: Vec<Skipped>,
    students: usize,
}

#[derive(Serialize)]
pub struct Skipped {
    line: usize,
    why: String,
}

/// The body is the CSV itself (simple or Canvas). Adds new students and
/// re-enrolls dropped ones; nobody is removed.
pub async fn import_roster(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_staff(&state, &course, &current).await?;
    let text = std::str::from_utf8(&body)
        .map_err(|_| refused(StatusCode::BAD_REQUEST, "the roster isn't UTF-8 text"))?;
    let parsed = roster::parse(text, &state.config.login_domain)
        .map_err(|e| refused(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let (mut added, mut already) = (0, 0);
    for entry in &parsed.entries {
        let user = db::upsert_user(&state.pool, &entry.email, &entry.name)
            .await
            .map_err(|e| internal("adding a student", e))?;
        if db::enroll(&state.pool, &course, user.id)
            .await
            .map_err(|e| internal("enrolling a student", e))?
        {
            added += 1;
        } else {
            already += 1;
        }
    }
    let students = db::roster(&state.pool, &course)
        .await
        .map_err(|e| internal("reading the roster", e))?
        .len();
    tracing::info!(by = current.user.id, %course, added, already, skipped = parsed.skipped.len(), "roster imported");
    Ok(Json(Imported {
        added,
        already,
        skipped: parsed
            .skipped
            .into_iter()
            .map(|(line, why)| Skipped { line, why })
            .collect(),
        students,
    })
    .into_response())
}

pub async fn list_roster(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
) -> Result<Response, ApiError> {
    require_staff(&state, &course, &current).await?;
    let students = db::roster(&state.pool, &course)
        .await
        .map_err(|e| internal("reading the roster", e))?;
    Ok(Json(students.into_iter().map(UserView::from).collect::<Vec<_>>()).into_response())
}

pub async fn drop_student(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, email)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    require_staff(&state, &course, &current).await?;
    let user = db::user_by_email(&state.pool, &email)
        .await
        .map_err(|e| internal("reading the student", e))?;
    let dropped = match user {
        Some(u) => db::drop_student(&state.pool, &course, u.id)
            .await
            .map_err(|e| internal("dropping the student", e))?,
        None => false,
    };
    if !dropped {
        return Err(refused(
            StatusCode::NOT_FOUND,
            format!("{email} isn't enrolled in {course}"),
        ));
    }
    tracing::info!(by = current.user.id, %course, %email, "student dropped");
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use crate::testing::{get, login, post, serve};
    use pedagog_store::db::accounts as db;

    async fn put(url: &str, token: &str) -> reqwest::Response {
        reqwest::Client::new()
            .put(url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    async fn delete(url: &str, token: &str) -> reqwest::Response {
        reqwest::Client::new()
            .delete(url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    async fn post_csv(url: &str, token: &str, csv: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(url)
            .bearer_auth(token)
            .header("content-type", "text/csv")
            .body(csv.to_owned())
            .send()
            .await
            .unwrap()
    }

    async fn json(r: reqwest::Response) -> serde_json::Value {
        serde_json::from_str(&r.text().await.unwrap()).unwrap()
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn admins_create_courses_and_instructors_run_rosters(pool: PgPool) -> sqlx::Result<()> {
        db::set_admin(&pool, "admin@ufl.edu", true).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let admin = login(&url, &outbox, "admin@ufl.edu").await;

        let created = post(
            &format!("{url}/courses"),
            serde_json::json!({"id": "cop3502-f26", "name": "COP3502 (Fall 2026)", "instructors": ["Prof@ufl.edu"]}),
            Some(&admin),
        )
        .await;
        assert_eq!(created.status(), 201);
        let again = post(
            &format!("{url}/courses"),
            serde_json::json!({"id": "cop3502-f26", "name": "x"}),
            Some(&admin),
        )
        .await;
        assert_eq!(again.status(), 409);
        let bad = post(
            &format!("{url}/courses"),
            serde_json::json!({"id": "COP 3502", "name": "x"}),
            Some(&admin),
        )
        .await;
        assert_eq!(bad.status(), 400);

        let prof = login(&url, &outbox, "prof@ufl.edu").await;
        let mine = json(get(&format!("{url}/courses"), &prof).await).await;
        assert_eq!(mine[0]["role"], "staff");

        let csv = "email,name\nalice@ufl.edu,Alice\nbob@ufl.edu,Bob\nnope,X\n";
        let imported =
            json(post_csv(&format!("{url}/courses/cop3502-f26/roster"), &prof, csv).await).await;
        assert_eq!(
            (imported["added"].as_u64(), imported["students"].as_u64()),
            (Some(2), Some(2))
        );
        assert_eq!(imported["skipped"][0]["line"], 4);
        let again =
            json(post_csv(&format!("{url}/courses/cop3502-f26/roster"), &prof, csv).await).await;
        assert_eq!(
            (again["added"].as_u64(), again["already"].as_u64()),
            (Some(0), Some(2))
        );

        let shown = json(get(&format!("{url}/courses/cop3502-f26"), &prof).await).await;
        assert_eq!(shown["students"], 2);
        assert_eq!(shown["instructors"][0]["email"], "prof@ufl.edu");

        assert_eq!(
            delete(
                &format!("{url}/courses/cop3502-f26/students/bob@ufl.edu"),
                &prof
            )
            .await
            .status(),
            204
        );
        assert_eq!(
            delete(
                &format!("{url}/courses/cop3502-f26/students/bob@ufl.edu"),
                &prof
            )
            .await
            .status(),
            404
        );
        let roster = json(get(&format!("{url}/courses/cop3502-f26/roster"), &prof).await).await;
        assert_eq!(roster.as_array().unwrap().len(), 1);
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn students_and_other_instructors_are_kept_out(pool: PgPool) -> sqlx::Result<()> {
        db::set_admin(&pool, "admin@ufl.edu", true).await?;
        db::create_course(&pool, "cop3502-f26", "COP3502").await?;
        db::create_course(&pool, "cis4930-f26", "CIS4930").await?;
        let other = db::upsert_user(&pool, "other@ufl.edu", "").await?;
        db::add_staff(&pool, "cis4930-f26", other.id).await?;
        let alice = db::upsert_user(&pool, "alice@ufl.edu", "").await?;
        db::enroll(&pool, "cop3502-f26", alice.id).await?;
        let (url, outbox) = serve(pool.clone()).await;

        let student = login(&url, &outbox, "alice@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}/courses/cop3502-f26/roster"), &student)
                .await
                .status(),
            403
        );
        assert_eq!(
            post(
                &format!("{url}/courses"),
                serde_json::json!({"id": "x", "name": "x"}),
                Some(&student)
            )
            .await
            .status(),
            403
        );
        assert_eq!(get(&format!("{url}/admins"), &student).await.status(), 403);

        let other = login(&url, &outbox, "other@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}/courses/cop3502-f26/roster"), &other)
                .await
                .status(),
            403,
            "staff of another course"
        );
        assert_eq!(
            put(
                &format!("{url}/courses/cop3502-f26/instructors/other@ufl.edu"),
                &other
            )
            .await
            .status(),
            403
        );
        assert_eq!(
            get(&format!("{url}/courses/nope"), &other).await.status(),
            404
        );

        let admin = login(&url, &outbox, "admin@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}/courses/cop3502-f26/roster"), &admin)
                .await
                .status(),
            200,
            "admins see every course"
        );
        let all = json(get(&format!("{url}/courses"), &admin).await).await;
        assert_eq!(all.as_array().unwrap().len(), 2);
        assert_eq!(all[0]["role"], "admin");
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn the_last_admin_stays(pool: PgPool) -> sqlx::Result<()> {
        db::set_admin(&pool, "admin@ufl.edu", true).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let admin = login(&url, &outbox, "admin@ufl.edu").await;
        assert_eq!(
            delete(&format!("{url}/admins/admin@ufl.edu"), &admin)
                .await
                .status(),
            409
        );
        let added = post(
            &format!("{url}/admins"),
            serde_json::json!({"email": "second@ufl.edu"}),
            Some(&admin),
        )
        .await;
        assert_eq!(added.status(), 201);
        assert_eq!(
            delete(&format!("{url}/admins/admin@ufl.edu"), &admin)
                .await
                .status(),
            204
        );
        assert_eq!(
            get(&format!("{url}/admins"), &admin).await.status(),
            403,
            "no longer an admin"
        );
        Ok(())
    }
}
