//! Submissions. Students submit their own within the assignment's window;
//! staff (instructors, graders, admins) submit their own work, or a student's
//! on their behalf, whatever the dates. The files go to jobs; reads come from
//! Postgres. Until results are filtered by visibility, students see status and
//! lateness but no scores.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pedagog_core::recipe::policy::{Acceptance, Dates, Instant, spoken};
use pedagog_store::db::{accounts, assignments as builds_db, queue, submissions as db};
use pedagog_store::{Assignment, CourseRole, QueuedRun, Submission, User};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::assignments::find;
use crate::auth::CurrentUser;
use crate::courses::{Access, Need, require};
use crate::error::{ApiError, internal, refused};
use crate::state::AppState;

/// Course staff, and admins acting in the course.
pub(crate) fn is_staff(access: &Access) -> bool {
    access.role != Some(CourseRole::Student)
}

/// The dates in effect: the course's edits over the current build's file dates.
async fn dates_of(state: &AppState, a: &Assignment) -> Result<Dates, ApiError> {
    Ok(crate::policy::policy_of(state, a).await?.dates())
}

fn parse_time(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// "late by …" for a student's submission counted at `counted_at`.
pub(crate) fn lateness(dates: &Dates, s: &Submission) -> Option<String> {
    if s.staff {
        return None;
    }
    dates.late_by(parse_time(&s.counted_at)?).map(spoken)
}

/// URL query values: everything but unreserved characters escaped.
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

#[derive(Deserialize)]
pub struct SubmitQuery {
    /// A student's email: submit on their behalf (staff).
    #[serde(rename = "for")]
    for_: Option<String>,
    /// With `for`: `now` (default), `last` (their latest counted time), or a time.
    at: Option<String>,
}

/// Who a submission is for and how it counts, as jobs records it.
struct Plan {
    owner: User,
    by: Option<i64>,
    staff: bool,
    /// RFC 3339; `None` counts it now.
    at: Option<String>,
}

async fn plan(
    state: &AppState,
    access: &Access,
    current: &CurrentUser,
    a: &Assignment,
    q: &SubmitQuery,
) -> Result<Plan, ApiError> {
    let Some(email) = &q.for_ else {
        if q.at.is_some() {
            return Err(refused(
                StatusCode::BAD_REQUEST,
                "a submission time goes with submitting for a student",
            ));
        }
        if !is_staff(access)
            && let Acceptance::Closed(why) = dates_of(state, a)
                .await?
                .acceptance(OffsetDateTime::now_utc())
        {
            return Err(refused(
                StatusCode::FORBIDDEN,
                format!("{} isn't taking submissions: {why}", a.slug),
            ));
        }
        return Ok(Plan {
            owner: current.user.clone(),
            by: None,
            staff: is_staff(access),
            at: None,
        });
    };
    if !is_staff(access) {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "only course staff can submit for someone else",
        ));
    }
    let not_student = || {
        refused(
            StatusCode::NOT_FOUND,
            format!("{email} isn't a student of {}", a.course_id),
        )
    };
    let owner = accounts::user_by_email(&state.pool, email)
        .await
        .map_err(|e| internal("reading the student", e))?
        .ok_or_else(not_student)?;
    if accounts::role_in(&state.pool, &a.course_id, owner.id)
        .await
        .map_err(|e| internal("reading the student's role", e))?
        != Some(CourseRole::Student)
    {
        return Err(not_student());
    }
    let at = match q.at.as_deref().map(str::trim) {
        None | Some("now") => None,
        Some("last") => Some(
            db::submissions_for_user(&state.pool, a.id, owner.id)
                .await
                .map_err(|e| internal("reading their submissions", e))?
                .first()
                .map(|s| s.counted_at.clone())
                .ok_or_else(|| {
                    refused(
                        StatusCode::CONFLICT,
                        format!("{email} has no earlier submission to take the time from"),
                    )
                })?,
        ),
        Some(t) => {
            let when =
                Instant::try_from(t.to_owned()).map_err(|e| refused(StatusCode::BAD_REQUEST, e))?;
            if when.0 > OffsetDateTime::now_utc() {
                return Err(refused(
                    StatusCode::BAD_REQUEST,
                    format!("{t} is in the future"),
                ));
            }
            Some(t.to_owned())
        }
    };
    Ok(Plan {
        owner,
        by: Some(current.user.id),
        staff: false,
        at,
    })
}

