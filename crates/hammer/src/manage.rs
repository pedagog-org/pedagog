//! `hammer users`, `courses`, `members`: managing the platform through the
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

    pub fn put(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.send(
            self.client
                .put(self.url(path))
                .header("content-type", "application/json")
                .body(body.to_string()),
        )
    }

    pub fn patch(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.send(
            self.client
                .patch(self.url(path))
                .header("content-type", "application/json")
                .body(body.to_string()),
        )
    }

    pub fn delete(&self, path: &str) -> Result<serde_json::Value, String> {
        self.send(self.client.delete(self.url(path)))
    }
}

/// `?k=v&...` for the pairs whose value is set, percent-encoded.
pub fn query(pairs: &[(&str, Option<String>)]) -> String {
    let set: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", encode(v))))
        .collect();
    if set.is_empty() {
        String::new()
    } else {
        format!("?{}", set.join("&"))
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn str_of(v: &serde_json::Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_owned()
}

/// `2026-10-18T21:04:05Z` as `2026-10-18 21:04 UTC`.
fn when(v: &serde_json::Value) -> String {
    match v.as_str() {
        Some(t) if t.len() >= 16 => format!("{} UTC", t[..16].replace('T', " ")),
        Some(t) => t.to_owned(),
        None => "-".to_owned(),
    }
}

/// Just the date of a timestamp.
fn day(v: &serde_json::Value) -> String {
    v.as_str()
        .map(|t| t.chars().take(10).collect())
        .unwrap_or_else(|| "-".to_owned())
}

/// `email (name)`.
fn person(u: &serde_json::Value) -> String {
    let name = str_of(u, "name");
    if name.is_empty() {
        str_of(u, "email")
    } else {
        format!("{} ({name})", str_of(u, "email"))
    }
}

fn or_none(lines: Vec<String>, none: &str) -> Vec<String> {
    if lines.is_empty() {
        vec![none.to_owned()]
    } else {
        lines
    }
}

/// The footer of a paged listing, if there's more than one page.
fn paging(v: &serde_json::Value, what: &str) -> Option<String> {
    let (page, pages) = (v["page"].as_i64()?, v["pages"].as_i64()?);
    (pages > 1).then(|| {
        format!(
            "page {page} of {pages} ({} {what}){}",
            v["total"],
            if page < pages {
                format!("; --page {} for more", page + 1)
            } else {
                String::new()
            }
        )
    })
}

/// `users list`: one line per user, with their role.
pub fn users(v: &serde_json::Value) -> Vec<String> {
    let mut lines: Vec<String> = v["users"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|u| format!("{}: {}", person(u), str_of(u, "role")))
        .collect();
    lines = or_none(lines, "no users");
    lines.extend(paging(v, "users"));
    lines
}

/// What a search in `show` found, when it isn't one: the candidates.
fn matches(v: &serde_json::Value, line: impl Fn(&serde_json::Value) -> String) -> Vec<String> {
    let found = v["matches"].as_array().cloned().unwrap_or_default();
    let mut lines = vec![format!("{} match:", found.len())];
    lines.extend(found.iter().map(|m| format!("  {}", line(m))));
    if v["more"].as_bool() == Some(true) {
        lines.push("  ... and more: narrow the search".to_owned());
    }
    lines
}

/// `users show`.
pub fn user(v: &serde_json::Value) -> Vec<String> {
    if v.get("matches").is_some() {
        return matches(v, |u| format!("{}: {}", person(u), str_of(u, "role")));
    }
    let u = &v["user"];
    let login = match u["last_login_kind"].as_str() {
        Some(kind) => format!("{} ({kind})", when(&u["last_login_at"])),
        None => "never".to_owned(),
    };
    let (web, cli) = (
        u["sessions"]["web"].as_i64().unwrap_or(0),
        u["sessions"]["cli"].as_i64().unwrap_or(0),
    );
    let mut lines = vec![
        person(u),
        format!("role:        {}", str_of(u, "role")),
        format!("since:       {}", day(&u["created_at"])),
        format!("last login:  {login}"),
        format!("sessions:    {} active ({web} web, {cli} cli)", web + cli),
    ];
    let courses: Vec<String> = u["courses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let dropped = if c["dropped_at"].is_null() {
                String::new()
            } else {
                format!(" (dropped {})", day(&c["dropped_at"]))
            };
            format!("  {}  {}{dropped}", str_of(c, "id"), str_of(c, "role"))
        })
        .collect();
    if courses.is_empty() {
        lines.push("courses:     none".to_owned());
    } else {
        lines.push("courses:".to_owned());
        lines.extend(courses);
    }
    lines
}

