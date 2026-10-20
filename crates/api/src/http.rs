//! The public HTTP surface: login, logout and who am I here; users, courses
//! and members in their modules.
//!
//! `POST /auth/start {email}` always answers the same, so it doesn't reveal
//! who's registered. `POST /auth/finish {email, code, client}` opens a
//! session: a cookie for the web, a token in the body for hammer.

use axum::Router;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum::response::{AppendHeaders, IntoResponse, Json, Response};
use axum::routing::{delete, get, post, put};
use pedagog_store::db::accounts as db;
use pedagog_store::{CourseRole, SessionKind};
use serde::{Deserialize, Serialize};

use crate::auth::{COOKIE_NAME, CurrentUser, Login, open_session};
use crate::error::{ApiError, internal, refused, unauthorized};
use crate::state::AppState;
use crate::users::UserView;
use crate::{assignments, courses, members, policy, submissions, users};

pub fn router(state: AppState) -> Router {
    let max_upload = state.config.max_upload_bytes;
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/auth/start", post(start))
        .route("/auth/finish", post(finish))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/users", get(users::list))
        .route("/users/{query}", get(users::show))
        .route("/users/{email}/role", put(users::set_role))
        .route("/users/{email}/sessions", delete(users::sign_out))
        .route("/courses", get(courses::list).post(courses::create))
        .route(
            "/courses/{course}",
            get(courses::show).patch(courses::rename),
        )
        .route(
            "/courses/{course}/assignments",
            get(assignments::list)
                .post(assignments::upload)
                .layer(DefaultBodyLimit::max(max_upload)),
        )
        .route(
            "/courses/{course}/assignments/{slug}",
            get(assignments::show),
        )
        .route(
            "/courses/{course}/assignments/{slug}/builds/{build}",
            get(assignments::build),
        )
        .route(
            "/courses/{course}/assignments/{slug}/submissions",
            get(submissions::list)
                .post(submissions::submit)
                .layer(DefaultBodyLimit::max(max_upload)),
        )
        .route("/courses/{course}/submissions/{id}", get(submissions::show))
        .route(
            "/courses/{course}/assignments/{slug}/dates",
            get(policy::show_dates)
                .put(policy::set_dates)
                .delete(policy::clear_dates),
        )
        .route(
            "/courses/{course}/assignments/{slug}/visibility",
            get(policy::show_visibility)
                .put(policy::set_visibility)
                .delete(policy::clear_visibility),
        )
        .route(
            "/courses/{course}/assignments/{slug}/release",
            post(policy::release).delete(policy::unrelease),
        )
        .route("/courses/{course}/members", get(members::list))
        .route("/courses/{course}/members/import", post(members::import))
        .route(
            "/courses/{course}/members/{query}",
            get(members::show).put(members::add).delete(members::remove),
        )
        .with_state(state)
}

#[derive(Deserialize)]
struct StartBody {
    email: String,
}

async fn start(
    State(state): State<AppState>,
    Json(body): Json<StartBody>,
) -> Result<Response, ApiError> {
    let email = body.email.trim();
    if !email.contains('@') {
        return Err(refused(
            StatusCode::BAD_REQUEST,
            "that isn't an email address",
        ));
    }
    state
        .auth
        .start(&state, email)
        .await
        .map_err(|e| internal("sending a login code", e))?;
    let reply = serde_json::json!({
        "message": "If this email is registered, a login code is on its way."
    });
    Ok((StatusCode::ACCEPTED, Json(reply)).into_response())
}

#[derive(Deserialize)]
struct FinishBody {
    email: String,
    code: String,
    /// `web` (a cookie) or `cli` (a token in the reply).
    #[serde(default = "default_client")]
    client: String,
}

fn default_client() -> String {
    "web".to_owned()
}

