//! A course's members: list, show, add (or change a role), drop, and import
//! students from a roster file.
//!
//! Instructors (and admins) change members; graders only look, and may drop
//! themselves. Removing or demoting another instructor takes an admin, and a
//! course keeps at least one instructor. Drops are soft.

use std::collections::HashSet;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pedagog_store::db::accounts::{self as db, Actor, Enrolled, Joined, Refused};
use pedagog_store::{CourseRole, JoinSource, Membership};
use serde::{Deserialize, Serialize};

use crate::auth::CurrentUser;
use crate::courses::{Need, require};
use crate::error::{ApiError, internal, refused};
use crate::roster;
use crate::state::AppState;
use crate::users::{CANDIDATES, Pick, forbidden, parse_role, pick, valid_email};

const ROLES: &str = "instructor, grader or student";

#[derive(Serialize)]
pub struct MemberView {
    email: String,
    name: String,
    role: String,
    source: String,
    added_by: Option<String>,
    joined_at: String,
    dropped_at: Option<String>,
}

impl From<&Membership> for MemberView {
    fn from(m: &Membership) -> Self {
        Self {
            email: m.user.email.clone(),
            name: m.user.name.clone(),
            role: m.role.to_string(),
            source: m.source.to_string(),
            added_by: m.added_by.clone(),
            joined_at: m.joined_at.clone(),
            dropped_at: m.dropped_at.clone(),
        }
    }
}

fn actor(current: &CurrentUser) -> Actor {
    Actor {
        id: current.user.id,
        admin: current.user.is_admin(),
    }
}

fn refusal(r: Refused, email: &str, course: &str) -> ApiError {
    match r {
        Refused::LastInstructor => refused(
            StatusCode::CONFLICT,
            format!("{email} is the last instructor of {course}"),
        ),
        Refused::OtherInstructor => refused(
            StatusCode::FORBIDDEN,
            "only an admin can remove or demote another instructor",
        ),
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    role: Option<String>,
    #[serde(default)]
    dropped: bool,
}

pub async fn list(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    require(&state, &course, &current, Need::See).await?;
    let role: Option<CourseRole> = q
        .role
        .as_deref()
        .map(|r| parse_role(r, ROLES))
        .transpose()?;
    let members = db::members(&state.pool, &course, role, q.dropped)
        .await
        .map_err(|e| internal("reading the members", e))?;
    Ok(Json(members.iter().map(MemberView::from).collect::<Vec<_>>()).into_response())
}

/// One member, found by part of their email or name.
pub async fn show(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, query)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    require(&state, &course, &current, Need::See).await?;
    let found = db::find_members(&state.pool, &course, &query, CANDIDATES + 1)
        .await
        .map_err(|e| internal("finding members", e))?;
    let wanted = query.trim().to_lowercase();
    match pick(found, |m| m.user.email == wanted) {
        Pick::One(m) => {
            Ok(Json(serde_json::json!({ "member": MemberView::from(&m) })).into_response())
        }
        Pick::Many(ms, more) => Ok(Json(serde_json::json!({
            "matches": ms.iter().map(MemberView::from).collect::<Vec<_>>(),
            "more": more,
        }))
        .into_response()),
        Pick::None => Err(refused(
            StatusCode::NOT_FOUND,
            format!("no member of {course} matches {query:?}"),
        )),
    }
}

#[derive(Deserialize)]
pub struct AddBody {
    role: Option<String>,
    #[serde(default)]
    name: String,
}

/// Add someone (creating the user if needed), restore them if dropped, or
/// change their role.
pub async fn add(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, email)): Path<(String, String)>,
    Json(body): Json<AddBody>,
) -> Result<Response, ApiError> {
    require(&state, &course, &current, Need::Manage).await?;
    let email = valid_email(&email)?;
    let role = match body.role.as_deref() {
        Some(r) => parse_role(r, ROLES)?,
        None => CourseRole::Student,
    };
    let user = db::upsert_user(&state.pool, &email, &body.name)
        .await
        .map_err(|e| internal("adding the user", e))?;
    let joined = db::add_member(
        &state.pool,
        &course,
        user.id,
        role,
        JoinSource::Manual,
        actor(&current),
    )
    .await
    .map_err(|e| internal("adding the member", e))?
    .map_err(|r| refusal(r, &email, &course))?;
    let (change, from) = match joined {
        Joined::New => ("added", None),
        Joined::Restored => ("restored", None),
        Joined::Changed { from } => ("changed", Some(from.to_string())),
        Joined::Unchanged => ("unchanged", None),
    };
    tracing::info!(by = current.user.id, %course, user = user.id, %role, change, "member added");
    Ok(Json(serde_json::json!({ "email": email, "role": role.to_string(), "change": change, "from": from }))
        .into_response())
}

