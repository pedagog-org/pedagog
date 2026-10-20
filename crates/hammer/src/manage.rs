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

/// `submissions`: latest first, with when each counts and how it stands.
pub fn submissions(list: &serde_json::Value) -> Vec<String> {
    or_none(
        list.as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                let mut line = format!(
                    "#{}  {}  {}",
                    s["id"],
                    when(&s["counted_at"]),
                    match s["late"].as_str() {
                        Some(late) => format!("late by {late}"),
                        None if s["staff"].as_bool() == Some(true) => "staff".to_owned(),
                        None => "on time".to_owned(),
                    }
                );
                if let Some(status) = s["status"].as_str() {
                    line.push_str(&format!("  {status}"));
                }
                if let (Some(score), Some(total)) = (s["score"].as_f64(), s["total"].as_f64()) {
                    line.push_str(&format!(" {score}/{total}"));
                }
                if let Some(by) = s["submitted_by"].as_str() {
                    line.push_str(&format!("  submitted by {by}"));
                }
                if s["counts"].as_bool() == Some(true) {
                    line.push_str(if s["activated_by"].is_null() {
                        " <- counts"
                    } else {
                        " <- counts (activated)"
                    });
                }
                // As shown: a fraction of a second apart is the same time.
                if s["created_at"].is_string() && when(&s["created_at"]) != when(&s["counted_at"]) {
                    line.push_str(&format!(" (arrived {})", when(&s["created_at"])));
                }
                line
            })
            .collect(),
        "no submissions",
    )
}

/// `dates show|set|clear`: each field, where it comes from, and the status.
pub fn dates(v: &serde_json::Value) -> Vec<String> {
    let field = |label: &str, key: &str| {
        let f = &v[key];
        match f["value"].as_str() {
            Some(value) => format!("{label:<10}{value}  ({})", str_of(f, "from")),
            None => format!("{label:<10}-"),
        }
    };
    vec![
        field("open:", "open"),
        field("due:", "due"),
        field("late due:", "late_due"),
        format!("{:<10}{}", "status:", str_of(v, "status")),
    ]
}

/// The parts of a test's result, each with its own visibility.
const PARTS: [&str; 4] = ["score", "feedback", "stdout", "error"];

/// `visibility show`: each test's parts, decided by which layer, and
/// whether students see them now; `verbose` adds every layer's setting.
pub fn visibility(v: &serde_json::Value, test: Option<&str>, verbose: bool) -> Vec<String> {
    let mut lines = vec![format!(
        "released: {}   end: {}",
        v["released_at"]
            .as_str()
            .map(|r| format!("yes ({r})"))
            .unwrap_or_else(|| "no".to_owned()),
        v["end"].as_str().unwrap_or("none")
    )];
    if let Some(note) = v["note"].as_str() {
        lines.push(note.to_owned());
    }
    let tests: Vec<&serde_json::Value> = v["tests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| {
            test.is_none_or(|q| {
                [&t["id"], &t["node"], &t["name"]]
                    .iter()
                    .any(|f| f.as_str() == Some(q))
            })
        })
        .collect();
    if tests.is_empty() && test.is_some() {
        lines.push(format!("no test {:?}", test.unwrap_or_default()));
    }
    let shown = |v: &serde_json::Value| -> String {
        let value = str_of(v, "value");
        format!("{value:<14}{}  [{}]", str_of(v, "why"), str_of(v, "from"))
    };
    for t in tests {
        let id = t["id"].as_str().unwrap_or("");
        lines.push(if id.is_empty() {
            format!("{} ({})", str_of(t, "name"), str_of(t, "node"))
        } else {
            format!("{} ({id}, {})", str_of(t, "name"), str_of(t, "node"))
        });
        for part in PARTS {
            lines.push(format!("  {:<9} {}", format!("{part}:"), shown(&t[part])));
        }
        if verbose {
            for l in t["layers"].as_array().into_iter().flatten() {
                let parts: Vec<String> = PARTS
                    .iter()
                    .map(|p| format!("{p} {:<14}", l[*p].as_str().unwrap_or("-")))
                    .collect();
                lines.push(format!(
                    "    {:<20} {}",
                    str_of(l, "layer"),
                    parts.join(" ").trim_end()
                ));
            }
        }
    }
    lines
}

