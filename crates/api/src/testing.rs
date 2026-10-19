//! Test harness: a real api on a free port, mail kept in memory.

use std::sync::{Arc, Mutex};

use sqlx::PgPool;

use crate::auth::Authenticator;
use crate::config::{AuthLimits, Config, Smtp, TlsMode};
use crate::mail::{Mail, Mailer};
use crate::state::AppState;

/// A running api on a free port, with mail kept in memory.
pub async fn serve(pool: PgPool) -> (String, Arc<Mutex<Vec<Mail>>>) {
    let (mailer, outbox) = Mailer::memory();
    let config = Config {
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
    };
    let state = AppState {
        pool,
        config: Arc::new(config),
        mailer,
        auth: Authenticator::EmailCode,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, crate::http::router(state)).await });
    (url, outbox)
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