/// The body is the multipart upload; jobs normalizes, stores and queues it.
pub async fn submit(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Query(q): Query<SubmitQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Member).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    if a.current_build_id.is_none() {
        return Err(refused(
            StatusCode::CONFLICT,
            format!("{slug} has no usable build yet"),
        ));
    }
    let plan = plan(&state, &access, &current, &a, &q).await?;
    let mut params = vec![format!("user={}", plan.owner.id)];
    if let Some(by) = plan.by {
        params.push(format!("by={by}"));
    }
    if plan.staff {
        params.push("staff=true".to_owned());
    }
    if let Some(at) = &plan.at {
        params.push(format!("at={}", encode(at)));
    }
    let url = format!(
        "{}/internal/courses/{}/assignments/{}/submissions?{}",
        state.config.jobs_url,
        a.course_id,
        a.slug,
        params.join("&")
    );
    let mut req = state.jobs.post(url).body(body);
    if let Some(ct) = headers.get(header::CONTENT_TYPE) {
        req = req.header(header::CONTENT_TYPE, ct);
    }
    let reply = req.send().await.map_err(|e| {
        tracing::error!(error = %e, "jobs is unreachable");
        refused(
            StatusCode::BAD_GATEWAY,
            "the grading service is unreachable",
        )
    })?;
    let status = StatusCode::from_u16(reply.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = reply
        .text()
        .await
        .map_err(|e| internal("reading the grading service's reply", e))?;
    if !status.is_success() {
        return Ok((status, [(header::CONTENT_TYPE, "application/json")], text).into_response());
    }
    let mut out: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| internal("reading the grading service's reply", e))?;
    let counted = plan
        .at
        .as_deref()
        .and_then(parse_time)
        .unwrap_or_else(OffsetDateTime::now_utc);
    let dates = dates_of(&state, &a).await?;
    out["for"] = serde_json::json!(plan.owner.email);
    out["submitted_by"] = serde_json::json!(plan.by.map(|_| current.user.email.clone()));
    out["staff"] = serde_json::json!(plan.staff);
    out["counted_at"] = serde_json::json!(counted.format(&Rfc3339).unwrap_or_default());
    out["late"] = serde_json::json!(if plan.staff {
        None
    } else {
        dates.late_by(counted).map(spoken)
    });
    out["due"] = serde_json::json!(dates.due.map(String::from));
    tracing::info!(
        by = current.user.id, %course, assignment = %slug, owner = plan.owner.id,
        staff = plan.staff, "submission accepted"
    );
    Ok((StatusCode::CREATED, Json(out)).into_response())
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(rename = "for")]
    for_: Option<String>,
}

/// Your submissions to an assignment, latest first; staff may ask for a
/// student's (`?for=`).
pub async fn list(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Member).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let staff = is_staff(&access);
    let owner = match &q.for_ {
        None => current.user.clone(),
        Some(email) if staff => accounts::user_by_email(&state.pool, email)
            .await
            .map_err(|e| internal("reading the student", e))?
            .ok_or_else(|| refused(StatusCode::NOT_FOUND, format!("no user {email}")))?,
        Some(_) => {
            return Err(refused(
                StatusCode::FORBIDDEN,
                "only course staff can see someone else's submissions",
            ));
        }
    };
    let dates = dates_of(&state, &a).await?;
    let counting = db::counting_for(&state.pool, a.id, owner.id)
        .await
        .map_err(|e| internal("reading what counts", e))?;
    let mut out = Vec::new();
    for s in db::submissions_for_user(&state.pool, a.id, owner.id)
        .await
        .map_err(|e| internal("reading the submissions", e))?
    {
        let latest = db::runs_for_submission(&state.pool, s.id)
            .await
            .map_err(|e| internal("reading the runs", e))?
            .into_iter()
            .next();
        let mut view = serde_json::json!({
            "id": s.id,
            "counted_at": s.counted_at,
            "late": lateness(&dates, &s),
            "submitted_by": s.submitted_by,
            "staff": s.staff,
            "status": latest.as_ref().map(|r| r.status.to_string()),
            "counts": counting.as_ref().is_some_and(|c| c.submission_id == s.id),
            "activated_by": counting
                .as_ref()
                .filter(|c| c.submission_id == s.id)
                .and_then(|c| c.activated_by.clone()),
        });
        if staff {
            view["created_at"] = serde_json::json!(s.created_at);
            view["score"] = serde_json::json!(latest.as_ref().and_then(|r| r.score));
            view["total"] = serde_json::json!(latest.as_ref().and_then(|r| r.total));
        }
        out.push(view);
    }
    Ok(Json(out).into_response())
}

