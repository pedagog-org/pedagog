//! Users and their global roles (admins only): list, show, set a role, sign
//! out everywhere.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pedagog_store::db::accounts::{self as db, RoleSet};
use pedagog_store::{GlobalRole, User};
use serde::{Deserialize, Serialize};

use crate::auth::CurrentUser;
use crate::error::{ApiError, internal, refused};
use crate::state::AppState;

/// Rows per page in long listings.
pub const PAGE: i64 = 50;
/// How many candidates a search in `show` lists before asking to narrow it.
pub const CANDIDATES: i64 = 20;

pub fn forbidden(who: &str) -> ApiError {
    refused(StatusCode::FORBIDDEN, format!("only {who} can do that"))
}

pub fn require_admin(current: &CurrentUser) -> Result<(), ApiError> {
    if current.user.is_admin() {
        Ok(())
    } else {
        Err(forbidden("admins"))
    }
}

pub fn valid_email(email: &str) -> Result<String, ApiError> {
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

pub fn parse_role<T: std::str::FromStr>(role: &str, expected: &str) -> Result<T, ApiError> {
    role.trim().to_lowercase().parse().map_err(|_| {
        refused(
            StatusCode::BAD_REQUEST,
            format!("{role:?} isn't a role: expected {expected}"),
        )
    })
}

/// The 1-based page asked for, as (limit, offset, page).
pub fn page(asked: Option<i64>) -> (i64, i64, i64) {
    let page = asked.unwrap_or(1).max(1);
    (PAGE, (page - 1) * PAGE, page)
}

pub fn pages(total: i64) -> i64 {
    ((total + PAGE - 1) / PAGE).max(1)
}

/// What a search in `show` found.
#[derive(Debug, PartialEq, Eq)]
pub enum Pick<T> {
    One(T),
    /// Up to [`CANDIDATES`], and whether there are more.
    Many(Vec<T>, bool),
    None,
}

/// `found` holds up to `CANDIDATES + 1` matches, an exact email first.
pub fn pick<T>(mut found: Vec<T>, exact: impl Fn(&T) -> bool) -> Pick<T> {
    if found.first().is_some_and(&exact) || found.len() == 1 {
        return Pick::One(found.swap_remove(0));
    }
    if found.is_empty() {
        return Pick::None;
    }
    let more = found.len() > CANDIDATES as usize;
    found.truncate(CANDIDATES as usize);
    Pick::Many(found, more)
}

#[derive(Serialize)]
pub struct UserView {
    pub email: String,
    pub name: String,
    pub role: String,
}

impl From<&User> for UserView {
    fn from(u: &User) -> Self {
        Self {
            email: u.email.clone(),
            name: u.name.clone(),
            role: u.role.to_string(),
        }
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    role: Option<String>,
    search: Option<String>,
    page: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    current: CurrentUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    let role: Option<GlobalRole> = q
        .role
        .as_deref()
        .map(|r| parse_role(r, "admin, staff or student"))
        .transpose()?;
    let (limit, offset, page) = page(q.page);
    let (users, total) = db::find_users(&state.pool, role, q.search.as_deref(), limit, offset)
        .await
        .map_err(|e| internal("reading users", e))?;
    Ok(Json(serde_json::json!({
        "users": users.iter().map(UserView::from).collect::<Vec<_>>(),
        "total": total,
        "page": page,
        "pages": pages(total),
    }))
    .into_response())
}

/// One user's profile: role, logins, sessions and every course (dropped ones
/// too). The path is part of an email or name.
pub async fn show(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(query): Path<String>,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    let (found, _) = db::find_users(&state.pool, None, Some(&query), CANDIDATES + 1, 0)
        .await
        .map_err(|e| internal("finding users", e))?;
    let wanted = query.trim().to_lowercase();
    let user = match pick(found, |u| u.email == wanted) {
        Pick::One(u) => u,
        Pick::Many(us, more) => {
            return Ok(Json(serde_json::json!({
                "matches": us.iter().map(UserView::from).collect::<Vec<_>>(),
                "more": more,
            }))
            .into_response());
        }
        Pick::None => {
            return Err(refused(
                StatusCode::NOT_FOUND,
                format!("no user matches {query:?}"),
            ));
        }
    };
    let activity = db::activity(&state.pool, user.id)
        .await
        .map_err(|e| internal("reading the user's activity", e))?;
    let courses = db::memberships_of(&state.pool, user.id)
        .await
        .map_err(|e| internal("reading the user's courses", e))?;
    Ok(Json(serde_json::json!({
        "user": {
            "email": user.email,
            "name": user.name,
            "role": user.role.to_string(),
            "created_at": activity.created_at,
            "last_login_at": activity.last_login_at,
            "last_login_kind": activity.last_login_kind.map(|k| k.to_string()),
            "sessions": {"web": activity.web_sessions, "cli": activity.cli_sessions},
            "courses": courses.iter().map(|m| serde_json::json!({
                "id": m.course.id,
                "name": m.course.name,
                "role": m.role.to_string(),
                "joined_at": m.joined_at,
                "dropped_at": m.dropped_at,
            })).collect::<Vec<_>>(),
        }
    }))
    .into_response())
}

#[derive(Deserialize)]
pub struct RoleBody {
    role: String,
}

/// Set a global role (creating the user if needed); the last admin stays.
pub async fn set_role(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(email): Path<String>,
    Json(body): Json<RoleBody>,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    let email = valid_email(&email)?;
    let role: GlobalRole = parse_role(&body.role, "admin, staff or student")?;
    match db::set_role(&state.pool, &email, role)
        .await
        .map_err(|e| internal("setting the role", e))?
    {
        RoleSet::LastAdmin => Err(refused(
            StatusCode::CONFLICT,
            format!("{email} is the last admin"),
        )),
        RoleSet::Set { user, from } => {
            tracing::info!(by = current.user.id, user = user.id, %role, "global role set");
            Ok(Json(serde_json::json!({
                "user": UserView::from(&user),
                "from": from.map(|r| r.to_string()),
            }))
            .into_response())
        }
    }
}

/// End every session a user has (a lost laptop, a shared machine).
pub async fn sign_out(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(email): Path<String>,
) -> Result<Response, ApiError> {
    require_admin(&current)?;
    let user = db::user_by_email(&state.pool, &email)
        .await
        .map_err(|e| internal("reading the user", e))?
        .ok_or_else(|| refused(StatusCode::NOT_FOUND, format!("no user {email}")))?;
    let ended = db::end_sessions(&state.pool, user.id)
        .await
        .map_err(|e| internal("ending sessions", e))?;
    tracing::info!(
        by = current.user.id,
        user = user.id,
        ended,
        "signed out everywhere"
    );
    Ok(Json(serde_json::json!({ "ended": ended })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    use sqlx::PgPool;

    use crate::testing::{delete, get, json, login, put_json, serve};

    #[test]
    fn a_search_picks_an_exact_email_or_a_single_match() {
        let exact = |s: &&str| *s == "a@ufl.edu";
        assert_eq!(
            pick(vec!["a@ufl.edu", "ba@ufl.edu"], exact),
            Pick::One("a@ufl.edu")
        );
        assert_eq!(pick(vec!["ba@ufl.edu"], exact), Pick::One("ba@ufl.edu"));
        assert_eq!(
            pick(vec!["ba@ufl.edu", "ca@ufl.edu"], exact),
            Pick::Many(vec!["ba@ufl.edu", "ca@ufl.edu"], false)
        );
        assert_eq!(pick(Vec::<&str>::new(), exact), Pick::None);
        let lots: Vec<&str> = vec!["x"; CANDIDATES as usize + 1];
        let Pick::Many(shown, more) = pick(lots, exact) else {
            panic!()
        };
        assert_eq!((shown.len(), more), (CANDIDATES as usize, true));
    }

    #[test]
    fn pages_count_up_from_one() {
        assert_eq!(page(None), (PAGE, 0, 1));
        assert_eq!(page(Some(0)), (PAGE, 0, 1));
        assert_eq!(page(Some(3)), (PAGE, 2 * PAGE, 3));
        assert_eq!((pages(0), pages(PAGE), pages(PAGE + 1)), (1, 1, 2));
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn admins_list_show_and_set_roles(pool: PgPool) -> sqlx::Result<()> {
        db::set_role(&pool, "admin@ufl.edu", GlobalRole::Admin).await?;
        db::upsert_user(&pool, "albert.gator@ufl.edu", "Albert Gator").await?;
        db::upsert_user(&pool, "alberta.gator@ufl.edu", "Alberta Gator").await?;
        let (url, outbox) = serve(pool.clone()).await;
        let admin = login(&url, &outbox, "admin@ufl.edu").await;

        let set = put_json(
            &format!("{url}/users/Prof@ufl.edu/role"),
            serde_json::json!({"role": "staff"}),
            &admin,
        )
        .await;
        assert_eq!(set.status(), 200);
        let set = json(set).await;
        assert_eq!(
            (set["user"]["role"].as_str(), set["from"].as_str()),
            (Some("staff"), None)
        );
        let staff = json(get(&format!("{url}/users?role=staff"), &admin).await).await;
        assert_eq!(staff["users"][0]["email"], "prof@ufl.edu");
        assert_eq!(
            (staff["total"].as_i64(), staff["pages"].as_i64()),
            (Some(1), Some(1))
        );

        let many = json(get(&format!("{url}/users/gator"), &admin).await).await;
        assert_eq!(many["matches"].as_array().unwrap().len(), 2);
        let one = json(get(&format!("{url}/users/albert%20gator"), &admin).await).await;
        assert_eq!(one["user"]["email"], "albert.gator@ufl.edu");
        assert_eq!(one["user"]["courses"], serde_json::json!([]));
        let me = json(get(&format!("{url}/users/admin@ufl.edu"), &admin).await).await;
        assert_eq!(
            (
                me["user"]["last_login_kind"].as_str(),
                me["user"]["sessions"]["cli"].as_i64()
            ),
            (Some("cli"), Some(1))
        );
        assert_eq!(
            get(&format!("{url}/users/nobody"), &admin).await.status(),
            404
        );

        let last = put_json(
            &format!("{url}/users/admin@ufl.edu/role"),
            serde_json::json!({"role": "student"}),
            &admin,
        )
        .await;
        assert_eq!(last.status(), 409);
        let bad = put_json(
            &format!("{url}/users/prof@ufl.edu/role"),
            serde_json::json!({"role": "wizard"}),
            &admin,
        )
        .await;
        assert_eq!(bad.status(), 400);
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn signing_out_ends_every_session(pool: PgPool) -> sqlx::Result<()> {
        db::set_role(&pool, "admin@ufl.edu", GlobalRole::Admin).await?;
        db::upsert_user(&pool, "alice@ufl.edu", "").await?;
        let (url, outbox) = serve(pool.clone()).await;
        let admin = login(&url, &outbox, "admin@ufl.edu").await;
        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}/users?role=staff"), &alice)
                .await
                .status(),
            403
        );
        let ended =
            json(delete(&format!("{url}/users/alice@ufl.edu/sessions"), &admin).await).await;
        assert_eq!(ended["ended"], 1);
        assert_eq!(get(&format!("{url}/me"), &alice).await.status(), 401);
        Ok(())
    }
}
