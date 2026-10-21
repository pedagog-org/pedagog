//! What students see of their assignments: each one's window and their own
//! standing (the submission that counts, its shown score), the rules a
//! submission must meet, and Home's "open or due soon".

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use pedagog_core::api::{
    DatesView, HomeCourse, Mine, MyCourses, StudentAssignment, SubmitRules, Window,
};
use pedagog_core::recipe::policy::{Dates, Open};
use pedagog_core::recipe::submission::SubmissionRules;
use pedagog_store::db::{accounts, assignments as builds_db, submissions as db};
use pedagog_store::{Assignment, CourseRole};
use time::{Duration, OffsetDateTime};

use crate::auth::CurrentUser;
use crate::error::{ApiError, internal};
use crate::policy::{Policy, policy_of, status_text};
use crate::results::{run_state, shown};
use crate::state::AppState;
use crate::submissions::lateness;

/// How far ahead Home looks for due dates and openings.
const SOON: Duration = Duration::days(7);

fn dates_view(d: &Dates) -> DatesView {
    DatesView {
        open: d.open.map(String::from),
        due: d.due.map(String::from),
        late_due: d.late_due.map(String::from),
    }
}

/// `a` as the student `user` sees it now.
pub async fn assignment(
    state: &AppState,
    a: &Assignment,
    user: i64,
) -> Result<(StudentAssignment, Policy), ApiError> {
    let p = policy_of(state, a).await?;
    let now = OffsetDateTime::now_utc();
    let dates = p.dates();
    let window = dates.window(now);
    let submissions = db::submissions_for_user(&state.pool, a.id, user)
        .await
        .map_err(|e| internal("reading your submissions", e))?;
    let counting = db::counting_for(&state.pool, a.id, user)
        .await
        .map_err(|e| internal("reading what counts", e))?;
    let mine = match counting.and_then(|c| submissions.iter().find(|s| s.id == c.submission_id)) {
        None => None,
        Some(s) => {
            let run = db::latest_runs(&state.pool, &[s.id])
                .await
                .map_err(|e| internal("reading the run", e))?
                .into_iter()
                .next();
            Some(Mine {
                submission: s.id,
                status: run
                    .as_ref()
                    .map_or(pedagog_core::api::RunState::Queued, |r| run_state(r.status)),
                shown: run.as_ref().and_then(|r| shown(&p, r)),
                late: lateness(&dates, s),
                submissions: submissions.len(),
            })
        }
    };
    let view = StudentAssignment {
        slug: a.slug.clone(),
        name: a.name.clone(),
        dates: dates_view(&dates),
        window,
        status: status_text(&dates, now),
        accepting: matches!(window, Window::Open | Window::OpenLate)
            && a.current_build_id.is_some(),
        mine,
    };
    Ok((view, p))
}

/// The current build's submission rules, for checking files before sending.
pub async fn rules(state: &AppState, a: &Assignment) -> Result<Option<SubmitRules>, ApiError> {
    let Some(build) = a.current_build_id else {
        return Ok(None);
    };
    let Some(settings) = builds_db::build_by_id(&state.pool, build)
        .await
        .map_err(|e| internal("reading the build", e))?
        .and_then(|b| b.settings)
    else {
        return Ok(None);
    };
    let r: SubmissionRules = serde_json::from_str(&settings.submission)
        .map_err(|e| internal("reading the submission rules", e))?;
    let ceiling = state.config.max_upload_bytes as u64;
    Ok(Some(SubmitRules {
        required: r.required.into_iter().map(String::from).collect(),
        max_bytes: Some(r.max_size.map_or(ceiling, |m| m.0.min(ceiling))),
        unpack_archives: r.archives == pedagog_core::recipe::submission::Archives::Unpack,
        strip_wrapper: r.strip == pedagog_core::recipe::submission::Strip::Auto,
    }))
}

