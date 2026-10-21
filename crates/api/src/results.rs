//! Results: the staff list (one row per student who submitted, with the
//! cumulative score columns), one student's results in full for staff, a
//! student's own results filtered by visibility, and activating the
//! submission that counts.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pedagog_core::api::{MyResults, RunState, Shown};
use pedagog_core::recipe::policy::ResultsPolicy;
use pedagog_core::results::{Results, TestVisibility};
use pedagog_core::view::{Stage, student_view, tallies};
use pedagog_store::db::{accounts, assignments as builds_db, submissions as db};
use pedagog_store::{CourseRole, ErrorKind, GradingRun, Membership, RunStatus, Submission};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::assignments::find;
use crate::auth::CurrentUser;
use crate::courses::{Need, require};
use crate::error::{ApiError, internal, refused};
use crate::policy::{Policy, policy_of};
use crate::state::AppState;
use crate::submissions::{is_staff, lateness};
use crate::users::{CANDIDATES, Pick, pick};

/// What students are told about a run that couldn't be graded.
pub fn sentence(kind: Option<ErrorKind>) -> &'static str {
    match kind {
        Some(ErrorKind::Load) => "the tests couldn't run with your code",
        Some(ErrorKind::Timeout) => "it ran past the time limit",
        Some(ErrorKind::OutOfMemory) => "it ran out of memory",
        Some(ErrorKind::OutOfDisk) => "it wrote more to disk than allowed",
        Some(ErrorKind::Crash) => "the test run stopped unexpectedly",
        _ => "something went wrong on our side",
    }
}

/// What a student can do about a run that couldn't be graded.
pub const NEXT_STEP: &str =
    "check your files and submit again; if problems persist, contact course staff";

/// Each test's resolved visibility, in order.
fn visibilities(p: &Policy, results: &Results) -> Vec<TestVisibility> {
    results
        .tests
        .iter()
        .map(|t| ResultsPolicy::resolve(&p.course_visibility, &p.file.results, t))
        .collect()
}

fn parsed(run: &GradingRun) -> Option<Results> {
    run.results
        .as_deref()
        .and_then(|r| serde_json::from_str(r).ok())
}

pub(crate) fn run_state(s: RunStatus) -> RunState {
    match s {
        RunStatus::Queued => RunState::Queued,
        RunStatus::Grading => RunState::Grading,
        RunStatus::Graded => RunState::Graded,
        RunStatus::Error => RunState::Error,
    }
}

/// The score a student sees now of a graded run.
pub(crate) fn shown(p: &Policy, run: &GradingRun) -> Option<Shown> {
    if run.status != RunStatus::Graded {
        return None;
    }
    let results = parsed(run)?;
    let view = student_view(
        &results,
        &visibilities(p, &results),
        p.dates().ended(OffsetDateTime::now_utc()),
        p.released_at.is_some(),
    );
    Some(Shown {
        score: view.score,
        total: view.total,
    })
}

/// The stage a student sees now.
fn stage_now(p: &Policy) -> Stage {
    Stage::now(
        p.dates().ended(OffsetDateTime::now_utc()),
        p.released_at.is_some(),
    )
}

fn stages_json(p: &Policy, results: &Results) -> serde_json::Value {
    let now = stage_now(p);
    serde_json::json!(
        tallies(results, &visibilities(p, results))
            .into_iter()
            .map(|(stage, t)| serde_json::json!({
                "stage": stage, "score": t.score, "total": t.total,
                "passed": t.passed, "tests": t.tests, "student_sees": stage == now,
            }))
            .collect::<Vec<_>>()
    )
}

#[derive(Deserialize)]
pub struct ListQuery {
    search: Option<String>,
    #[serde(default)]
    dropped: bool,
    #[serde(default)]
    staff: bool,
}