/// Drop a member: instructors drop anyone (the instructor rules apply);
/// graders may drop themselves.
pub async fn remove(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, email)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::See).await?;
    let email = email.trim().to_lowercase();
    if !access.manages() && email != current.user.email {
        return Err(forbidden(&format!("{course}'s instructors")));
    }
    let target = db::user_by_email(&state.pool, &email)
        .await
        .map_err(|e| internal("reading the member", e))?;
    let dropped = match target {
        Some(u) => db::drop_member(&state.pool, &course, u.id, actor(&current))
            .await
            .map_err(|e| internal("dropping the member", e))?
            .map_err(|r| refusal(r, &email, &course))?,
        None => None,
    };
    let Some(role) = dropped else {
        return Err(refused(
            StatusCode::NOT_FOUND,
            format!("{email} isn't an active member of {course}"),
        ));
    };
    tracing::info!(by = current.user.id, %course, %email, %role, "member dropped");
    Ok(Json(serde_json::json!({ "email": email, "role": role.to_string() })).into_response())
}

#[derive(Deserialize)]
pub struct ImportQuery {
    /// Also drop students missing from the file.
    #[serde(default)]
    sync: bool,
    /// Change nothing; say who a sync would drop.
    #[serde(default)]
    dry_run: bool,
}

#[derive(Serialize)]
struct Skipped {
    line: usize,
    why: String,
}