/// One submission and its runs: yours, or any in the course for staff.
pub async fn show(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, id)): Path<(String, i64)>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Member).await?;
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
    let a = builds_db::assignment_by_id(&state.pool, s.assignment_id)
        .await
        .map_err(|e| internal("reading the assignment", e))?
        .filter(|a| a.course_id == access.course.id)
        .ok_or_else(missing)?;
    let staff = is_staff(&access);
    if !staff && s.user_id != Some(current.user.id) {
        return Err(missing());
    }
    let json = |text: &Option<String>| {
        text.as_deref()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
    };
    let mut runs = Vec::new();
    for r in db::runs_for_submission(&state.pool, s.id)
        .await
        .map_err(|e| internal("reading the runs", e))?
    {
        let queue_position = if r.status == pedagog_store::RunStatus::Queued {
            queue::position(&state.pool, QueuedRun::Grading(r.id))
                .await
                .map_err(|e| internal("reading the queue", e))?
        } else {
            None
        };
        let mut run = serde_json::json!({
            "id": r.id,
            "reason": r.reason.to_string(),
            "status": r.status.to_string(),
            "queue_position": queue_position,
        });
        if staff {
            run["build_id"] = serde_json::json!(r.build_id);
            run["score"] = serde_json::json!(r.score);
            run["total"] = serde_json::json!(r.total);
            run["problems"] = serde_json::json!(r.problems);
            run["error"] = serde_json::json!(r.error);
            run["log_tail"] = serde_json::json!(r.log_tail);
            run["changes"] = serde_json::json!(json(&r.changes));
            run["results"] = serde_json::json!(json(&r.results));
        }
        runs.push(run);
    }
    let dates = dates_of(&state, &a).await?;
    let mut out = serde_json::json!({
        "id": s.id,
        "course": a.course_id,
        "assignment": a.slug,
        "for": s.user_email.clone().or(s.student_id.clone()),
        "submitted_by": s.submitted_by,
        "staff": s.staff,
        "counted_at": s.counted_at,
        "late": lateness(&dates, &s),
        "receipt": json(&Some(s.receipt.clone())),
        "runs": runs,
    });
    if staff {
        out["created_at"] = serde_json::json!(s.created_at);
        out["build_id"] = serde_json::json!(s.build_id);
    }
    Ok(Json(out).into_response())
}

#[cfg(test)]
mod tests {
    use pedagog_store::db::accounts::{self, Actor};
    use pedagog_store::db::assignments::*;
    use pedagog_store::db::submissions::{NewSubmission, record_submission};
    use pedagog_store::{
        BuildSettings, CourseRole, Isolation, JoinSource, PlatformKind, ValidationStatus,
    };
    use sqlx::PgPool;

    use crate::testing::{JobsSeen, get, json, login, serve_full};

