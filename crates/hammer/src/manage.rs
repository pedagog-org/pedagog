//! `hammer admins`, `courses`, `roster`: managing the platform through the
//! api with the saved session.

use std::time::Duration;

use crate::account;

/// The api, as the logged-in user.
pub struct Api {
    base: String,
    token: String,
    client: reqwest::blocking::Client,
}

impl Api {
    pub fn from_session() -> Result<Self, String> {
        let session = account::load().ok_or("not logged in: run `hammer login`")?;
        Ok(Self {
            base: session.api,
            token: session.token,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|e| e.to_string())?,
        })
    }

    fn send(&self, req: reqwest::blocking::RequestBuilder) -> Result<serde_json::Value, String> {
        let reply = req
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| format!("cannot reach the api at {}: {e}", self.base))?;
        let status = reply.status();
        let text = reply.text().map_err(|e| e.to_string())?;
        let body = if text.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text))
        };
        if status.is_success() {
            Ok(body)
        } else {
            Err(body
                .get("error")
                .and_then(|e| e.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("the api answered {status}")))
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn get(&self, path: &str) -> Result<serde_json::Value, String> {
        self.send(self.client.get(self.url(path)))
    }

    pub fn post(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.send(
            self.client
                .post(self.url(path))
                .header("content-type", "application/json")
                .body(body.to_string()),
        )
    }

    pub fn post_text(
        &self,
        path: &str,
        content_type: &str,
        body: String,
    ) -> Result<serde_json::Value, String> {
        self.send(
            self.client
                .post(self.url(path))
                .header("content-type", content_type)
                .body(body),
        )
    }

    pub fn put(&self, path: &str) -> Result<serde_json::Value, String> {
        self.send(self.client.put(self.url(path)))
    }

    pub fn delete(&self, path: &str) -> Result<serde_json::Value, String> {
        self.send(self.client.delete(self.url(path)))
    }
}

fn str_of(v: &serde_json::Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_owned()
}

/// One line per user: `email (name)`.
pub fn users(list: &serde_json::Value) -> Vec<String> {
    list.as_array()
        .map(|users| {
            users
                .iter()
                .map(|u| {
                    let name = str_of(u, "name");
                    if name.is_empty() {
                        str_of(u, "email")
                    } else {
                        format!("{} ({name})", str_of(u, "email"))
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn courses(list: &serde_json::Value) -> Vec<String> {
    let lines: Vec<String> = list
        .as_array()
        .map(|cs| {
            cs.iter()
                .map(|c| {
                    format!(
                        "{} ({}): {}",
                        str_of(c, "id"),
                        str_of(c, "role"),
                        str_of(c, "name")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if lines.is_empty() {
        vec!["no courses".to_owned()]
    } else {
        lines
    }
}

pub fn course(c: &serde_json::Value) -> Vec<String> {
    let mut lines = vec![
        format!("{}: {}", str_of(c, "id"), str_of(c, "name")),
        format!("students: {}", c["students"]),
        "instructors:".to_owned(),
    ];
    lines.extend(
        users(&c["instructors"])
            .into_iter()
            .map(|u| format!("  {u}")),
    );
    lines
}

/// What a roster import did.
pub fn imported(r: &serde_json::Value) -> Vec<String> {
    let mut lines = vec![format!(
        "{} added, {} already enrolled; {} students now",
        r["added"], r["already"], r["students"]
    )];
    if let Some(skipped) = r["skipped"].as_array()
        && !skipped.is_empty()
    {
        lines.push(format!("skipped {} row(s):", skipped.len()));
        lines.extend(
            skipped
                .iter()
                .map(|s| format!("  line {}: {}", s["line"], str_of(s, "why"))),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_read_well() {
        let us = serde_json::json!([{"email": "a@ufl.edu", "name": "A"}, {"email": "b@ufl.edu", "name": ""}]);
        assert_eq!(users(&us), ["a@ufl.edu (A)", "b@ufl.edu"]);
        assert_eq!(courses(&serde_json::json!([])), ["no courses"]);
        let cs = serde_json::json!([{"id": "cop3502-f26", "role": "staff", "name": "COP3502"}]);
        assert_eq!(courses(&cs), ["cop3502-f26 (staff): COP3502"]);
    }

    #[test]
    fn imports_report_skipped_rows() {
        let r = serde_json::json!({"added": 2, "already": 1, "students": 3, "skipped": [{"line": 4, "why": "\"nope\" isn't an email address"}]});
        assert_eq!(
            imported(&r),
            [
                "2 added, 1 already enrolled; 3 students now",
                "skipped 1 row(s):",
                "  line 4: \"nope\" isn't an email address"
            ]
        );
    }
}