/// A number as people write it: `2`, `2.5`.
fn num(v: &serde_json::Value) -> String {
    match v.as_f64() {
        Some(x) => format!("{x}"),
        None => "?".to_owned(),
    }
}

/// `2/3`, with a star where it's what the student sees now.
fn stage_cell(st: &serde_json::Value) -> String {
    format!(
        "{}/{}{}",
        num(&st["score"]),
        num(&st["total"]),
        if st["student_sees"].as_bool() == Some(true) {
            "*"
        } else {
            ""
        }
    )
}

/// Rows of cells as aligned columns.
fn table(rows: &[Vec<String>]) -> Vec<String> {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..width)
        .map(|i| {
            rows.iter()
                .filter_map(|r| r.get(i))
                .map(|c| c.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, c)| format!("{c:<w$}", w = widths[i]))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_owned()
        })
        .collect()
}

const STAGES: [&str; 4] = ["visible", "after deadline", "after release", "hidden"];

/// `results list`.
pub fn results_list(v: &serde_json::Value) -> Vec<String> {
    let mut rows = vec![
        ["name", "email", "counts", "late"]
            .iter()
            .chain(STAGES.iter())
            .chain(["passed"].iter())
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    ];
    for r in v["rows"].as_array().into_iter().flatten() {
        let mut row = vec![
            str_of(r, "name"),
            str_of(r, "email"),
            format!(
                "#{}{}",
                r["submission"],
                if r["activated_by"].is_null() {
                    ""
                } else {
                    " (activated)"
                }
            ),
            r["late"].as_str().unwrap_or("-").to_owned(),
        ];
        match r["stages"].as_array() {
            Some(stages) => {
                row.extend(stages.iter().map(stage_cell));
                let all = &stages[stages.len() - 1];
                row.push(format!("{}/{}", all["passed"], all["tests"]));
            }
            None => row.push(match r["status"].as_str() {
                Some("error") => format!("error ({})", str_of(r, "error_kind")),
                Some(status) => status.to_owned(),
                None => "no run".to_owned(),
            }),
        }
        rows.push(row);
    }
    let mut lines = table(&rows);
    lines.push(format!(
        "{} of {} students submitted   * what each student sees now",
        v["submitted"], v["students"]
    ));
    lines
}

fn steps_lines(steps: &serde_json::Value, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for st in steps.as_array().into_iter().flatten() {
        let reason = st["reason"]
            .as_str()
            .map(|r| format!(" {r}"))
            .unwrap_or_default();
        lines.push(match st["result"].as_str() {
            Some("pass") => format!("{indent}✓ +{}{reason}", num(&st["points"])),
            Some(_) => format!("{indent}✗ +0{reason} (worth {})", num(&st["worth"])),
            None => {
                let p = st["points"].as_f64().unwrap_or(0.0);
                format!(
                    "{indent}{}{}{reason}",
                    if p >= 0.0 { "+" } else { "" },
                    num(&st["points"])
                )
            }
        });
        if let Some(m) = st["message"].as_str() {
            lines.push(format!("{indent}    {m}"));
        }
    }
    lines
}

fn indented(label: &str, text: &str) -> Vec<String> {
    let mut lines = vec![format!("  {label}:")];
    lines.extend(text.lines().map(|l| format!("    {l}")));
    lines
}