async fn finish(
    State(state): State<AppState>,
    Json(body): Json<FinishBody>,
) -> Result<Response, ApiError> {
    let kind: SessionKind = body
        .client
        .parse()
        .map_err(|_| refused(StatusCode::BAD_REQUEST, "client must be web or cli"))?;
    let login = state
        .auth
        .finish(&state, body.email.trim(), &body.code)
        .await
        .map_err(|e| internal("checking a login code", e))?;
    let user = match login {
        Login::Ok(user) => user,
        Login::Wrong { left } => {
            return Err(unauthorized(format!(
                "wrong code; {left} {} left",
                if left == 1 { "try" } else { "tries" }
            )));
        }
        Login::Void => {
            return Err(unauthorized(
                "no valid code for this email: ask for a new one",
            ));
        }
    };
    let token = open_session(&state, &user, kind)
        .await
        .map_err(|e| internal("opening a session", e))?;
    tracing::info!(user = user.id, %kind, "logged in");
    let me = UserView::from(&user);
    Ok(match kind {
        SessionKind::Cli => Json(serde_json::json!({ "token": token, "user": me })).into_response(),
        SessionKind::Web => {
            let secure = if state.config.cookie_secure {
                "; Secure"
            } else {
                ""
            };
            let max_age = state.config.limits.web_session.as_secs();
            let cookie = format!(
                "{COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}"
            );
            (
                AppendHeaders([(SET_COOKIE, cookie)]),
                Json(serde_json::json!({ "user": me })),
            )
                .into_response()
        }
    })
}

async fn logout(State(state): State<AppState>, current: CurrentUser) -> Result<Response, ApiError> {
    db::delete_session(&state.pool, &current.token_hash)
        .await
        .map_err(|e| internal("ending the session", e))?;
    let clear = format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    Ok((StatusCode::NO_CONTENT, AppendHeaders([(SET_COOKIE, clear)])).into_response())
}

#[derive(Serialize)]
struct CourseView {
    id: String,
    name: String,
    /// `instructor`, `grader` or `student`.
    role: String,
}

