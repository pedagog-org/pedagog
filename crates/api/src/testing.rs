//! Test harness: a real api on a free port, mail kept in memory.

use std::sync::{Arc, Mutex};

use sqlx::PgPool;

use crate::auth::Authenticator;
use crate::config::{AuthLimits, Config, Smtp, TlsMode};
use crate::mail::{Mail, Mailer};
use crate::state::AppState;

/// What the stand-in jobs was asked: `(path and query, body length)`.
pub type JobsSeen = Arc<Mutex<Vec<(String, usize)>>>;

/// A stand-in for jobs' upload endpoint: records the request and answers as
/// jobs would.
async fn stub_jobs() -> (String, JobsSeen) {
    use axum::extract::{Path, RawQuery};
    let seen: JobsSeen = Arc::default();
    let recorded = seen.clone();
    let submissions = seen.clone();
    let app = axum::Router::new()
        .route(
            "/internal/courses/{course}/assignments",
            axum::routing::post(
                move |Path(course): Path<String>,
                      RawQuery(q): RawQuery,
                      body: axum::body::Bytes| {
                    let recorded = recorded.clone();
                    async move {
                        let path = format!("/internal/courses/{course}/assignments");
                        let full = q.map(|q| format!("{path}?{q}")).unwrap_or(path);
                        recorded.lock().unwrap().push((full, body.len()));
                        (
                            axum::http::StatusCode::CREATED,
                            axum::Json(serde_json::json!({
                                "course": course, "assignment": "linked-list",
                                "build": {"status": "dispatched", "build_id": 1}, "warnings": []
                            })),
                        )
                    }
                },
            ),
        )
        .route(
            "/internal/courses/{course}/assignments/{slug}/submissions",
            axum::routing::post(
                move |Path((course, slug)): Path<(String, String)>,
                      RawQuery(q): RawQuery,
                      body: axum::body::Bytes| {
                    let recorded = submissions.clone();
                    async move {
                        let path =
                            format!("/internal/courses/{course}/assignments/{slug}/submissions");
                        let full = q.map(|q| format!("{path}?{q}")).unwrap_or(path);
                        recorded.lock().unwrap().push((full, body.len()));
                        (
                            axum::http::StatusCode::CREATED,
                            axum::Json(serde_json::json!({
                                "submission_id": 1, "run_id": 1, "build_id": 1, "tree_hash": "t",
                                "queue_position": 1, "files": ["a.py"], "ignored": []
                            })),
                        )
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, seen)
}

/// A running api on a free port, with mail kept in memory.
pub async fn serve(pool: PgPool) -> (String, Arc<Mutex<Vec<Mail>>>) {
    let (url, outbox, _) = serve_full(pool).await;
    (url, outbox)
}

/// [`serve`], plus what the stand-in jobs was asked.
pub async fn serve_full(pool: PgPool) -> (String, Arc<Mutex<Vec<Mail>>>, JobsSeen) {
    let (jobs_url, seen) = stub_jobs().await;
    let (mailer, outbox) = Mailer::memory();
    let config = config(jobs_url);
    let state = AppState {
        pool,
        config: Arc::new(config),
        mailer,
        auth: Authenticator::EmailCode,
        jobs: reqwest::Client::new(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, crate::http::router(state)).await });
    (url, outbox, seen)
}

pub async fn post(url: &str, body: serde_json::Value, token: Option<&str>) -> reqwest::Response {
    let client = reqwest::Client::new();
    let mut req = client
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_string());
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    req.send().await.unwrap()
}

pub fn code_in(outbox: &Arc<Mutex<Vec<Mail>>>) -> String {
    let mail = outbox.lock().unwrap().last().cloned().unwrap();
    mail.subject.rsplit(' ').next().unwrap().to_owned()
}

pub async fn get(url: &str, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
}

/// Log `email` in (it must be a user) and return a CLI token.
pub async fn login(url: &str, outbox: &Arc<Mutex<Vec<Mail>>>, email: &str) -> String {
    post(
        &format!("{url}/auth/start"),
        serde_json::json!({ "email": email }),
        None,
    )
    .await;
    let code = code_in(outbox);
    let reply = post(
        &format!("{url}/auth/finish"),
        serde_json::json!({ "email": email, "code": code, "client": "cli" }),
        None,
    )
    .await;
    let body: serde_json::Value = serde_json::from_str(&reply.text().await.unwrap()).unwrap();
    body["token"].as_str().unwrap().to_owned()
}

pub async fn put_json(url: &str, body: serde_json::Value, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .put(url)
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
}

pub async fn delete(url: &str, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .delete(url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
}

pub async fn post_csv(url: &str, token: &str, csv: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .header("content-type", "text/csv")
        .body(csv.to_owned())
        .send()
        .await
        .unwrap()
}

pub async fn json(r: reqwest::Response) -> serde_json::Value {
    serde_json::from_str(&r.text().await.unwrap()).unwrap()
}

/// An app state over `pool` and its outbox, for calling handlers' helpers
/// directly.
pub fn state(pool: PgPool) -> (AppState, Arc<Mutex<Vec<Mail>>>) {
    let (mailer, outbox) = Mailer::memory();
    let state = AppState {
        pool,
        config: Arc::new(config(String::new())),
        mailer,
        auth: Authenticator::EmailCode,
        jobs: reqwest::Client::new(),
    };
    (state, outbox)
}

fn config(jobs_url: String) -> Config {
    Config {
        database_url: String::new(),
        listen_addr: String::new(),
        public_url: "https://pedagog.example".into(),
        cookie_secure: true,
        smtp: Smtp {
            host: "unused".into(),
            port: 25,
            tls: TlsMode::None,
            login: None,
            from_name: "Pedagog".into(),
            from_address: "pedagog@ufl.edu".into(),
            reply_to: None,
        },
        limits: AuthLimits::default(),
        login_domain: "ufl.edu".into(),
        jobs_url,
        max_upload_bytes: 1 << 20,
        mail_per_second: 1000,
    }
}