/// One row per student who submitted (staff and dropped members on request),
/// sorted by name then email.
pub async fn list(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::See).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let p = policy_of(&state, &a).await?;
    let dates = p.dates();
    let members = accounts::members(&state.pool, &a.course_id, None, true)
        .await
        .map_err(|e| internal("reading the members", e))?;
    let by_user: HashMap<i64, &Membership> = members.iter().map(|m| (m.user.id, m)).collect();
    let active_students = members
        .iter()
        .filter(|m| m.role == CourseRole::Student && m.dropped_at.is_none())
        .count();
    let search = q.search.as_deref().map(str::to_lowercase);
    let counting = db::counting(&state.pool, a.id)
        .await
        .map_err(|e| internal("reading what counts", e))?;
    let counting: Vec<_> = counting
        .into_iter()
        .filter(|c| {
            let Some(m) = by_user.get(&c.user_id) else {
                return q.staff; // an admin who isn't a member
            };
            let wanted = match (m.role, m.dropped_at.is_some()) {
                (CourseRole::Student, false) => true,
                (CourseRole::Student, true) => q.dropped,
                (_, dropped) => q.staff && (!dropped || q.dropped),
            };
            wanted
                && search.as_ref().is_none_or(|s| {
                    m.user.email.contains(s) || m.user.name.to_lowercase().contains(s)
                })
        })
        .collect();
    let ids: Vec<i64> = counting.iter().map(|c| c.submission_id).collect();
    let subs: HashMap<i64, Submission> = db::submissions_by_id(&state.pool, &ids)
        .await
        .map_err(|e| internal("reading the submissions", e))?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
    let runs: HashMap<i64, GradingRun> = db::latest_runs(&state.pool, &ids)
        .await
        .map_err(|e| internal("reading the runs", e))?
        .into_iter()
        .map(|r| (r.submission_id, r))
        .collect();
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut submitted = 0;
    for c in &counting {
        let Some(s) = subs.get(&c.submission_id) else {
            continue;
        };
        let m = by_user.get(&c.user_id);
        let run = runs.get(&s.id);
        if m.is_some_and(|m| m.role == CourseRole::Student && m.dropped_at.is_none()) {
            submitted += 1;
        }
        let results = run.and_then(parsed);
        rows.push(serde_json::json!({
            "email": s.user_email,
            "name": m.map(|m| m.user.name.clone()).unwrap_or_default(),
            "role": m.map(|m| m.role.to_string()),
            "dropped": m.is_some_and(|m| m.dropped_at.is_some()),
            "submission": s.id,
            "activated_by": c.activated_by,
            "late": lateness(&dates, s),
            "status": run.map(|r| r.status.to_string()),
            "error_kind": run.and_then(|r| r.error_kind).map(|k| k.to_string()),
            "stages": results.as_ref().map(|r| stages_json(&p, r)),
        }));
    }
    rows.sort_by(|x, y| {
        let key = |v: &serde_json::Value| {
            (
                v["name"].as_str().unwrap_or("").to_lowercase(),
                v["email"].as_str().unwrap_or("").to_owned(),
            )
        };
        let (kx, ky) = (key(x), key(y));
        // Unnamed rows after named ones.
        (kx.0.is_empty(), kx).cmp(&(ky.0.is_empty(), ky))
    });
    Ok(Json(serde_json::json!({
        "rows": rows,
        "submitted": submitted,
        "students": active_students,
        "student_stage": stage_now(&p),
    }))
    .into_response())
}

#[derive(Deserialize)]
pub struct ShowQuery {
    /// Another of the student's submissions than the one that counts.
    submission: Option<i64>,
}

/// The submission to show for `user`: one asked for (theirs, in this
/// assignment), or the one that counts.
async fn chosen(
    state: &AppState,
    assignment: i64,
    user: i64,
    asked: Option<i64>,
) -> Result<(Submission, Option<String>, bool), ApiError> {
    let counting = db::counting_for(&state.pool, assignment, user)
        .await
        .map_err(|e| internal("reading what counts", e))?;
    let id = match (asked, &counting) {
        (Some(id), _) => id,
        (None, Some(c)) => c.submission_id,
        (None, None) => return Err(refused(StatusCode::NOT_FOUND, "no submissions yet")),
    };
    let s = db::get_submission(&state.pool, id)
        .await
        .map_err(|e| internal("reading the submission", e))?
        .filter(|s| s.assignment_id == assignment && s.user_id == Some(user))
        .ok_or_else(|| refused(StatusCode::NOT_FOUND, format!("no submission {id} here")))?;
    let counts = counting.as_ref().is_some_and(|c| c.submission_id == s.id);
    let by = counting.and_then(|c| c.activated_by).filter(|_| counts);
    Ok((s, by, counts))
}