    /// cop3502-f26 (prof, ta grading, alice and bob students) with `linked-list`
    /// current under `policy`; returns the assignment and its build.
    async fn course(pool: &PgPool, policy: &str) -> sqlx::Result<(i64, i64)> {
        let prof = accounts::upsert_user(pool, "prof@ufl.edu", "").await?;
        accounts::create_course(pool, "cop3502-f26", "Prog 1", prof.id, &[prof.id]).await?;
        let me = Actor {
            id: prof.id,
            admin: false,
        };
        for (email, role) in [
            ("ta@ufl.edu", CourseRole::Grader),
            ("alice@ufl.edu", CourseRole::Student),
            ("bob@ufl.edu", CourseRole::Student),
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
        finish_validation(pool, b, ValidationStatus::Skipped, None, &[]).await?;
        Ok((a.id, b))
    }

    /// Late window open: due long ago, late submissions until far away.
    const LATE_OPEN: &str = r#"{"dates": {"open": "2020-01-01T00:00:00Z", "due": "2020-02-01T00:00:00Z", "late_due": "2999-01-01T00:00:00Z"}}"#;
    const CLOSED: &str =
        r#"{"dates": {"open": "2020-01-01T00:00:00Z", "due": "2020-02-01T00:00:00Z"}}"#;

    async fn send(url: &str, token: &str, q: &str) -> reqwest::Response {
        let body = "--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.py\"\r\n\r\nx\r\n--b--\r\n";
        reqwest::Client::new()
            .post(format!(
                "{url}/courses/cop3502-f26/assignments/linked-list/submissions{q}"
            ))
            .bearer_auth(token)
            .header("content-type", "multipart/form-data; boundary=b")
            .body(body)
            .send()
            .await
            .unwrap()
    }

    fn last(jobs: &JobsSeen) -> String {
        jobs.lock().unwrap().last().unwrap().0.clone()
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn students_submit_in_the_window_and_late_is_said(pool: PgPool) -> sqlx::Result<()> {
        course(&pool, LATE_OPEN).await?;
        let (url, outbox, jobs) = serve_full(pool.clone()).await;
        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        let alice_id = accounts::user_by_email(&pool, "alice@ufl.edu")
            .await?
            .unwrap()
            .id;
        let r = send(&url, &alice, "").await;
        assert_eq!(r.status(), 201);
        let r = json(r).await;
        assert!(r["late"].as_str().unwrap().contains("days"), "{r}");
        assert_eq!(
            (r["for"].as_str(), r["staff"].as_bool()),
            (Some("alice@ufl.edu"), Some(false))
        );
        assert_eq!(
            last(&jobs),
            format!(
                "/internal/courses/cop3502-f26/assignments/linked-list/submissions?user={alice_id}"
            )
        );
        assert_eq!(
            send(&url, &alice, "?at=last").await.status(),
            400,
            "times are for staff"
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn closed_assignments_refuse_students_but_not_staff(pool: PgPool) -> sqlx::Result<()> {
        course(&pool, CLOSED).await?;
        let (url, outbox, jobs) = serve_full(pool.clone()).await;
        let alice = login(&url, &outbox, "alice@ufl.edu").await;
        let r = send(&url, &alice, "").await;
        assert_eq!(r.status(), 403);
        assert!(
            json(r).await["error"]
                .as_str()
                .unwrap()
                .contains("deadline has passed")
        );
        assert!(jobs.lock().unwrap().is_empty());

        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        let r = json(send(&url, &ta, "").await).await;
        assert_eq!(
            (r["staff"].as_bool(), r["late"].clone()),
            (Some(true), serde_json::Value::Null)
        );
        assert!(last(&jobs).ends_with("&staff=true"));
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn staff_submit_on_behalf_at_a_chosen_time(pool: PgPool) -> sqlx::Result<()> {
        let (a, b) = course(&pool, CLOSED).await?;
        let alice = accounts::user_by_email(&pool, "alice@ufl.edu")
            .await?
            .unwrap();
        let (url, outbox, jobs) = serve_full(pool.clone()).await;
        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        let ta_id = accounts::user_by_email(&pool, "ta@ufl.edu")
            .await?
            .unwrap()
            .id;

        let r = send(&url, &ta, "?for=alice@ufl.edu&at=last").await;
        assert_eq!(r.status(), 409, "no earlier submission");
        record_submission(
            &pool,
            &NewSubmission {
                assignment_id: a,
                user_id: alice.id,
                submitted_by: None,
                staff: false,
                counted_at: Some("2020-01-31T23:00:00Z"),
                build_id: b,
                source: pedagog_store::SubmissionSource::Upload,
                tree_hash: "t",
                object_key: "k",
                size_bytes: 1,
                receipt: "{}",
            },
        )
        .await?;
        let r = json(send(&url, &ta, "?for=alice@ufl.edu&at=last").await).await;
        assert_eq!(
            (
                r["counted_at"].as_str(),
                r["late"].clone(),
                r["submitted_by"].as_str()
            ),
            (
                Some("2020-01-31T23:00:00Z"),
                serde_json::Value::Null,
                Some("ta@ufl.edu")
            ),
            "on time, though the deadline has passed"
        );
        assert_eq!(
            last(&jobs),
            format!(
                "/internal/courses/cop3502-f26/assignments/linked-list/submissions?user={}&by={ta_id}&at=2020-01-31T23%3A00%3A00.000000Z",
                alice.id
            )
        );
        let r =
            json(send(&url, &ta, "?for=alice@ufl.edu&at=2020-01-31T23:00:00-04:00").await).await;
        assert_eq!(r["late"], "3 hours");
        assert_eq!(
            send(&url, &ta, "?for=alice@ufl.edu&at=2999-01-01T00:00:00Z")
                .await
                .status(),
            400
        );
        assert_eq!(
            send(&url, &ta, "?for=prof@ufl.edu").await.status(),
            404,
            "not a student"
        );
        let alice_token = login(&url, &outbox, "alice@ufl.edu").await;
        assert_eq!(
            send(&url, &alice_token, "?for=bob@ufl.edu").await.status(),
            403
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn students_see_their_own_without_scores(pool: PgPool) -> sqlx::Result<()> {
        let (a, b) = course(&pool, LATE_OPEN).await?;
        let alice = accounts::user_by_email(&pool, "alice@ufl.edu")
            .await?
            .unwrap();
        let prof = accounts::user_by_email(&pool, "prof@ufl.edu")
            .await?
            .unwrap();
        let sub = record_submission(
            &pool,
            &NewSubmission {
                assignment_id: a,
                user_id: alice.id,
                submitted_by: Some(prof.id),
                staff: false,
                counted_at: Some("2020-02-01T02:14:00Z"),
                build_id: b,
                source: pedagog_store::SubmissionSource::Upload,
                tree_hash: "t",
                object_key: "k",
                size_bytes: 1,
                receipt: r#"{"files": ["a.py"]}"#,
            },
        )
        .await?;
        let (url, outbox, _) = serve_full(pool.clone()).await;
        let alice_token = login(&url, &outbox, "alice@ufl.edu").await;
        let mine = json(
            get(
                &format!("{url}/courses/cop3502-f26/assignments/linked-list/submissions"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(
            (
                mine[0]["late"].as_str(),
                mine[0]["submitted_by"].as_str(),
                mine[0]["status"].as_str()
            ),
            (
                Some("2 hours 14 minutes"),
                Some("prof@ufl.edu"),
                Some("queued")
            )
        );
        assert!(mine[0].get("score").is_none() && mine[0].get("created_at").is_none());
        let one = json(
            get(
                &format!(
                    "{url}/courses/cop3502-f26/submissions/{}",
                    sub.submission_id
                ),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(one["runs"][0]["status"], "queued");
        assert!(one["runs"][0].get("results").is_none());
        assert_eq!(one["receipt"]["files"][0], "a.py");

        let bob = login(&url, &outbox, "bob@ufl.edu").await;
        assert_eq!(
            get(
                &format!(
                    "{url}/courses/cop3502-f26/submissions/{}",
                    sub.submission_id
                ),
                &bob
            )
            .await
            .status(),
            404,
            "someone else's"
        );
        assert_eq!(
            get(
                &format!("{url}/courses/cop3502-f26/assignments/linked-list/submissions?for=alice@ufl.edu"),
                &bob
            )
            .await
            .status(),
            403
        );
        let ta = login(&url, &outbox, "ta@ufl.edu").await;
        let theirs = json(
            get(
                &format!("{url}/courses/cop3502-f26/assignments/linked-list/submissions?for=alice@ufl.edu"),
                &ta,
            )
            .await,
        )
        .await;
        assert!(
            theirs[0].get("created_at").is_some(),
            "staff see when it really arrived"
        );
        Ok(())
    }
}