async fn me(State(state): State<AppState>, current: CurrentUser) -> Result<Response, ApiError> {
    let courses = db::courses_for(&state.pool, current.user.id)
        .await
        .map_err(|e| internal("reading your courses", e))?
        .into_iter()
        .map(|(c, role): (_, CourseRole)| CourseView {
            id: c.id,
            name: c.name,
            role: role.to_string(),
        })
        .collect::<Vec<_>>();
    Ok(
        Json(serde_json::json!({ "user": UserView::from(&current.user), "courses": courses }))
            .into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use sqlx::PgPool;

    use crate::testing::{code_in, post, serve};

    #[sqlx::test(migrations = "../store/migrations")]
    async fn a_cli_login_gets_a_token_that_works_until_logout(pool: PgPool) -> sqlx::Result<()> {
        db::upsert_user(&pool, "alice@ufl.edu", "Alice").await?;
        let (url, outbox) = serve(pool).await;

        let started = post(
            &format!("{url}/auth/start"),
            serde_json::json!({"email": "Alice@ufl.edu"}),
            None,
        )
        .await;
        assert_eq!(started.status(), 202);
        let code = code_in(&outbox);
        assert_eq!(outbox.lock().unwrap()[0].to, "alice@ufl.edu");

        let wrong = post(
            &format!("{url}/auth/finish"),
            serde_json::json!({"email": "alice@ufl.edu", "code": "a-b-c", "client": "cli"}),
            None,
        )
        .await;
        assert_eq!(wrong.status(), 401);
        assert!(wrong.text().await.unwrap().contains("4 tries left"));

        let ok = post(
            &format!("{url}/auth/finish"),
            serde_json::json!({"email": "alice@ufl.edu", "code": code, "client": "cli"}),
            None,
        )
        .await;
        assert_eq!(ok.status(), 200);
        let body: serde_json::Value = serde_json::from_str(&ok.text().await.unwrap()).unwrap();
        let token = body["token"].as_str().unwrap().to_owned();

        let me = reqwest::Client::new()
            .get(format!("{url}/me"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(me.status(), 200);
        assert!(me.text().await.unwrap().contains("\"alice@ufl.edu\""));

        let out = post(
            &format!("{url}/auth/logout"),
            serde_json::json!({}),
            Some(&token),
        )
        .await;
        assert_eq!(out.status(), 204);
        let me = reqwest::Client::new()
            .get(format!("{url}/me"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(me.status(), 401);
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn unknown_emails_get_the_same_answer_and_no_mail(pool: PgPool) -> sqlx::Result<()> {
        let (url, outbox) = serve(pool).await;
        let r = post(
            &format!("{url}/auth/start"),
            serde_json::json!({"email": "nobody@ufl.edu"}),
            None,
        )
        .await;
        assert_eq!(r.status(), 202);
        assert!(outbox.lock().unwrap().is_empty());
        let r = post(
            &format!("{url}/auth/finish"),
            serde_json::json!({"email": "nobody@ufl.edu", "code": "a-b-c"}),
            None,
        )
        .await;
        assert_eq!(r.status(), 401);
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn a_web_login_sets_a_secure_http_only_cookie(pool: PgPool) -> sqlx::Result<()> {
        db::upsert_user(&pool, "bob@ufl.edu", "Bob").await?;
        let (url, outbox) = serve(pool).await;
        post(
            &format!("{url}/auth/start"),
            serde_json::json!({"email": "bob@ufl.edu"}),
            None,
        )
        .await;
        let code = code_in(&outbox);
        let r = post(
            &format!("{url}/auth/finish"),
            serde_json::json!({"email": "bob@ufl.edu", "code": code}),
            None,
        )
        .await;
        let cookie = r.headers()["set-cookie"].to_str().unwrap().to_owned();
        assert!(cookie.starts_with("pedagog_session="), "{cookie}");
        assert!(
            cookie.contains("HttpOnly")
                && cookie.contains("Secure")
                && cookie.contains("SameSite=Lax"),
            "{cookie}"
        );
        assert!(
            !r.text().await.unwrap().contains("token"),
            "no token in the body for the web"
        );

        let session = cookie.split(';').next().unwrap();
        let me = reqwest::Client::new()
            .get(format!("{url}/me"))
            .header("cookie", session)
            .send()
            .await
            .unwrap();
        assert_eq!(me.status(), 200);
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn me_lists_courses_and_requires_a_session(pool: PgPool) -> sqlx::Result<()> {
        let alice = db::upsert_user(&pool, "alice@ufl.edu", "Alice").await?;
        let prof = db::upsert_user(&pool, "prof@ufl.edu", "").await?;
        db::create_course(&pool, "cop3502-f26", "COP3502", prof.id, &[prof.id]).await?;
        db::enroll(
            &pool,
            "cop3502-f26",
            alice.id,
            prof.id,
            pedagog_store::JoinSource::Import,
        )
        .await?;
        let (url, outbox) = serve(pool).await;
        let me = reqwest::Client::new()
            .get(format!("{url}/me"))
            .send()
            .await
            .unwrap();
        assert_eq!(me.status(), 401);

        post(
            &format!("{url}/auth/start"),
            serde_json::json!({"email": "alice@ufl.edu"}),
            None,
        )
        .await;
        let code = code_in(&outbox);
        let ok = post(
            &format!("{url}/auth/finish"),
            serde_json::json!({"email": "alice@ufl.edu", "code": code, "client": "cli"}),
            None,
        )
        .await;
        let body: serde_json::Value = serde_json::from_str(&ok.text().await.unwrap()).unwrap();
        let me = reqwest::Client::new()
            .get(format!("{url}/me"))
            .bearer_auth(body["token"].as_str().unwrap())
            .send()
            .await
            .unwrap();
        let me: serde_json::Value = serde_json::from_str(&me.text().await.unwrap()).unwrap();
        assert_eq!(me["courses"][0]["id"], "cop3502-f26");
        assert_eq!(me["courses"][0]["role"], "student");
        Ok(())
    }
}