async fn latest_run(state: &AppState, s: &Submission) -> Result<Option<GradingRun>, ApiError> {
    Ok(db::latest_runs(&state.pool, &[s.id])
        .await
        .map_err(|e| internal("reading the run", e))?
        .into_iter()
        .next())
}

/// One student's results in full, with what they see of each part (staff).
pub async fn show(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug, query)): Path<(String, String, String)>,
    Query(q): Query<ShowQuery>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::See).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let found = accounts::find_members(&state.pool, &a.course_id, &query, CANDIDATES + 1)
        .await
        .map_err(|e| internal("finding the student", e))?;
    let wanted = query.trim().to_lowercase();
    let member = match pick(found, |m| m.user.email == wanted) {
        Pick::One(m) => m,
        Pick::Many(ms, more) => {
            return Ok(Json(serde_json::json!({
                "matches": ms.iter().map(|m| serde_json::json!({
                    "email": m.user.email, "name": m.user.name, "role": m.role.to_string(),
                })).collect::<Vec<_>>(),
                "more": more,
            }))
            .into_response());
        }
        Pick::None => {
            return Err(refused(
                StatusCode::NOT_FOUND,
                format!("no member of {course} matches {query:?}"),
            ));
        }
    };
    let (s, activated_by, counts) = chosen(&state, a.id, member.user.id, q.submission).await?;
    let run = latest_run(&state, &s).await?;
    let p = policy_of(&state, &a).await?;
    let now = OffsetDateTime::now_utc();
    let (end, released) = (p.dates().end(), p.released_at.is_some());
    let results = run.as_ref().and_then(parsed);
    let tests = results.as_ref().map(|r| {
        r.tests
            .iter()
            .map(|t| {
                let e = ResultsPolicy::explain(&p.course_visibility, &p.file.results, t);
                let part = |r: pedagog_core::recipe::policy::Resolved| {
                    let shown = r.value.shown_now(end, now, released);
                    serde_json::json!({ "value": r.value, "shown": shown.shown, "why": shown.why })
                };
                serde_json::json!({
                    "name": t.name, "node": t.node, "id": t.id, "status": t.status,
                    "score": t.score, "max": t.max,
                    "visibility": {
                        "score": part(e.score), "feedback": part(e.feedback),
                        "stdout": part(e.stdout), "error": part(e.error),
                    },
                    "items": t.items, "feedback": t.feedback,
                    "stdout": t.stdout, "error": t.error, "trace": t.trace,
                })
            })
            .collect::<Vec<_>>()
    });
    let told = results.as_ref().map(|r| {
        student_view(r, &visibilities(&p, r), p.dates().ended(now), released).hidden_until
    });
    Ok(Json(serde_json::json!({
        "email": member.user.email,
        "name": member.user.name,
        "submission": s.id,
        "counts": counts,
        "activated_by": activated_by,
        "counted_at": s.counted_at,
        "created_at": s.created_at,
        "submitted_by": s.submitted_by,
        "late": lateness(&p.dates(), &s),
        "run": run.as_ref().map(|r| serde_json::json!({
            "id": r.id, "status": r.status.to_string(),
            "error_kind": r.error_kind.map(|k| k.to_string()),
            "error": r.error, "problems": r.problems, "log_tail": r.log_tail,
        })),
        "run_output": results.as_ref().and_then(|r| r.run.as_ref()).map(|r| r.output.clone()),
        "stages": results.as_ref().map(|r| stages_json(&p, r)),
        "student_is_told": told,
        "tests": tests,
    }))
    .into_response())
}