/// `users role`.
pub fn role_set(v: &serde_json::Value) -> Vec<String> {
    let u = &v["user"];
    vec![match v["from"].as_str() {
        Some(from) => format!("{}: {from} -> {}", str_of(u, "email"), str_of(u, "role")),
        None => format!(
            "{} is now {} (new user)",
            str_of(u, "email"),
            str_of(u, "role")
        ),
    }]
}

/// `courses list`: yours with your role, or (with --all) every course.
pub fn courses(v: &serde_json::Value) -> Vec<String> {
    let mut lines: Vec<String> = v["courses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| match c["role"].as_str() {
            Some(role) => format!("{} ({role}): {}", str_of(c, "id"), str_of(c, "name")),
            None => format!("{}: {}", str_of(c, "id"), str_of(c, "name")),
        })
        .collect();
    lines = or_none(lines, "no courses");
    lines.extend(paging(v, "courses"));
    lines
}

/// `courses show`.
pub fn course(c: &serde_json::Value) -> Vec<String> {
    let people = |key: &str| -> Vec<String> {
        c[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|u| format!("  {}", person(u)))
            .collect()
    };
    let mut lines = vec![
        format!("{}: {}", str_of(c, "id"), str_of(c, "name")),
        format!("you: {}", str_of(c, "role")),
        "instructors:".to_owned(),
    ];
    lines.extend(people("instructors"));
    let graders = people("graders");
    if graders.is_empty() {
        lines.push("graders: none".to_owned());
    } else {
        lines.push("graders:".to_owned());
        lines.extend(graders);
    }
    lines.push(format!(
        "students: {} ({} dropped)",
        c["students"], c["dropped"]
    ));
    lines
}

fn member_line(m: &serde_json::Value) -> String {
    let dropped = if m["dropped_at"].is_null() {
        String::new()
    } else {
        format!(" (dropped {})", day(&m["dropped_at"]))
    };
    format!("{}: {}{dropped}", person(m), str_of(m, "role"))
}

/// `members list`.
pub fn members(list: &serde_json::Value) -> Vec<String> {
    or_none(
        list.as_array()
            .into_iter()
            .flatten()
            .map(member_line)
            .collect(),
        "no members",
    )
}

/// `members show`.
pub fn member(v: &serde_json::Value) -> Vec<String> {
    if v.get("matches").is_some() {
        return matches(v, member_line);
    }
    let m = &v["member"];
    let by = match m["added_by"].as_str() {
        Some(by) => format!(", by {by}"),
        None => String::new(),
    };
    vec![
        person(m),
        format!("role:    {}", str_of(m, "role")),
        format!(
            "joined:  {} ({}{by})",
            day(&m["joined_at"]),
            str_of(m, "source")
        ),
        format!("dropped: {}", day(&m["dropped_at"])),
    ]
}

/// `members add`, for one email.
pub fn added(v: &serde_json::Value) -> String {
    let (email, role) = (str_of(v, "email"), str_of(v, "role"));
    match v["change"].as_str() {
        Some("added") => format!("added {email} as {role}"),
        Some("restored") => format!("restored {email} as {role}"),
        Some("changed") => format!("{email}: {} -> {role}", str_of(v, "from")),
        _ => format!("{email} is already {role}"),
    }
}

