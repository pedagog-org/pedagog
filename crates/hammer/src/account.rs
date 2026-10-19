//! `hammer login`, `logout`, `whoami`: a session with the api, saved in
//! `$XDG_CONFIG_HOME/pedagog/session.json` (or `~/.config/pedagog/`), readable
//! only by you.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const API_URL_ENV: &str = "PEDAGOG_API_URL";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub api: String,
    pub email: String,
    pub token: String,
}

pub fn session_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("pedagog").join("session.json"))
}

pub fn load() -> Option<Session> {
    let text = std::fs::read_to_string(session_path()?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save(session: &Session) -> Result<PathBuf, String> {
    let path = session_path().ok_or("no home directory to save the session in")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    write_private(&path, json.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

/// The api's message from an error reply, or the body itself.
fn error_of(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| body.to_owned())
}

fn post(url: &str, body: &serde_json::Value, token: Option<&str>) -> Result<String, String> {
    let mut req = client()?
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_string());
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let reply = req
        .send()
        .map_err(|e| format!("cannot reach the api at {url}: {e}"))?;
    let status = reply.status();
    let text = reply.text().map_err(|e| e.to_string())?;
    if status.is_success() {
        Ok(text)
    } else {
        Err(error_of(&text))
    }
}

/// Ask for a code, read it from `input`, and save the session.
pub fn login(
    api: &str,
    email: &str,
    input: &mut impl BufRead,
) -> Result<(Session, PathBuf), String> {
    let api = api.trim_end_matches('/');
    post(
        &format!("{api}/auth/start"),
        &serde_json::json!({ "email": email }),
        None,
    )?;
    print!("A login code was sent to {email} (if it's registered). Code: ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut code = String::new();
    input.read_line(&mut code).map_err(|e| e.to_string())?;
    let reply = post(
        &format!("{api}/auth/finish"),
        &serde_json::json!({ "email": email, "code": code.trim(), "client": "cli" }),
        None,
    )?;
    let reply: serde_json::Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    let token = reply["token"]
        .as_str()
        .ok_or("the api's reply has no token")?
        .to_owned();
    let session = Session {
        api: api.to_owned(),
        email: reply["user"]["email"].as_str().unwrap_or(email).to_owned(),
        token,
    };
    let path = save(&session)?;
    Ok((session, path))
}

/// End the saved session (on the api, then locally).
pub fn logout() -> Result<Option<String>, String> {
    let Some(session) = load() else {
        return Ok(None);
    };
    // Already expired or revoked is fine: forget it either way.
    let _ = post(
        &format!("{}/auth/logout", session.api),
        &serde_json::json!({}),
        Some(&session.token),
    );
    if let Some(path) = session_path() {
        let _ = std::fs::remove_file(path);
    }
    Ok(Some(session.email))
}

/// Who the saved session belongs to, and their courses, as lines.
pub fn whoami() -> Result<Vec<String>, String> {
    let session = load().ok_or("not logged in: run `hammer login`")?;
    let url = format!("{}/me", session.api);
    let reply = client()?
        .get(&url)
        .bearer_auth(&session.token)
        .send()
        .map_err(|e| format!("cannot reach the api at {url}: {e}"))?;
    let status = reply.status();
    let text = reply.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(error_of(&text));
    }
    let me: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(describe(&me, &session.api))
}

fn describe(me: &serde_json::Value, api: &str) -> Vec<String> {
    let user = &me["user"];
    let mut lines = vec![format!(
        "{}{} at {api}",
        user["email"].as_str().unwrap_or("?"),
        if user["is_admin"].as_bool() == Some(true) {
            " (admin)"
        } else {
            ""
        }
    )];
    match me["courses"].as_array().map(Vec::as_slice) {
        Some([]) | None => lines.push("no courses".to_owned()),
        Some(courses) => {
            for c in courses {
                lines.push(format!(
                    "  {} ({}): {}",
                    c["id"].as_str().unwrap_or("?"),
                    c["role"].as_str().unwrap_or("?"),
                    c["name"].as_str().unwrap_or("")
                ));
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whoami_lists_the_user_and_their_courses() {
        let me = serde_json::json!({
            "user": {"email": "conder@ufl.edu", "name": "", "is_admin": true},
            "courses": [{"id": "cop3502-f26", "name": "COP3502", "role": "staff"}]
        });
        assert_eq!(
            describe(&me, "https://pedagog.example"),
            [
                "conder@ufl.edu (admin) at https://pedagog.example",
                "  cop3502-f26 (staff): COP3502"
            ]
        );
        let none =
            serde_json::json!({"user": {"email": "a@ufl.edu", "is_admin": false}, "courses": []});
        assert_eq!(describe(&none, "x")[1], "no courses");
    }

    #[test]
    fn errors_come_from_the_api_reply() {
        assert_eq!(
            error_of(r#"{"error": "wrong code; 4 tries left"}"#),
            "wrong code; 4 tries left"
        );
        assert_eq!(error_of("bad gateway"), "bad gateway");
    }

    #[cfg(unix)]
    #[test]
    fn the_session_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("hammer-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        write_private(&path, b"{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