/// `results show` for staff: everything, and what the student sees of it.
pub fn results_staff(v: &serde_json::Value) -> Vec<String> {
    if v.get("matches").is_some() {
        return matches(v, |m| format!("{}: {}", person(m), str_of(m, "role")));
    }
    let counts = match (v["counts"].as_bool(), v["activated_by"].as_str()) {
        (Some(true), Some(by)) => format!("counts (activated by {by})"),
        (Some(true), None) => "counts".to_owned(),
        _ => "doesn't count".to_owned(),
    };
    let mut lines = vec![format!(
        "{}  #{} {counts}, {}",
        person(v),
        v["submission"],
        v["late"]
            .as_str()
            .map(|l| format!("late by {l}"))
            .unwrap_or_else(|| "on time".to_owned())
    )];
    if let Some(by) = v["submitted_by"].as_str() {
        lines.push(format!(
            "submitted by {by}, counted at {}",
            when(&v["counted_at"])
        ));
    }
    let run = &v["run"];
    if run["status"] == "error" {
        lines.push(format!(
            "couldn't be graded ({})",
            str_of(run, "error_kind")
        ));
        lines.extend(
            run["problems"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str())
                .map(|p| format!("  {p}")),
        );
        if let Some(out) = v["run_output"].as_str().filter(|o| !o.is_empty()) {
            lines.extend(indented("run output", out));
        }
        return lines;
    }
    if !run.is_object() || run["status"] != "graded" {
        lines.push(format!(
            "run: {}",
            run["status"].as_str().unwrap_or("none yet")
        ));
        return lines;
    }
    let sees = |p: &serde_json::Value| -> String {
        if p["shown"].as_bool() == Some(true) {
            return "yes".to_owned();
        }
        match p["value"].as_str() {
            Some("after_end") => "after deadline",
            Some("after_publish") => "after release",
            _ => "never",
        }
        .to_owned()
    };
    let mut rows = vec![
        [
            "test",
            "result",
            "score",
            "student sees: score",
            "feedback",
            "stdout",
            "error",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>(),
    ];
    let tests = v["tests"].as_array().cloned().unwrap_or_default();
    for t in &tests {
        let vis = &t["visibility"];
        rows.push(vec![
            str_of(t, "name"),
            str_of(t, "status"),
            format!("{}/{}", num(&t["score"]), num(&t["max"])),
            sees(&vis["score"]),
            sees(&vis["feedback"]),
            sees(&vis["stdout"]),
            sees(&vis["error"]),
        ]);
    }
    lines.extend(table(&rows));
    if let Some(stages) = v["stages"].as_array() {
        let mut rows = vec![
            std::iter::once(String::new())
                .chain(STAGES.iter().map(|s| s.to_string()))
                .collect::<Vec<_>>(),
        ];
        rows.push(
            std::iter::once("score".to_owned())
                .chain(stages.iter().map(stage_cell))
                .collect(),
        );
        rows.push(
            std::iter::once("passed".to_owned())
                .chain(stages.iter().map(|st| {
                    format!(
                        "{}/{}{}",
                        st["passed"],
                        st["tests"],
                        if st["student_sees"].as_bool() == Some(true) {
                            "*"
                        } else {
                            ""
                        }
                    )
                }))
                .collect(),
        );
        lines.push(String::new());
        lines.extend(table(&rows));
    }
    if let Some(told) = v["student_is_told"].as_array().filter(|t| !t.is_empty()) {
        let when: Vec<&str> = told.iter().filter_map(|w| w.as_str()).collect();
        lines.push(format!(
            "student is told: some tests are hidden until {}",
            when.join(", others until ")
        ));
    }
    for t in &tests {
        let mut detail = Vec::new();
        for item in t["items"].as_array().into_iter().flatten() {
            let steps = steps_lines(&item["steps"], "    ");
            if !steps.is_empty() {
                detail.push(format!("  feedback ({}):", str_of(item, "name")));
                detail.extend(steps);
            }
        }
        for (label, key) in [("stdout", "stdout"), ("error", "error"), ("trace", "trace")] {
            if let Some(text) = t[key].as_str().filter(|x| !x.is_empty()) {
                detail.extend(indented(label, text));
            }
        }
        if !detail.is_empty() {
            lines.push(format!("--- {} ---", str_of(t, "name")));
            lines.extend(detail);
        }
    }
    lines
}

/// `results show` for a student: what they may see now.
pub fn results_mine(v: &serde_json::Value) -> Vec<String> {
    let mut head = format!("#{}", v["submission"]);
    if v["counts"].as_bool() == Some(true) {
        head.push_str(" (counts)");
    }
    head.push_str(&match v["late"].as_str() {
        Some(l) => format!(", late by {l}"),
        None => ", on time".to_owned(),
    });
    if let Some(by) = v["submitted_by"].as_str() {
        head.push_str(&format!(", submitted by {by} on your behalf"));
    }
    match v["status"].as_str() {
        Some("error") => {
            let mut lines = vec![format!("{head}: couldn't be graded: {}", str_of(v, "why"))];
            lines.extend(v["next"].as_str().map(|n| format!("  {n}")));
            return lines;
        }
        Some("graded") => {}
        Some(other) => return vec![format!("{head}: {other}")],
        None => return vec![head],
    }
    let r = &v["results"];
    let mut lines = vec![format!(
        "{head}: {}/{} on the tests shown",
        num(&r["score"]),
        num(&r["total"])
    )];
    let tests = r["tests"].as_array().cloned().unwrap_or_default();
    let rows: Vec<Vec<String>> = tests
        .iter()
        .map(|t| {
            vec![
                str_of(t, "name"),
                str_of(t, "status"),
                format!("{}/{}", num(&t["score"]), num(&t["max"])),
            ]
        })
        .collect();
    lines.extend(table(&rows));
    let hidden: Vec<&str> = r["hidden_until"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| w.as_str())
        .collect();
    if !hidden.is_empty() {
        lines.push(format!(
            "some tests are hidden until {}",
            hidden.join(", others until ")
        ));
    } else if r["hidden_never"].as_bool() == Some(true) {
        lines.push("some tests aren't shown".to_owned());
    }
    for t in &tests {
        let mut detail = Vec::new();
        for item in t["feedback"].as_array().into_iter().flatten() {
            detail.extend(steps_lines(&item["steps"], "  "));
            if let Some(text) = item["text"].as_str().filter(|x| !x.is_empty()) {
                detail.extend(text.lines().map(|l| format!("  {l}")));
            }
        }
        for key in ["stdout", "error"] {
            if let Some(text) = t[key].as_str().filter(|x| !x.is_empty()) {
                detail.extend(indented(key, text));
            }
        }
        if !detail.is_empty() {
            lines.push(format!("--- {} ---", str_of(t, "name")));
            lines.extend(detail);
        }
    }
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
    fn submissions_read_well() {
        let list = json!([
            {"id": 14, "counted_at": "2026-09-09T03:41:00Z", "late": null, "staff": false,
             "status": "graded", "submitted_by": "prof@ufl.edu", "created_at": "2026-09-12T15:00:00Z",
             "score": 10.0, "total": 10.0},
            {"id": 13, "counted_at": "2026-09-09T06:13:00.123456Z", "late": "2 hours 14 minutes",
             "staff": false, "status": "error", "submitted_by": null, "created_at": "2026-09-09T06:13:00Z"}
        ]);
        assert_eq!(
            submissions(&list),
            [
                "#14  2026-09-09 03:41 UTC  on time  graded 10/10  submitted by prof@ufl.edu (arrived 2026-09-12 15:00 UTC)",
                "#13  2026-09-09 06:13 UTC  late by 2 hours 14 minutes  error"
            ]
        );
        assert_eq!(submissions(&json!([])), ["no submissions"]);
    }

    #[test]
    fn dates_and_visibility_read_well() {
        let d = json!({"open": {"value": "always", "from": "course"},
                       "due": {"value": "2026-09-08T23:59:00-04:00", "from": "file"},
                       "late_due": null, "status": "open"});
        assert_eq!(
            dates(&d),
            [
                "open:     always  (course)",
                "due:      2026-09-08T23:59:00-04:00  (file)",
                "late due: -",
                "status:   open"
            ]
        );
        let part =
            |value: &str, from: &str, why: &str| json!({"value": value, "from": from, "why": why});
        let v = json!({"released_at": null, "end": "2026-09-08T23:59:00-04:00", "tests": [
            {"id": "push", "node": "t.py::test_push", "name": "push",
             "score": part("visible", "the test's own", "always shown"),
             "feedback": part("visible", "the test's own", "always shown"),
             "stdout": part("visible", "the test's own", "always shown"),
             "error": part("after_end", "file, all tests", "not yet: until the end"),
             "layers": [{"layer": "course, this test", "score": null, "feedback": null, "stdout": null, "error": "hidden"}]},
            {"id": null, "node": "t.py::test_stress", "name": "stress",
             "score": part("hidden", "course, all tests", "never shown to students"),
             "feedback": part("hidden", "course, all tests", "never shown to students"),
             "stdout": part("hidden", "course, all tests", "never shown to students"),
             "error": part("hidden", "course, all tests", "never shown to students"),
             "layers": []}
        ]});
        let all = visibility(&v, None, false);
        assert_eq!(all[0], "released: no   end: 2026-09-08T23:59:00-04:00");
        assert_eq!(all[1], "push (push, t.py::test_push)");
        assert_eq!(
            all[2],
            "  score:    visible       always shown  [the test's own]"
        );
        assert_eq!(
            all[5],
            "  error:    after_end     not yet: until the end  [file, all tests]"
        );
        assert_eq!(all[6], "stress (t.py::test_stress)");
        let one = visibility(&v, Some("push"), true);
        assert_eq!(one.len(), 7);
        assert_eq!(
            one[6],
            "    course, this test    score -              feedback -              stdout -              error hidden"
        );
        assert_eq!(visibility(&v, Some("nope"), false)[1], "no test \"nope\"");
    }

    #[test]
    fn results_read_well() {
        let st = |score: f64, total: f64, passed: u32, tests: u32, sees: bool| json!({"score": score, "total": total, "passed": passed, "tests": tests, "student_sees": sees});
        let list = json!({"rows": [
            {"name": "Albert Gator", "email": "albert@ufl.edu", "submission": 133, "activated_by": null,
             "late": null, "status": "graded",
             "stages": [st(5.0, 5.0, 2, 2, false), st(5.0, 5.0, 2, 2, true), st(7.0, 8.0, 2, 3, false), st(7.0, 9.0, 2, 4, false)]},
            {"name": "Zed", "email": "zz@ufl.edu", "submission": 120, "activated_by": "ta@ufl.edu",
             "late": "2 hours", "status": "error", "error_kind": "load", "stages": null}
        ], "submitted": 2, "students": 143});
        let lines = results_list(&list);
        assert!(
            lines[0].starts_with("name          email           counts"),
            "{lines:?}"
        );
        assert!(
            lines[1].contains("5/5*") && lines[1].ends_with("2/4"),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].contains("#120 (activated)") && lines[2].contains("error (load)"),
            "{}",
            lines[2]
        );
        assert_eq!(
            lines[3],
            "2 of 143 students submitted   * what each student sees now"
        );

        let mine = json!({"submission": 133, "counts": true, "late": "2 hours", "status": "graded",
            "results": {"score": 2.0, "total": 2.0, "hidden_until": ["release"], "hidden_never": false,
              "tests": [{"name": "push", "status": "pass", "score": 2.0, "max": 2.0,
                         "feedback": [{"name": "push", "steps": [
                             {"result": "pass", "points": 2.0, "worth": 2.0, "reason": "pushes"},
                             {"result": "fail", "points": 0.0, "worth": 1.0, "reason": "pops", "message": "pop() lost it"}]}],
                         "stdout": "pushed 3\n"}]}});
        assert_eq!(
            results_mine(&mine),
            [
                "#133 (counts), late by 2 hours: 2/2 on the tests shown",
                "push  pass  2/2",
                "some tests are hidden until release",
                "--- push ---",
                "  ✓ +2 pushes",
                "  ✗ +0 pops (worth 1)",
                "      pop() lost it",
                "  stdout:",
                "    pushed 3",
            ]
        );
        let err = json!({"submission": 140, "counts": true, "late": null, "status": "error",
                         "why": "the tests couldn't run with your code",
                         "next": "check your files and submit again; if problems persist, contact course staff"});
        assert_eq!(
            results_mine(&err),
            [
                "#140 (counts), on time: couldn't be graded: the tests couldn't run with your code",
                "  check your files and submit again; if problems persist, contact course staff"
            ]
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