/// Where an assignment sorts on Home, if it shows there: open and due within
/// a week (or open with no deadline, until submitted), open late, or opening
/// within a week; soonest first.
pub fn soon(dates: &Dates, a: &StudentAssignment, now: OffsetDateTime) -> Option<OffsetDateTime> {
    match a.window {
        Window::OpenLate => dates.late_due.map(|i| i.0),
        Window::Open => match dates.due {
            Some(due) => (due.0 - now <= SOON).then_some(due.0),
            None => a.mine.is_none().then_some(now + SOON),
        },
        Window::Upcoming => match dates.open {
            Some(Open::At(open)) => (open.0 - now <= SOON).then_some(open.0),
            _ => None,
        },
        Window::NoDates | Window::Closed => None,
    }
}

/// `GET /api/me/courses`: your courses, each with what needs you.
pub async fn home(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    let now = OffsetDateTime::now_utc();
    let mut courses = Vec::new();
    for (c, role) in accounts::courses_for(&state.pool, current.user.id)
        .await
        .map_err(|e| internal("reading your courses", e))?
    {
        let mut items = Vec::new();
        if role == CourseRole::Student {
            for a in builds_db::assignments_in(&state.pool, &c.id)
                .await
                .map_err(|e| internal("reading the assignments", e))?
            {
                let (view, p) = assignment(&state, &a, current.user.id).await?;
                if let Some(at) = soon(&p.dates(), &view, now) {
                    items.push((at, view));
                }
            }
        }
        items.sort_by_key(|(at, _)| *at);
        courses.push(HomeCourse {
            id: c.id,
            name: c.name,
            role: role.to_string(),
            items: items.into_iter().map(|(_, v)| v).collect(),
        });
    }
    Ok(Json(MyCourses { courses }).into_response())
}

#[cfg(test)]
mod tests {
    use pedagog_core::recipe::policy::Instant;

    use super::*;

    fn at(s: &str) -> OffsetDateTime {
        Instant::try_from(s.to_owned()).unwrap().0
    }

    fn item(window: Window, submitted: bool) -> StudentAssignment {
        StudentAssignment {
            slug: "a".into(),
            name: "A".into(),
            dates: DatesView::default(),
            window,
            status: String::new(),
            accepting: true,
            mine: submitted.then_some(Mine {
                submission: 1,
                status: pedagog_core::api::RunState::Graded,
                shown: None,
                late: None,
                submissions: 1,
            }),
        }
    }

    #[test]
    fn home_shows_what_is_due_or_opening_within_a_week() {
        let now = at("2026-10-20T12:00:00Z");
        let due = |d: &str| Dates {
            open: Some(Open::Always),
            due: Some(Instant(at(d))),
            late_due: None,
        };
        let open = item(Window::Open, false);
        assert_eq!(
            soon(&due("2026-10-23T23:59:00Z"), &open, now),
            Some(at("2026-10-23T23:59:00Z"))
        );
        assert_eq!(
            soon(&due("2026-11-23T23:59:00Z"), &open, now),
            None,
            "a month out"
        );

        let late = Dates {
            late_due: Some(Instant(at("2026-10-30T23:59:00Z"))),
            ..due("2026-10-19T23:59:00Z")
        };
        assert_eq!(
            soon(&late, &item(Window::OpenLate, true), now),
            Some(at("2026-10-30T23:59:00Z")),
            "open late shows however far off"
        );

        let opening = |d: &str| Dates {
            open: Some(Open::At(Instant(at(d)))),
            due: None,
            late_due: None,
        };
        let upcoming = item(Window::Upcoming, false);
        assert!(soon(&opening("2026-10-26T09:00:00Z"), &upcoming, now).is_some());
        assert!(soon(&opening("2026-11-26T09:00:00Z"), &upcoming, now).is_none());

        let always = Dates {
            open: Some(Open::Always),
            due: None,
            late_due: None,
        };
        assert!(
            soon(&always, &open, now).is_some(),
            "no deadline, not submitted"
        );
        assert!(soon(&always, &item(Window::Open, true), now).is_none());
        assert!(soon(&always, &item(Window::Closed, false), now).is_none());
    }
}
