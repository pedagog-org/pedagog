//! The course's policy for an assignment, over the file's: dates (edited field
//! by field), visibility overrides (for all tests or one), and releasing
//! results. Edits are database writes and apply at once; they never build.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pedagog_core::recipe::policy::{
    Acceptance, AssignmentPolicy, DateEdits, Dates, FieldOverrides, Instant, Open, ResultsPolicy,
};
use pedagog_core::results::{Results, Visibility};
use pedagog_store::Assignment;
use pedagog_store::db::assignments::{self as db, Overrides};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::assignments::find;
use crate::auth::CurrentUser;
use crate::courses::{Need, require};
use crate::error::{ApiError, internal, refused};
use crate::state::AppState;

/// An assignment's policy: the current build's file, and the course's edits.
pub struct Policy {
    pub file: AssignmentPolicy,
    pub course_dates: DateEdits,
    pub course_visibility: ResultsPolicy,
    /// When results were released (RFC 3339).
    pub released_at: Option<String>,
    /// The current build's validation results, to list tests from.
    pub tests: Option<Results>,
}

impl Policy {
    /// The dates in effect: the course's edits over the file's, field by field.
    pub fn dates(&self) -> Dates {
        Dates::merged(&self.file.dates, &self.course_dates)
    }
}

fn bad_json(what: &str) -> impl Fn(serde_json::Error) -> ApiError + '_ {
    move |e| internal(what, e)
}

pub async fn policy_of(state: &AppState, a: &Assignment) -> Result<Policy, ApiError> {
    let build = match a.current_build_id {
        Some(b) => db::build_by_id(&state.pool, b)
            .await
            .map_err(|e| internal("reading the build", e))?,
        None => None,
    };
    let file = build
        .as_ref()
        .and_then(|b| b.policy.as_deref())
        .map(serde_json::from_str::<AssignmentPolicy>)
        .transpose()
        .map_err(bad_json("reading the assignment's policy"))?
        .unwrap_or_default();
    let tests = build
        .as_ref()
        .and_then(|b| b.results.as_deref())
        .and_then(|r| serde_json::from_str::<Results>(r).ok());
    let course = db::course_policy(&state.pool, a.id)
        .await
        .map_err(|e| internal("reading the course's policy", e))?;
    Ok(Policy {
        file,
        course_dates: serde_json::from_str(&course.date_overrides)
            .map_err(bad_json("reading the course's dates"))?,
        course_visibility: serde_json::from_str(&course.visibility_overrides)
            .map_err(bad_json("reading the course's visibility"))?,
        released_at: course.released_at,
        tests,
    })
}

fn open_text(o: Option<Open>) -> Option<String> {
    o.map(String::from)
}

fn instant_text(i: Option<Instant>) -> Option<String> {
    i.map(String::from)
}

/// Each field's value in effect and where it comes from.
fn dates_view(p: &Policy) -> serde_json::Value {
    let field = |course: Option<String>, file: Option<String>| match (course, file) {
        (Some(c), _) => serde_json::json!({ "value": c, "from": "course" }),
        (None, Some(f)) => serde_json::json!({ "value": f, "from": "file" }),
        (None, None) => serde_json::Value::Null,
    };
    let (c, f) = (&p.course_dates, &p.file.dates);
    let status = match p.dates().acceptance(OffsetDateTime::now_utc()) {
        Acceptance::OnTime => "open".to_owned(),
        Acceptance::Late => "open, late".to_owned(),
        Acceptance::Closed(why) => why,
    };
    serde_json::json!({
        "open": field(open_text(c.open), open_text(f.open)),
        "due": field(instant_text(c.due), instant_text(f.due)),
        "late_due": field(instant_text(c.late_due), instant_text(f.late_due)),
        "status": status,
    })
}

/// The dates in effect, field by field (anyone in the course).
pub async fn show_dates(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Member).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    Ok(Json(dates_view(&policy_of(&state, &a).await?)).into_response())
}

#[derive(Deserialize)]
pub struct DatesBody {
    /// A time, or `always`.
    open: Option<String>,
    due: Option<String>,
    late_due: Option<String>,
}