/// Your own results, as you may see them now.
pub async fn mine(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Query(q): Query<ShowQuery>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Member).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let (s, activated_by, counts) = chosen(&state, a.id, current.user.id, q.submission).await?;
    let run = latest_run(&state, &s).await?;
    let p = policy_of(&state, &a).await?;
    let now = OffsetDateTime::now_utc();
    let ended = p.dates().ended(now);
    let released = p.released_at.is_some();
    let (status, view, why) = match &run {
        Some(r) if r.status == RunStatus::Graded => {
            let results = parsed(r);
            let view = results
                .as_ref()
                .map(|res| student_view(res, &visibilities(&p, res), ended, released));
            (RunState::Graded, view, None)
        }
        Some(r) => (
            run_state(r.status),
            None,
            (r.status == RunStatus::Error).then(|| sentence(r.error_kind)),
        ),
        None => (RunState::Queued, None, None),
    };
    let queue_position = match &run {
        Some(r) if r.status == RunStatus::Queued => {
            pedagog_store::db::queue::position(&state.pool, pedagog_store::QueuedRun::Grading(r.id))
                .await
                .map_err(|e| internal("reading the queue", e))?
        }
        _ => None,
    };
    Ok(Json(MyResults {
        submission: s.id,
        counts,
        activated: activated_by.is_some(),
        late: lateness(&p.dates(), &s),
        counted_at: s.counted_at,
        submitted_by: s.submitted_by,
        status,
        queue_position,
        why: why.map(str::to_owned),
        next: why.map(|_| NEXT_STEP.to_owned()),
        results: view,
    })
    .into_response())
}

/// The submission's owner, if it's a user's in this course (staff only).
async fn owned(
    state: &AppState,
    current: &CurrentUser,
    course: &str,
    id: i64,
) -> Result<Submission, ApiError> {
    let access = require(state, course, current, Need::See).await?;
    if !is_staff(&access) {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "only course staff can do that",
        ));
    }
    let missing = || {
        refused(
            StatusCode::NOT_FOUND,
            format!("no submission {id} in {course}"),
        )
    };
    let s = db::get_submission(&state.pool, id)
        .await
        .map_err(|e| internal("reading the submission", e))?
        .ok_or_else(missing)?;
    builds_db::assignment_by_id(&state.pool, s.assignment_id)
        .await
        .map_err(|e| internal("reading the assignment", e))?
        .filter(|a| a.course_id == access.course.id)
        .ok_or_else(missing)?;
    if s.user_id.is_none() {
        return Err(missing());
    }
    Ok(s)
}

/// Make an earlier submission count (until a newer one arrives).
pub async fn activate(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, id)): Path<(String, i64)>,
) -> Result<Response, ApiError> {
    let s = owned(&state, &current, &course, id).await?;
    db::activate(&state.pool, &s, current.user.id)
        .await
        .map_err(|e| internal("activating the submission", e))?;
    tracing::info!(
        by = current.user.id,
        submission = id,
        "submission activated"
    );
    Ok(Json(serde_json::json!({ "submission": id, "for": s.user_email })).into_response())
}

/// Back to the latest submission counting.
pub async fn deactivate(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, id)): Path<(String, i64)>,
) -> Result<Response, ApiError> {
    let s = owned(&state, &current, &course, id).await?;
    let cleared = db::deactivate(&state.pool, s.assignment_id, s.user_id.unwrap_or_default())
        .await
        .map_err(|e| internal("clearing the activation", e))?;
    Ok(Json(serde_json::json!({ "cleared": cleared, "for": s.user_email })).into_response())
}

#[cfg(test)]
mod tests {
    use pedagog_store::db::accounts::{self, Actor};
    use pedagog_store::db::assignments::*;
    use pedagog_store::db::submissions::{self as db, NewSubmission, RunOutcome};
    use pedagog_store::{
        BuildSettings, CourseRole, ErrorKind, Isolation, JoinSource, PlatformKind,
        SubmissionSource, ValidationStatus,
    };
    use sqlx::PgPool;

    use crate::testing::{delete, get, json, login, post, serve};
    use pedagog_core::api::{
        MyCourses, MyResults, RunState, Shown, StudentAssignment, StudentAssignmentPage,
        SubmissionRow, Window,
    };