fn build_line(b: &serde_json::Value) -> String {
    format!(
        "build {}: {}/{}{}{}",
        b["id"],
        str_of(b, "build_status"),
        str_of(b, "validation_status"),
        if b["finished"].as_bool() == Some(false) {
            " (in progress)"
        } else {
            ""
        },
        if b["current"].as_bool() == Some(true) {
            " (current)"
        } else {
            ""
        }
    )
}

/// `assignments list`.
pub fn assignments(list: &serde_json::Value) -> Vec<String> {
    or_none(
        list.as_array()
            .into_iter()
            .flatten()
            .map(|a| {
                let line = format!("{}: {}", str_of(a, "slug"), str_of(a, "name"));
                match a.get("latest_build") {
                    Some(b) if !b.is_null() => format!("{line}  [{}]", build_line(b)),
                    Some(_) => format!("{line}  [no builds]"),
                    None => line,
                }
            })
            .collect(),
        "no assignments",
    )
}

/// `assignments show`.
pub fn assignment(a: &serde_json::Value) -> Vec<String> {
    let mut lines = vec![format!(
        "{}/{}: {}",
        str_of(a, "course"),
        str_of(a, "slug"),
        str_of(a, "name")
    )];
    let builds: Vec<String> = a["builds"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| format!("  {}", build_line(b)))
        .collect();
    lines.extend(or_none(builds, "  no builds"));
    lines
}

/// Who a sync would drop.
pub fn would_drop(v: &serde_json::Value) -> Vec<String> {
    v["would_drop"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e.as_str().map(str::to_owned))
        .collect()
}