/// The body is the CSV itself (simple or Canvas). Enrolls students; staff
/// in the file keep their role, and a sync never drops them.
pub async fn import(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(course): Path<String>,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require(&state, &course, &current, Need::Manage).await?;
    let text = std::str::from_utf8(&body)
        .map_err(|_| refused(StatusCode::BAD_REQUEST, "the roster isn't UTF-8 text"))?;
    let parsed = roster::parse(text, &state.config.login_domain)
        .map_err(|e| refused(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let skipped: Vec<Skipped> = parsed
        .skipped
        .iter()
        .map(|(line, why)| Skipped {
            line: *line,
            why: why.clone(),
        })
        .collect();
    let listed: HashSet<&str> = parsed.entries.iter().map(|e| e.email.as_str()).collect();
    let students = db::members(&state.pool, &course, Some(CourseRole::Student), false)
        .await
        .map_err(|e| internal("reading the roster", e))?;
    let missing: Vec<&Membership> = if q.sync {
        students
            .iter()
            .filter(|m| !listed.contains(m.user.email.as_str()))
            .collect()
    } else {
        Vec::new()
    };
    let missing_emails: Vec<&str> = missing.iter().map(|m| m.user.email.as_str()).collect();
    if q.dry_run {
        return Ok(Json(serde_json::json!({
            "dry_run": true,
            "listed": parsed.entries.len(),
            "would_drop": missing_emails,
            "skipped": skipped,
        }))
        .into_response());
    }

    let (mut added, mut restored, mut already, mut staff) = (0, 0, 0, Vec::new());
    for entry in &parsed.entries {
        let user = db::upsert_user(&state.pool, &entry.email, &entry.name)
            .await
            .map_err(|e| internal("adding a student", e))?;
        match db::enroll(
            &state.pool,
            &course,
            user.id,
            current.user.id,
            JoinSource::Import,
        )
        .await
        .map_err(|e| internal("enrolling a student", e))?
        {
            Enrolled::New => added += 1,
            Enrolled::Restored => restored += 1,
            Enrolled::Already => already += 1,
            Enrolled::Staff => staff.push(entry.email.clone()),
        }
    }
    let ids: Vec<i64> = missing.iter().map(|m| m.user.id).collect();
    db::drop_students(&state.pool, &course, &ids)
        .await
        .map_err(|e| internal("dropping students", e))?;
    let now = db::members(&state.pool, &course, Some(CourseRole::Student), false)
        .await
        .map_err(|e| internal("reading the roster", e))?
        .len();
    tracing::info!(
        by = current.user.id, %course, added, restored, already,
        dropped = ids.len(), skipped = skipped.len(), "roster imported"
    );
    Ok(Json(serde_json::json!({
        "added": added,
        "restored": restored,
        "already": already,
        "staff": staff,
        "dropped": missing_emails,
        "skipped": skipped,
        "students": now,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use pedagog_store::GlobalRole;
    use sqlx::PgPool;

    use super::*;
    use crate::testing::{delete, get, json, login, post_csv, put_json, serve};

    /// `cop3502-f26` taught by prof@ufl.edu; returns the api url and prof's token.
    async fn course(
        pool: &PgPool,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<crate::mail::Mail>>>,
        String,
    ) {
        let prof = db::upsert_user(pool, "prof@ufl.edu", "Prof").await.unwrap();
        db::create_course(pool, "cop3502-f26", "Programming 1", prof.id, &[prof.id])
            .await
            .unwrap();
        let (url, outbox) = serve(pool.clone()).await;
        let token = login(&url, &outbox, "prof@ufl.edu").await;
        (url, outbox, token)
    }

    async fn add(url: &str, token: &str, email: &str, role: &str) -> reqwest::Response {
        put_json(
            &format!("{url}/courses/cop3502-f26/members/{email}"),
            serde_json::json!({ "role": role }),
            token,
        )
        .await
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn instructors_add_change_and_drop_members(pool: PgPool) -> sqlx::Result<()> {
        let (url, outbox, prof) = course(&pool).await;
        let r = json(add(&url, &prof, "Alice@ufl.edu", "student").await).await;
        assert_eq!(
            (r["change"].as_str(), r["email"].as_str()),
            (Some("added"), Some("alice@ufl.edu"))
        );
        let r = json(add(&url, &prof, "ta@ufl.edu", "student").await).await;
        assert_eq!(r["change"], "added");
        let r = json(add(&url, &prof, "ta@ufl.edu", "grader").await).await;
        assert_eq!(
            (r["change"].as_str(), r["from"].as_str()),
            (Some("changed"), Some("student"))
        );
        assert_eq!(add(&url, &prof, "x@ufl.edu", "dean").await.status(), 400);

        let graders = json(
            get(
                &format!("{url}/courses/cop3502-f26/members?role=grader"),
                &prof,
            )
            .await,
        )
        .await;
        assert_eq!(graders[0]["email"], "ta@ufl.edu");
        assert_eq!(graders[0]["added_by"], "prof@ufl.edu");

        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}/courses/cop3502-f26/members"), &ta)
                .await
                .status(),
            200,
            "graders look"
        );
        assert_eq!(add(&url, &ta, "bob@ufl.edu", "student").await.status(), 403);
        assert_eq!(
            delete(
                &format!("{url}/courses/cop3502-f26/members/alice@ufl.edu"),
                &ta
            )
            .await
            .status(),
            403
        );

        let dropped = delete(
            &format!("{url}/courses/cop3502-f26/members/alice@ufl.edu"),
            &prof,
        )
        .await;
        assert_eq!(dropped.status(), 200);
        assert_eq!(
            delete(
                &format!("{url}/courses/cop3502-f26/members/alice@ufl.edu"),
                &prof
            )
            .await
            .status(),
            404
        );
        let active = json(get(&format!("{url}/courses/cop3502-f26/members"), &prof).await).await;
        assert_eq!(active.as_array().unwrap().len(), 2);
        let all = json(
            get(
                &format!("{url}/courses/cop3502-f26/members?dropped=true"),
                &prof,
            )
            .await,
        )
        .await;
        assert_eq!(all.as_array().unwrap().len(), 3);
        let r = json(add(&url, &prof, "alice@ufl.edu", "student").await).await;
        assert_eq!(r["change"], "restored");

        assert_eq!(
            delete(
                &format!("{url}/courses/cop3502-f26/members/ta@ufl.edu"),
                &ta
            )
            .await
            .status(),
            200,
            "graders may leave"
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn only_admins_unseat_other_instructors(pool: PgPool) -> sqlx::Result<()> {
        db::set_role(&pool, "admin@ufl.edu", GlobalRole::Admin).await?;
        let (url, outbox, prof) = course(&pool).await;
        assert_eq!(
            delete(
                &format!("{url}/courses/cop3502-f26/members/prof@ufl.edu"),
                &prof
            )
            .await
            .status(),
            409,
            "the last instructor"
        );
        add(&url, &prof, "grad@ufl.edu", "instructor").await;
        let grad = login(&url, &outbox, "grad@ufl.edu").await;
        assert_eq!(
            delete(
                &format!("{url}/courses/cop3502-f26/members/prof@ufl.edu"),
                &grad
            )
            .await
            .status(),
            403
        );
        assert_eq!(
            add(&url, &grad, "prof@ufl.edu", "grader").await.status(),
            403,
            "no demoting"
        );
        let admin = login(&url, &outbox, "admin@ufl.edu").await;
        assert_eq!(
            delete(
                &format!("{url}/courses/cop3502-f26/members/grad@ufl.edu"),
                &admin
            )
            .await
            .status(),
            200
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn show_finds_members_by_part_of_a_name(pool: PgPool) -> sqlx::Result<()> {
        let (url, _, prof) = course(&pool).await;
        for (email, name) in [
            ("albert.gator@ufl.edu", "Albert Gator"),
            ("sswamp@ufl.edu", "Sally Swamp"),
        ] {
            put_json(
                &format!("{url}/courses/cop3502-f26/members/{email}"),
                serde_json::json!({ "name": name }),
                &prof,
            )
            .await;
        }
        let one = json(get(&format!("{url}/courses/cop3502-f26/members/sally"), &prof).await).await;
        assert_eq!(
            (
                one["member"]["email"].as_str(),
                one["member"]["source"].as_str()
            ),
            (Some("sswamp@ufl.edu"), Some("manual"))
        );
        let many =
            json(get(&format!("{url}/courses/cop3502-f26/members/ufl.edu"), &prof).await).await;
        assert_eq!(many["matches"].as_array().unwrap().len(), 3);
        assert_eq!(
            get(&format!("{url}/courses/cop3502-f26/members/zed"), &prof)
                .await
                .status(),
            404
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn imports_add_students_and_sync_drops_the_missing(pool: PgPool) -> sqlx::Result<()> {
        let (url, _, prof) = course(&pool).await;
        let import = |q: &'static str, csv: &'static str| {
            let (url, prof) = (url.clone(), prof.clone());
            async move {
                post_csv(
                    &format!("{url}/courses/cop3502-f26/members/import{q}"),
                    &prof,
                    csv,
                )
                .await
            }
        };
        add(&url, &prof, "ta@ufl.edu", "grader").await;
        let r = json(
            import(
                "",
                "email,name\nalice@ufl.edu,Alice\nbob@ufl.edu,Bob\nta@ufl.edu,TA\nnope,X\n",
            )
            .await,
        )
        .await;
        assert_eq!(
            (
                r["added"].as_i64(),
                r["students"].as_i64(),
                r["staff"][0].as_str()
            ),
            (Some(2), Some(2), Some("ta@ufl.edu"))
        );
        assert_eq!(r["skipped"][0]["line"], 5);

        let csv = "email\nalice@ufl.edu\ncarol@ufl.edu\n";
        let dry = json(import("?sync=true&dry_run=true", csv).await).await;
        assert_eq!(dry["would_drop"], serde_json::json!(["bob@ufl.edu"]));
        let still = json(
            get(
                &format!("{url}/courses/cop3502-f26/members?role=student"),
                &prof,
            )
            .await,
        )
        .await;
        assert_eq!(
            still.as_array().unwrap().len(),
            2,
            "a dry run changes nothing"
        );

        let r = json(import("?sync=true", csv).await).await;
        assert_eq!(
            (
                r["added"].as_i64(),
                r["already"].as_i64(),
                r["dropped"].clone(),
                r["students"].as_i64()
            ),
            (
                Some(1),
                Some(1),
                serde_json::json!(["bob@ufl.edu"]),
                Some(2)
            )
        );
        let graders = json(
            get(
                &format!("{url}/courses/cop3502-f26/members?role=grader"),
                &prof,
            )
            .await,
        )
        .await;
        assert_eq!(
            graders.as_array().unwrap().len(),
            1,
            "a sync leaves staff alone"
        );

        let back = json(import("", "email\nbob@ufl.edu\n").await).await;
        assert_eq!(back["restored"], 1);
        Ok(())
    }
}
