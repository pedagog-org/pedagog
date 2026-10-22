//! The api, from the browser: same-origin requests under `/api` with the
//! session cookie, `X-Pedagog: web` on every change (the api refuses changes
//! without it), and the api's own sentences for refusals. A 401 sends the
//! viewer to log in, back to where they were afterwards.

use gloo_net::http::{Request, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Why a request didn't give what was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The HTTP status, or 0 when the api couldn't be reached.
    pub status: u16,
    /// The api's sentence (`{"error": …}`), or ours.
    pub message: String,
}

impl Problem {
    pub fn unreachable() -> Self {
        Self {
            status: 0,
            message: "the server is unreachable; check your connection and try again".into(),
        }
    }

    pub fn is_unauthorized(&self) -> bool {
        self.status == 401
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The api's refusal sentence from a reply body, else a plain one.
pub fn message_of(status: u16, body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_owned))
        .unwrap_or_else(|| match status {
            404 => "not found".into(),
            500..=599 => "something went wrong on our side; try again".into(),
            _ => format!("the request failed ({status})"),
        })
}

/// `path` under `/api` (it starts with `/`).
pub fn url(path: &str) -> String {
    format!("/api{path}")
}

/// The page to come back to after logging in: `/login?next=<path>`.
pub fn login_url(next: &str) -> String {
    if next.is_empty() || next == "/" || next.starts_with("/login") {
        "/login".into()
    } else {
        let encoded: String = js_sys::encode_uri_component(next).into();
        format!("/login?next={encoded}")
    }
}

/// Where `/login?next=…` should go once logged in: only a path on this site.
pub fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.starts_with("/login") => {
            n.into()
        }
        _ => "/".into(),
    }
}

async fn finish<T: DeserializeOwned>(
    reply: Result<Response, gloo_net::Error>,
) -> Result<T, Problem> {
    let reply = reply.map_err(|_| Problem::unreachable())?;
    let status = reply.status();
    let body = reply.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        // 204s and empty bodies read as JSON null.
        let body = if body.is_empty() {
            "null"
        } else {
            body.as_str()
        };
        serde_json::from_str(body).map_err(|e| Problem {
            status,
            message: format!("an unexpected reply from the server ({e})"),
        })
    } else {
        let problem = Problem {
            status,
            message: message_of(status, &body),
        };
        if problem.is_unauthorized() {
            crate::session::expired();
        }
        Err(problem)
    }
}

fn change(builder: RequestBuilder) -> RequestBuilder {
    builder.header("X-Pedagog", "web")
}

pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, Problem> {
    finish(Request::get(&url(path)).send().await).await
}

/// A text reply (a build's log).
pub async fn get_text(path: &str) -> Result<String, Problem> {
    let reply = Request::get(&url(path))
        .send()
        .await
        .map_err(|_| Problem::unreachable())?;
    let status = reply.status();
    let body = reply.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(Problem {
            status,
            message: message_of(status, &body),
        })
    }
}

async fn send_json<T: DeserializeOwned>(
    builder: RequestBuilder,
    body: &impl Serialize,
) -> Result<T, Problem> {
    let request = change(builder)
        .json(body)
        .map_err(|_| Problem::unreachable())?;
    finish(request.send().await).await
}

pub async fn post<T: DeserializeOwned>(path: &str, body: &impl Serialize) -> Result<T, Problem> {
    send_json(Request::post(&url(path)), body).await
}

#[expect(
    dead_code,
    reason = "used by the assignment screens (the student slice)"
)]
pub async fn put<T: DeserializeOwned>(path: &str, body: &impl Serialize) -> Result<T, Problem> {
    send_json(Request::put(&url(path)), body).await
}

#[expect(
    dead_code,
    reason = "used by the assignment screens (the student slice)"
)]
pub async fn delete<T: DeserializeOwned>(path: &str) -> Result<T, Problem> {
    finish(change(Request::delete(&url(path))).send().await).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_use_the_apis_sentence() {
        assert_eq!(
            message_of(
                403,
                r#"{"error": "only cop3502-f26's instructors can do that"}"#
            ),
            "only cop3502-f26's instructors can do that"
        );
        assert_eq!(message_of(404, "<html>"), "not found");
        assert_eq!(
            message_of(502, ""),
            "something went wrong on our side; try again"
        );
        assert_eq!(message_of(418, ""), "the request failed (418)");
    }

    #[test]
    fn only_paths_on_this_site_are_followed_after_login() {
        assert_eq!(
            safe_next(Some("/courses/cop3502-f26")),
            "/courses/cop3502-f26"
        );
        assert_eq!(safe_next(Some("https://evil.example")), "/");
        assert_eq!(safe_next(Some("//evil.example")), "/");
        assert_eq!(safe_next(Some("/login?next=/x")), "/");
        assert_eq!(safe_next(None), "/");
    }
}