/// What a roster import did.
pub fn imported(r: &serde_json::Value) -> Vec<String> {
    let list = |key: &str| -> Vec<String> {
        r[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e.as_str().map(str::to_owned))
            .collect()
    };
    let mut lines = vec![format!(
        "{} added, {} restored, {} already enrolled; {} students now",
        r["added"], r["restored"], r["already"], r["students"]
    )];
    let staff = list("staff");
    if !staff.is_empty() {
        lines.push(format!("left as staff: {}", staff.join(", ")));
    }
    let dropped = list("dropped");
    if !dropped.is_empty() {
        lines.push(format!("dropped {}: {}", dropped.len(), dropped.join(", ")));
    }
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

    use serde_json::json;

    #[test]
    fn queries_encode_what_they_carry() {
        assert_eq!(
            query(&[
                ("role", Some("staff".into())),
                ("search", Some("albert gator&x".into())),
                ("page", None)
            ]),
            "?role=staff&search=albert%20gator%26x"
        );
        assert_eq!(query(&[("page", None)]), "");
    }

    #[test]
    fn user_listings_page() {
        let v = json!({"users": [{"email": "a@ufl.edu", "name": "A", "role": "staff"}], "total": 120, "page": 1, "pages": 3});
        assert_eq!(
            users(&v),
            [
                "a@ufl.edu (A): staff",
                "page 1 of 3 (120 users); --page 2 for more"
            ]
        );
        let last = json!({"users": [], "total": 0, "page": 1, "pages": 1});
        assert_eq!(users(&last), ["no users"]);
    }

    #[test]
    fn a_profile_reads_well() {
        let v = json!({"user": {
            "email": "albert.gator@ufl.edu", "name": "Albert Gator", "role": "student",
            "created_at": "2026-08-20T13:00:00Z", "last_login_at": "2026-10-18T21:04:05Z",
            "last_login_kind": "cli", "sessions": {"web": 1, "cli": 1},
            "courses": [
                {"id": "cop3502-f26", "role": "student", "dropped_at": null},
                {"id": "cop3502-f25", "role": "student", "dropped_at": "2025-09-02T10:00:00Z"}
            ]
        }});
        assert_eq!(
            user(&v),
            [
                "albert.gator@ufl.edu (Albert Gator)",
                "role:        student",
                "since:       2026-08-20",
                "last login:  2026-10-18 21:04 UTC (cli)",
                "sessions:    2 active (1 web, 1 cli)",
                "courses:",
                "  cop3502-f26  student",
                "  cop3502-f25  student (dropped 2025-09-02)",
            ]
        );
        let many = json!({"matches": [{"email": "a@ufl.edu", "name": "", "role": "student"}], "more": true});
        assert_eq!(
            user(&many),
            [
                "1 match:",
                "  a@ufl.edu: student",
                "  ... and more: narrow the search"
            ]
        );
    }

    #[test]
    fn courses_and_members_read_well() {
        let mine = json!({"courses": [{"id": "cop3502-f26", "name": "Programming 1", "role": "instructor"}]});
        assert_eq!(courses(&mine), ["cop3502-f26 (instructor): Programming 1"]);
        assert_eq!(courses(&json!({"courses": []})), ["no courses"]);
        let c = json!({"id": "c", "name": "C", "role": "admin", "instructors": [{"email": "p@ufl.edu", "name": "P"}], "graders": [], "students": 3, "dropped": 1});
        assert_eq!(
            course(&c),
            [
                "c: C",
                "you: admin",
                "instructors:",
                "  p@ufl.edu (P)",
                "graders: none",
                "students: 3 (1 dropped)"
            ]
        );
        let m = json!({"member": {"email": "a@ufl.edu", "name": "A", "role": "student", "source": "import", "added_by": "p@ufl.edu", "joined_at": "2026-08-20T00:00:00Z", "dropped_at": null}});
        assert_eq!(
            member(&m),
            [
                "a@ufl.edu (A)",
                "role:    student",
                "joined:  2026-08-20 (import, by p@ufl.edu)",
                "dropped: -"
            ]
        );
        let list = json!([{"email": "b@ufl.edu", "name": "", "role": "grader", "dropped_at": "2026-09-01T00:00:00Z"}]);
        assert_eq!(members(&list), ["b@ufl.edu: grader (dropped 2026-09-01)"]);
        assert_eq!(
            added(
                &json!({"email": "b@ufl.edu", "role": "grader", "change": "changed", "from": "student"})
            ),
            "b@ufl.edu: student -> grader"
        );
    }

    #[test]
    fn assignments_read_well() {
        let list = json!([
            {"slug": "linked-list", "name": "Linked List", "current_build": 3,
             "latest_build": {"id": 4, "build_status": "running", "validation_status": "pending", "finished": false, "current": false}},
            {"slug": "maps", "name": "Maps", "latest_build": null},
            {"slug": "graphs", "name": "Graphs"}
        ]);
        assert_eq!(
            assignments(&list),
            [
                "linked-list: Linked List  [build 4: running/pending (in progress)]",
                "maps: Maps  [no builds]",
                "graphs: Graphs"
            ]
        );
        let shown = json!({"course": "c", "slug": "ll", "name": "LL", "builds": [
            {"id": 3, "build_status": "succeeded", "validation_status": "passed", "finished": true, "current": true}
        ]});
        assert_eq!(
            assignment(&shown),
            ["c/ll: LL", "  build 3: succeeded/passed (current)"]
        );
    }

    #[test]
    fn imports_report_what_changed() {
        let r = json!({"added": 2, "restored": 0, "already": 1, "students": 3, "staff": ["ta@ufl.edu"], "dropped": ["bob@ufl.edu"], "skipped": [{"line": 4, "why": "\"nope\" isn't an email address"}]});
        assert_eq!(
            imported(&r),
            [
                "2 added, 0 restored, 1 already enrolled; 3 students now",
                "left as staff: ta@ufl.edu",
                "dropped 1: bob@ufl.edu",
                "skipped 1 row(s):",
                "  line 4: \"nope\" isn't an email address"
            ]
        );
    }
}