/// Edit the course's dates: only the given fields change; the result, over
/// the file's, must be in order.
pub async fn set_dates(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Json(body): Json<DatesBody>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let bad = |e: String| refused(StatusCode::BAD_REQUEST, e);
    let open = body.open.map(Open::try_from).transpose().map_err(bad)?;
    let due = body.due.map(Instant::try_from).transpose().map_err(bad)?;
    let late_due = body
        .late_due
        .map(Instant::try_from)
        .transpose()
        .map_err(bad)?;
    if open.is_none() && due.is_none() && late_due.is_none() {
        return Err(bad("give --open, --due or --late-due".to_owned()));
    }
    let file = policy_of(&state, &a).await?.file.dates;
    db::edit_overrides(&state.pool, a.id, Overrides::Dates, |old| {
        let mut dates: DateEdits = serde_json::from_str(old).map_err(|e| e.to_string())?;
        dates.open = open.or(dates.open);
        dates.due = due.or(dates.due);
        dates.late_due = late_due.or(dates.late_due);
        Dates::merged(&file, &dates)
            .checked()
            .map_err(|e| format!("those dates don't work: {e}"))?;
        serde_json::to_string(&dates).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| internal("saving the dates", e))?
    .map_err(bad)?;
    tracing::info!(by = current.user.id, %course, assignment = %slug, "dates set");
    Ok(Json(dates_view(&policy_of(&state, &a).await?)).into_response())
}

#[derive(Deserialize)]
pub struct ClearQuery {
    /// Comma-separated fields; all of them if absent.
    fields: Option<String>,
}

/// Drop some (or all) of the course's date edits; the file's apply again.
pub async fn clear_dates(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Query(q): Query<ClearQuery>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let fields: Vec<String> = match &q.fields {
        Some(f) => f.split(',').map(|s| s.trim().replace('-', "_")).collect(),
        None => vec!["open".into(), "due".into(), "late_due".into()],
    };
    if let Some(bad) = fields
        .iter()
        .find(|f| !matches!(f.as_str(), "open" | "due" | "late_due"))
    {
        return Err(refused(
            StatusCode::BAD_REQUEST,
            format!("{bad:?} isn't a date: expected open, due or late_due"),
        ));
    }
    let file = policy_of(&state, &a).await?.file.dates;
    db::edit_overrides(&state.pool, a.id, Overrides::Dates, |old| {
        let mut dates: DateEdits = serde_json::from_str(old).map_err(|e| e.to_string())?;
        for f in &fields {
            match f.as_str() {
                "open" => dates.open = None,
                "due" => dates.due = None,
                _ => dates.late_due = None,
            }
        }
        Dates::merged(&file, &dates)
            .checked()
            .map_err(|e| format!("the dates left wouldn't work: {e}"))?;
        serde_json::to_string(&dates).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| internal("saving the dates", e))?
    .map_err(|e| refused(StatusCode::BAD_REQUEST, e))?;
    tracing::info!(by = current.user.id, %course, assignment = %slug, ?fields, "dates cleared");
    Ok(Json(dates_view(&policy_of(&state, &a).await?)).into_response())
}

/// Every test of the current build with its resolved visibility, the layer
/// that decided each field, whether students see it now and why, and every
/// layer's setting (staff).
pub async fn show_visibility(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::See).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let p = policy_of(&state, &a).await?;
    let end = p.dates().end();
    let now = OffsetDateTime::now_utc();
    let released = p.released_at.is_some();
    let tests: Vec<serde_json::Value> = p
        .tests
        .iter()
        .flat_map(|r| &r.tests)
        .map(|t| {
            let e = ResultsPolicy::explain(&p.course_visibility, &p.file.results, t);
            let field = |r: pedagog_core::recipe::policy::Resolved| {
                let shown = r.value.shown_now(end, now, released);
                serde_json::json!({
                    "value": r.value, "from": r.from.describe(),
                    "shown": shown.shown, "why": shown.why,
                })
            };
            serde_json::json!({
                "id": t.id, "node": t.node, "name": t.name,
                "score": field(e.score), "feedback": field(e.feedback),
                "stdout": field(e.stdout), "error": field(e.error),
                "layers": e.layers.iter().map(|(layer, o)| serde_json::json!({
                    "layer": layer.describe(), "score": o.score, "feedback": o.feedback,
                    "stdout": o.stdout, "error": o.error,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "released_at": p.released_at,
        "end": end.map(|e| String::from(Instant(e))),
        "course": p.course_visibility,
        "file": p.file.results,
        "tests": tests,
        "note": p.tests.is_none().then_some("no validation results to list tests from (no reference solution, or not built yet)"),
    }))
    .into_response())
}

#[derive(Deserialize)]
pub struct VisibilityBody {
    /// One test (its id, else its node); every test if absent.
    test: Option<String>,
    score: Option<Visibility>,
    feedback: Option<Visibility>,
    stdout: Option<Visibility>,
    error: Option<Visibility>,
}

/// Override visibility for every test or one, per field.
pub async fn set_visibility(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Json(body): Json<VisibilityBody>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let given = FieldOverrides {
        score: body.score,
        feedback: body.feedback,
        stdout: body.stdout,
        error: body.error,
    };
    if given.is_empty() {
        return Err(refused(
            StatusCode::BAD_REQUEST,
            "give --score, --feedback, --stdout or --error",
        ));
    }
    let p = policy_of(&state, &a).await?;
    let warning = body.test.as_ref().and_then(|t| {
        let known = p
            .tests
            .iter()
            .flat_map(|r| &r.tests)
            .any(|x| x.id.as_deref() == Some(t) || &x.node == t);
        (!known).then(|| format!("no test {t:?} in the current build's results; saved anyway"))
    });
    db::edit_overrides(&state.pool, a.id, Overrides::Visibility, |old| {
        let mut v: ResultsPolicy = serde_json::from_str(old)?;
        let entry: &mut FieldOverrides = match &body.test {
            Some(t) => v.tests.entry(t.clone()).or_default(),
            None => &mut v.visibility,
        };
        entry.score = given.score.or(entry.score);
        entry.feedback = given.feedback.or(entry.feedback);
        entry.stdout = given.stdout.or(entry.stdout);
        entry.error = given.error.or(entry.error);
        serde_json::to_string(&v)
    })
    .await
    .map_err(|e| internal("saving the visibility", e))?
    .map_err(|e| internal("saving the visibility", e))?;
    tracing::info!(by = current.user.id, %course, assignment = %slug, test = ?body.test, "visibility set");
    Ok(Json(serde_json::json!({ "warning": warning })).into_response())
}

#[derive(Deserialize)]
pub struct ClearVisibilityQuery {
    test: Option<String>,
    /// Every override, all tests and each test.
    #[serde(default)]
    all: bool,
}

/// Drop overrides: for one test, for all tests (the all-tests entry), or
/// everything (`all`).
pub async fn clear_visibility(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Query(q): Query<ClearVisibilityQuery>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    db::edit_overrides(&state.pool, a.id, Overrides::Visibility, |old| {
        let mut v: ResultsPolicy = serde_json::from_str(old)?;
        match (&q.test, q.all) {
            (_, true) => v = ResultsPolicy::default(),
            (Some(t), false) => {
                v.tests.remove(t);
            }
            (None, false) => v.visibility = FieldOverrides::default(),
        }
        serde_json::to_string(&v)
    })
    .await
    .map_err(|e| internal("saving the visibility", e))?
    .map_err(|e| internal("saving the visibility", e))?;
    tracing::info!(by = current.user.id, %course, assignment = %slug, "visibility cleared");
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Release results: `after_end` and `after_publish` fields show.
pub async fn release(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let was = db::set_released(&state.pool, a.id, true)
        .await
        .map_err(|e| internal("releasing results", e))?;
    let told = if was {
        0
    } else {
        match crate::notices::queue_release(&state, &a).await {
            Ok(told) => told,
            // Undo, so that releasing again tells them.
            Err(e) => {
                let _ = db::set_released(&state.pool, a.id, false).await;
                return Err(internal("queueing release notices", e));
            }
        }
    };
    tracing::info!(by = current.user.id, %course, assignment = %slug, told, "results released");
    Ok(Json(serde_json::json!({ "released": true, "already": was, "told": told })).into_response())
}

/// Take a release back (students may already have seen the results).
pub async fn unrelease(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let was = db::set_released(&state.pool, a.id, false)
        .await
        .map_err(|e| internal("taking the release back", e))?;
    tracing::info!(by = current.user.id, %course, assignment = %slug, "release taken back");
    Ok(Json(serde_json::json!({
        "released": false,
        "warning": was.then_some("students may already have seen the released results"),
    }))
    .into_response())
}

/// Where the course's date edits differ from an uploaded file's dates.
pub fn date_diff(file: &Dates, course: &DateEdits) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut check = |field: &str, c: Option<String>, f: Option<String>| {
        if let Some(c) = c
            && Some(&c) != f.as_ref()
        {
            out.push(serde_json::json!({ "field": field, "course": c, "file": f }));
        }
    };
    check("open", open_text(course.open), open_text(file.open));
    check("due", instant_text(course.due), instant_text(file.due));
    check(
        "late_due",
        instant_text(course.late_due),
        instant_text(file.late_due),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use pedagog_store::db::accounts::{self, Actor};
    use pedagog_store::{
        BuildSettings, CourseRole, Isolation, JoinSource, PlatformKind, ValidationStatus,
    };
    use sqlx::PgPool;

    use crate::testing::{delete, get, json, login, post, put_json, serve};

    const RESULTS: &str = r#"{"summary": {"score": 3, "total": 4}, "tests": [
        {"node": "t.py::test_push", "id": "push", "name": "push", "status": "pass", "score": 2, "max": 2,
         "visibility": {"score": "visible", "feedback": "visible", "stdout": "visible", "error": "visible"}},
        {"node": "t.py::test_stress", "name": "stress", "status": "fail", "score": 1, "max": 2,
         "visibility": {"score": "after_publish", "feedback": "hidden", "stdout": "hidden", "error": "hidden"}}]}"#;

    /// cop3502-f26 (prof teaching, ta grading, alice enrolled) with
    /// `linked-list` current: file due 2026-09-08, validated with RESULTS.
    async fn course(pool: &PgPool) -> sqlx::Result<()> {
        use pedagog_store::db::assignments::*;
        let prof = accounts::upsert_user(pool, "prof@ufl.edu", "").await?;
        accounts::create_course(pool, "cop3502-f26", "Prog 1", prof.id, &[prof.id]).await?;
        let me = Actor {
            id: prof.id,
            admin: false,
        };
        for (email, role) in [
            ("ta@ufl.edu", CourseRole::Grader),
            ("alice@ufl.edu", CourseRole::Student),
        ] {
            let u = accounts::upsert_user(pool, email, "").await?;
            accounts::add_member(pool, "cop3502-f26", u.id, role, JoinSource::Manual, me)
                .await?
                .unwrap();
        }
        let a = upsert_assignment(pool, "cop3502-f26", "linked-list", "Linked List").await?;
        let up = record_upload(pool, a.id, "tree", "k", 1).await?;
        let settings = BuildSettings {
            platform: PlatformKind::Upload,
            isolation: Isolation::Process,
            egress: r#"{"mode":"deny"}"#.into(),
            submission: "{}".into(),
        };
        let policy = r#"{"dates": {"open": "2026-09-01T00:00:00-04:00", "due": "2026-09-08T23:59:00-04:00"},
                         "results": {"visibility": {"error": "after_end"}}}"#;
        let b = start_build(
            pool,
            &NewBuild {
                assignment_id: a.id,
                upload_id: up.id,
                image_hash: "h",
                image_ref: "r",
                settings: &settings,
                policy,
            },
        )
        .await?;
        finish_build(pool, b, true, None).await?;
        start_validation(pool, b).await?;
        finish_validation(pool, b, ValidationStatus::Passed, Some(RESULTS), &[]).await?;
        Ok(())
    }

    const BASE: &str = "/courses/cop3502-f26/assignments/linked-list";

    #[sqlx::test(migrations = "../store/migrations")]
    async fn dates_are_edited_field_by_field_over_the_files(pool: PgPool) -> sqlx::Result<()> {
        course(&pool).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        let shown = json(get(&format!("{url}{BASE}/dates"), &alice).await).await;
        assert_eq!(shown["due"]["from"], "file");
        assert!(
            shown["status"].as_str().unwrap().contains("closed"),
            "{shown}"
        );
        assert_eq!(shown["late_due"], serde_json::Value::Null);

        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        let set = |token: String, body: serde_json::Value| {
            let url = url.clone();
            async move { put_json(&format!("{url}{BASE}/dates"), body, &token).await }
        };
        assert_eq!(
            set(ta, serde_json::json!({"due": "2999-01-01T00:00:00Z"}))
                .await
                .status(),
            403
        );
        let prof = login(&url, &outbox, "prof@ufl.edu").await;
        let r = set(
            prof.clone(),
            serde_json::json!({"late_due": "2999-01-01T00:00:00Z"}),
        )
        .await;
        assert_eq!(r.status(), 200);
        let r = json(r).await;
        assert_eq!(
            (r["late_due"]["from"].as_str(), r["due"]["from"].as_str()),
            (Some("course"), Some("file"))
        );
        assert_eq!(r["status"], "open, late");
        let bad = set(
            prof.clone(),
            serde_json::json!({"due": "3000-01-01T00:00:00Z"}),
        )
        .await;
        assert_eq!(bad.status(), 400, "due after late_due");
        assert_eq!(
            set(prof.clone(), serde_json::json!({"due": "2026-09-08"}))
                .await
                .status(),
            400,
            "no offset"
        );

        let cleared =
            json(delete(&format!("{url}{BASE}/dates?fields=late-due"), &prof).await).await;
        assert_eq!(cleared["late_due"], serde_json::Value::Null);
        assert_eq!(
            delete(&format!("{url}{BASE}/dates?fields=nope"), &prof)
                .await
                .status(),
            400
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn visibility_overrides_explain_themselves(pool: PgPool) -> sqlx::Result<()> {
        course(&pool).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let prof = login(&url, &outbox, "prof@ufl.edu").await;
        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}{BASE}/visibility"), &alice)
                .await
                .status(),
            403
        );

        let v = json(get(&format!("{url}{BASE}/visibility"), &prof).await).await;
        let push = &v["tests"][0];
        assert_eq!(
            (
                push["error"]["value"].as_str(),
                push["error"]["from"].as_str()
            ),
            (Some("after_end"), Some("file, all tests"))
        );
        assert_eq!(push["error"]["shown"], true, "the assignment has ended");
        let stress = &v["tests"][1];
        assert_eq!(
            stress["score"]["why"],
            "not yet: until results are released"
        );

        let r = json(
            put_json(
                &format!("{url}{BASE}/visibility"),
                serde_json::json!({"test": "t.py::test_stress", "score": "visible"}),
                &prof,
            )
            .await,
        )
        .await;
        assert_eq!(r["warning"], serde_json::Value::Null, "a known test");
        let r = json(
            put_json(
                &format!("{url}{BASE}/visibility"),
                serde_json::json!({"test": "typo", "error": "hidden"}),
                &prof,
            )
            .await,
        )
        .await;
        assert!(r["warning"].as_str().unwrap().contains("typo"));
        let v = json(get(&format!("{url}{BASE}/visibility"), &prof).await).await;
        assert_eq!(
            (
                v["tests"][1]["score"]["value"].as_str(),
                v["tests"][1]["score"]["from"].as_str()
            ),
            (Some("visible"), Some("course, this test"))
        );
        assert_eq!(
            put_json(
                &format!("{url}{BASE}/visibility"),
                serde_json::json!({"score": "sometimes"}),
                &prof
            )
            .await
            .status(),
            422,
            "not a visibility"
        );

        assert_eq!(
            delete(&format!("{url}{BASE}/visibility?all=true"), &prof)
                .await
                .status(),
            204
        );
        let v = json(get(&format!("{url}{BASE}/visibility"), &prof).await).await;
        assert_eq!(v["course"], serde_json::json!({}));
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn releasing_shows_after_publish_and_undo_warns(pool: PgPool) -> sqlx::Result<()> {
        course(&pool).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let prof = login(&url, &outbox, "prof@ufl.edu").await;
        let r = post(
            &format!("{url}{BASE}/release"),
            serde_json::json!({}),
            Some(&prof),
        )
        .await;
        assert_eq!(json(r).await["already"], false);
        let v = json(get(&format!("{url}{BASE}/visibility"), &prof).await).await;
        assert_eq!(v["tests"][1]["score"]["shown"], true);
        assert!(v["released_at"].is_string());
        let undo = json(delete(&format!("{url}{BASE}/release"), &prof).await).await;
        assert!(
            undo["warning"]
                .as_str()
                .unwrap()
                .contains("may already have seen")
        );
        let again = json(delete(&format!("{url}{BASE}/release"), &prof).await).await;
        assert_eq!(again["warning"], serde_json::Value::Null);
        Ok(())
    }

    fn dates(json: &str) -> Dates {
        serde_json::from_str(json).unwrap()
    }

    fn edits(json: &str) -> DateEdits {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_diff_lists_course_edits_the_file_disagrees_with() {
        let file = dates(r#"{"due": "2026-09-08T23:59:00-04:00"}"#);
        let course = edits(r#"{"due": "2026-09-10T23:59:00-04:00", "open": "always"}"#);
        let diff = date_diff(&file, &course);
        assert_eq!(diff.len(), 2);
        assert_eq!(diff[0]["field"], "open");
        assert_eq!(diff[0]["file"], serde_json::Value::Null);
        assert_eq!(diff[1]["course"], "2026-09-10T23:59:00-04:00");
        let same = edits(r#"{"due": "2026-09-08T23:59:00-04:00"}"#);
        assert!(date_diff(&file, &same).is_empty(), "the same as the file");
        assert!(date_diff(&file, &DateEdits::default()).is_empty());
    }
}