    const RESULTS: &str = r#"{"run": {"status": "complete", "output": ""},
      "summary": {"score": 6.0, "total": 9.0},
      "tests": [
        {"node": "t.py::test_push", "id": "push", "name": "push", "status": "pass", "score": 2.0, "max": 2.0,
         "items": [{"id": "push", "name": "push", "score": 2.0, "max": 2.0, "clamp": [0.0, 2.0],
                    "steps": [{"result": "pass", "points": 2.0, "worth": 2.0, "reason": "pushes"}]}],
         "stdout": "pushed 3\n", "error": "", "trace": "push ...",
         "visibility": {"score": "visible", "feedback": "visible", "stdout": "visible", "error": "visible"}},
        {"node": "t.py::test_stress", "name": "stress", "status": "fail", "score": 3.0, "max": 4.0,
         "error": "assert 3 == 4", "trace": "stress ...",
         "visibility": {"score": "after_publish", "feedback": "visible", "stdout": "visible", "error": "visible"}},
        {"node": "t.py::test_edge", "name": "edge", "status": "fail", "score": 1.0, "max": 3.0,
         "visibility": {"score": "hidden", "feedback": "hidden", "stdout": "hidden", "error": "hidden"}}
      ]}"#;

    /// cop3502-f26 (prof, ta grading, alice and bob enrolled) with `linked-list`
    /// current and open-ended; returns the assignment and its build.
    async fn course(pool: &PgPool) -> sqlx::Result<(i64, i64)> {
        let prof = accounts::upsert_user(pool, "prof@ufl.edu", "Prof").await?;
        accounts::create_course(pool, "cop3502-f26", "Prog 1", prof.id, &[prof.id]).await?;
        let me = Actor {
            id: prof.id,
            admin: false,
        };
        for (email, name, role) in [
            ("ta@ufl.edu", "Tara", CourseRole::Grader),
            ("alice@ufl.edu", "Alice Zed", CourseRole::Student),
            ("bob@ufl.edu", "Bob Abel", CourseRole::Student),
            ("carol@ufl.edu", "Carol", CourseRole::Student),
        ] {
            let u = accounts::upsert_user(pool, email, name).await?;
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
        let b = start_build(
            pool,
            &NewBuild {
                assignment_id: a.id,
                upload_id: up.id,
                image_hash: "h",
                image_ref: "r",
                settings: &settings,
                policy: r#"{"dates": {"open": "always"}}"#,
            },
        )
        .await?;
        finish_build(pool, b, true, None).await?;
        finish_validation(pool, b, ValidationStatus::Skipped, None, &[]).await?;
        Ok((a.id, b))
    }

    /// A graded (or errored) submission by `email`.
    async fn graded(
        pool: &PgPool,
        a: i64,
        b: i64,
        email: &str,
        kind: Option<ErrorKind>,
    ) -> sqlx::Result<i64> {
        let u = accounts::user_by_email(pool, email).await?.unwrap();
        let acc = db::record_submission(
            pool,
            &NewSubmission {
                assignment_id: a,
                user_id: u.id,
                submitted_by: None,
                staff: false,
                counted_at: None,
                build_id: b,
                source: SubmissionSource::Upload,
                tree_hash: "t",
                object_key: "k",
                size_bytes: 1,
                receipt: "{}",
            },
        )
        .await?;
        let mut conn = pool.acquire().await?;
        db::start_grading(&mut conn, acc.run_id, "g", Isolation::Process).await?;
        let problems = vec!["the tests couldn't all run: SyntaxError".to_owned()];
        let outcome = match kind {
            None => RunOutcome {
                score: Some((6.0, 9.0)),
                results: Some(RESULTS),
                ..Default::default()
            },
            Some(k) => RunOutcome {
                problems: &problems,
                error_kind: Some(k),
                ..Default::default()
            },
        };
        db::finish_run(pool, acc.run_id, &outcome).await?;
        Ok(acc.submission_id)
    }

    const BASE: &str = "/courses/cop3502-f26/assignments/linked-list";

    #[sqlx::test(migrations = "../store/migrations")]
    async fn staff_list_students_who_submitted_with_cumulative_columns(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let (a, b) = course(&pool).await?;
        graded(&pool, a, b, "alice@ufl.edu", None).await?;
        graded(&pool, a, b, "bob@ufl.edu", Some(ErrorKind::Load)).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        let list = json(get(&format!("{url}{BASE}/results"), &ta).await).await;
        assert_eq!(
            (list["submitted"].as_i64(), list["students"].as_i64()),
            (Some(2), Some(3))
        );
        let rows = list["rows"].as_array().unwrap();
        assert_eq!(rows[0]["name"], "Alice Zed", "sorted by name");
        assert_eq!(rows[1]["error_kind"], "load");
        let stages = rows[0]["stages"].as_array().unwrap();
        assert_eq!(
            (stages[0]["score"].as_f64(), stages[0]["total"].as_f64()),
            (Some(2.0), Some(2.0))
        );
        assert_eq!(stages[0]["student_sees"], true, "not released, no end");
        assert_eq!(
            (stages[2]["score"].as_f64(), stages[3]["total"].as_f64()),
            (Some(5.0), Some(9.0))
        );
        let one = json(get(&format!("{url}{BASE}/results?search=bob"), &ta).await).await;
        assert_eq!(one["rows"].as_array().unwrap().len(), 1);

        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}{BASE}/results"), &alice).await.status(),
            403
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn students_see_their_results_filtered(pool: PgPool) -> sqlx::Result<()> {
        let (a, b) = course(&pool).await?;
        graded(&pool, a, b, "alice@ufl.edu", None).await?;
        graded(&pool, a, b, "bob@ufl.edu", Some(ErrorKind::Load)).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        let mine = json(get(&format!("{url}{BASE}/my-results"), &alice).await).await;
        let r = &mine["results"];
        assert_eq!(
            r["tests"].as_array().unwrap().len(),
            1,
            "only scores shown now"
        );
        assert_eq!(
            (r["score"].as_f64(), r["total"].as_f64()),
            (Some(2.0), Some(2.0))
        );
        assert_eq!(r["tests"][0]["feedback"][0]["steps"][0]["reason"], "pushes");
        assert_eq!(r["hidden_until"], serde_json::json!(["release"]));
        assert_eq!(r["hidden_never"], true);
        assert!(!mine.to_string().contains("assert 3 == 4") && !mine.to_string().contains("trace"));

        let bob = login(&url, &outbox, "bob@ufl.edu").await;
        let mine = json(get(&format!("{url}{BASE}/my-results"), &bob).await).await;
        assert_eq!(
            (mine["status"].as_str(), mine["why"].as_str()),
            (Some("error"), Some("the tests couldn't run with your code"))
        );
        assert!(
            !mine.to_string().contains("SyntaxError"),
            "problems stay with staff"
        );

        let carol = login(&url, &outbox, "carol@ufl.edu").await;
        assert_eq!(
            get(&format!("{url}{BASE}/my-results"), &carol)
                .await
                .status(),
            404
        );

        // Release: the after_publish test shows.
        let prof = login(&url, &outbox, "prof@ufl.edu").await;
        post(
            &format!("{url}{BASE}/release"),
            serde_json::json!({}),
            Some(&prof),
        )
        .await;
        let mine = json(get(&format!("{url}{BASE}/my-results"), &alice).await).await;
        assert_eq!(mine["results"]["tests"].as_array().unwrap().len(), 2);
        assert_eq!(mine["results"]["tests"][1]["error"], "assert 3 == 4");
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn students_see_their_standing_in_lists_and_on_home(pool: PgPool) -> sqlx::Result<()> {
        let (a, b) = course(&pool).await?;
        graded(&pool, a, b, "alice@ufl.edu", None).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        let list: Vec<StudentAssignment> = serde_json::from_value(
            json(get(&format!("{url}/courses/cop3502-f26/assignments"), &alice).await).await,
        )
        .unwrap();
        assert_eq!((list[0].window, list[0].accepting), (Window::Open, true));
        let mine = list[0].mine.clone().unwrap();
        assert_eq!(
            (mine.status, mine.shown, mine.submissions),
            (
                RunState::Graded,
                Some(Shown {
                    score: 2.0,
                    total: 2.0
                }),
                1
            ),
            "only the score shown now"
        );

        let page: StudentAssignmentPage =
            serde_json::from_value(json(get(&format!("{url}{BASE}"), &alice).await).await).unwrap();
        let rules = page.rules.unwrap();
        assert_eq!(
            (rules.max_bytes, rules.unpack_archives, rules.strip_wrapper),
            (Some(1 << 20), true, true),
            "the api's ceiling, with no limit of the assignment's own"
        );

        let history: Vec<SubmissionRow> = serde_json::from_value(
            json(get(&format!("{url}{BASE}/submissions"), &alice).await).await,
        )
        .unwrap();
        assert_eq!(
            (history[0].shown, history[0].score),
            (
                Some(Shown {
                    score: 2.0,
                    total: 2.0
                }),
                None
            ),
            "the shown score, never the raw one"
        );

        // Open with no deadline: on Home until submitted.
        let home = |token: String| {
            let url = url.clone();
            async move {
                serde_json::from_value::<MyCourses>(
                    json(get(&format!("{url}/me/courses"), &token).await).await,
                )
                .unwrap()
            }
        };
        assert!(home(alice).await.courses[0].items.is_empty());
        let carol = login(&url, &outbox, "carol@ufl.edu").await;
        let courses = home(carol).await.courses;
        assert_eq!(
            (courses[0].id.as_str(), courses[0].items[0].slug.as_str()),
            ("cop3502-f26", "linked-list")
        );
        assert!(courses[0].items[0].mine.is_none());
        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        assert!(home(ta).await.courses[0].items.is_empty(), "staff: later");

        // A queued submission says where it is in line.
        let carol_id = accounts::user_by_email(&pool, "carol@ufl.edu")
            .await?
            .unwrap()
            .id;
        db::record_submission(
            &pool,
            &NewSubmission {
                assignment_id: a,
                user_id: carol_id,
                submitted_by: None,
                staff: false,
                counted_at: None,
                build_id: b,
                source: SubmissionSource::Upload,
                tree_hash: "t",
                object_key: "k",
                size_bytes: 1,
                receipt: "{}",
            },
        )
        .await?;
        let carol = login(&url, &outbox, "carol@ufl.edu").await;
        let mine: MyResults = serde_json::from_value(
            json(get(&format!("{url}{BASE}/my-results"), &carol).await).await,
        )
        .unwrap();
        assert_eq!(mine.status, RunState::Queued);
        // (Not first: the fixture never releases the entries queued before it.)
        assert!(mine.queue_position.is_some());
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn staff_see_everything_and_activate_a_submission(pool: PgPool) -> sqlx::Result<()> {
        let (a, b) = course(&pool).await?;
        let first = graded(&pool, a, b, "alice@ufl.edu", None).await?;
        let second = graded(&pool, a, b, "alice@ufl.edu", Some(ErrorKind::Timeout)).await?;
        let (url, outbox) = serve(pool.clone()).await;
        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        let full = json(get(&format!("{url}{BASE}/results/alice"), &ta).await).await;
        assert_eq!(
            (full["submission"].as_i64(), full["counts"].as_bool()),
            (Some(second), Some(true))
        );
        assert_eq!(full["run"]["error_kind"], "timeout");

        let r = post(
            &format!("{url}/courses/cop3502-f26/submissions/{first}/activate"),
            serde_json::json!({}),
            Some(&ta),
        )
        .await;
        assert_eq!(r.status(), 200);
        let full = json(get(&format!("{url}{BASE}/results/alice"), &ta).await).await;
        assert_eq!(
            (full["submission"].as_i64(), full["activated_by"].as_str()),
            (Some(first), Some("ta@ufl.edu"))
        );
        let stress = &full["tests"][1];
        assert_eq!(
            stress["visibility"]["score"]["why"],
            "not yet: until results are released"
        );
        assert_eq!(
            (stress["error"].as_str(), stress["trace"].as_str()),
            (Some("assert 3 == 4"), Some("stress ..."))
        );
        assert_eq!(full["student_is_told"], serde_json::json!(["release"]));

        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        let mine = json(get(&format!("{url}{BASE}/my-results"), &alice).await).await;
        assert_eq!(
            (mine["submission"].as_i64(), mine["activated"].as_bool()),
            (Some(first), Some(true))
        );
        let theirs = json(get(&format!("{url}{BASE}/submissions"), &alice).await).await;
        assert_eq!(
            theirs
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s["counts"] == true)
                .count(),
            1
        );
        assert_eq!(
            post(
                &format!("{url}/courses/cop3502-f26/submissions/{second}/activate"),
                serde_json::json!({}),
                Some(&alice)
            )
            .await
            .status(),
            403
        );
        let r = json(
            delete(
                &format!("{url}/courses/cop3502-f26/submissions/{first}/activate"),
                &ta,
            )
            .await,
        )
        .await;
        assert_eq!(r["cleared"], true);
        let full = json(get(&format!("{url}{BASE}/results/alice"), &ta).await).await;
        assert_eq!(full["submission"].as_i64(), Some(second));
        Ok(())
    }
}
