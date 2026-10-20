//! Notices to students by email: after regrades (runs jobs marked `notify`),
//! when results are released, and by hand. Each is rendered with what the
//! student may see now, queued in the outbox, and sent at a bounded rate.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pedagog_core::changes::Changes;
use pedagog_core::recipe::policy::ResultsPolicy;
use pedagog_core::results::{Part, Results, TestVisibility};
use pedagog_core::view::student_view;
use pedagog_store::db::notices::{self as db, NewNotice, UnnotifiedRun};
use pedagog_store::db::{accounts, assignments as builds_db, submissions};
use pedagog_store::{Assignment, CourseRole, GradingRun, NoticeKind, RunStatus};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::assignments::find;
use crate::auth::CurrentUser;
use crate::courses::{Need, require};
use crate::error::{ApiError, internal, refused};
use crate::mail::Mail;
use crate::policy::{Policy, policy_of};
use crate::results::{NEXT_STEP, sentence};
use crate::state::AppState;

/// How often finished runs are checked for notices to queue.
const QUEUE_EVERY: Duration = Duration::from_secs(15);
/// How often the outbox is checked when idle.
const SEND_EVERY: Duration = Duration::from_secs(5);

/// Who a notice goes to, and about which course and assignment.
struct To {
    user_id: i64,
    greeting: String,
    course_name: String,
    a: Assignment,
}

impl To {
    fn about(&self) -> String {
        format!("{} ({})", self.a.name, self.course_name)
    }

    fn see_them(&self) -> String {
        format!(
            "See them: hammer results show {} -c {}",
            self.a.slug, self.a.course_id
        )
    }
}

/// An active student of the assignment's course, or nobody to tell.
async fn student(state: &AppState, a: &Assignment, user_id: i64) -> anyhow::Result<Option<To>> {
    if accounts::role_in(&state.pool, &a.course_id, user_id).await? != Some(CourseRole::Student) {
        return Ok(None);
    }
    let Some(course) = accounts::course(&state.pool, &a.course_id).await? else {
        return Ok(None);
    };
    let name = accounts::user_name(&state.pool, user_id).await?;
    Ok(Some(To {
        user_id,
        greeting: if name.is_empty() {
            "Hello".to_owned()
        } else {
            format!("Hi {name}")
        },
        course_name: course.name,
        a: a.clone(),
    }))
}

fn visibilities(p: &Policy, r: &Results) -> Vec<TestVisibility> {
    r.tests
        .iter()
        .map(|t| ResultsPolicy::resolve(&p.course_visibility, &p.file.results, t))
        .collect()
}

fn parsed(run: &GradingRun) -> Option<Results> {
    run.results
        .as_deref()
        .and_then(|r| serde_json::from_str(r).ok())
}

/// The score a student sees of a run now, as `(score, total)`.
fn shown_score(p: &Policy, r: &Results, ended: bool, released: bool) -> (f64, f64) {
    let v = student_view(r, &visibilities(p, r), ended, released);
    (v.score, v.total)
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// A regrade notice's body: the score on what's shown, before and after, and
/// the parts the student can see that changed. None when the student can't
/// see any change, unless `always`.
fn regrade_body(
    to: &To,
    p: &Policy,
    before: Option<&GradingRun>,
    after: &GradingRun,
    always: bool,
) -> Option<String> {
    let now = OffsetDateTime::now_utc();
    let (ended, released) = (p.dates().ended(now), p.released_at.is_some());
    let mut lines = vec![
        format!("{},", to.greeting),
        String::new(),
        format!(
            "Your results for {} were updated after a regrade.",
            to.about()
        ),
    ];
    if after.status == RunStatus::Error {
        lines.push(format!(
            "It couldn't be graded: {}; {NEXT_STEP}.",
            sentence(after.error_kind)
        ));
    } else if let Some(results) = parsed(after) {
        let now_score = shown_score(p, &results, ended, released);
        let was = before
            .and_then(parsed)
            .map(|r| shown_score(p, &r, ended, released));
        let changes: Changes = after
            .changes
            .as_deref()
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or_default();
        let vis = visibilities(p, &results);
        let mut counts = [(Part::Feedback, 0), (Part::Stdout, 0), (Part::Error, 0)];
        for change in &changes.changed {
            let Some((_, v)) = results
                .tests
                .iter()
                .zip(&vis)
                .find(|(t, _)| t.key() == change.test)
            else {
                continue;
            };
            if !v.score.shown(ended, released) {
                continue;
            }
            for (part, n) in counts.iter_mut() {
                if change.parts.contains(part) && v.of(*part).shown(ended, released) {
                    *n += 1;
                }
            }
        }
        let fmt = |(s, t): (f64, f64)| format!("{s}/{t}");
        let also: Vec<String> = counts
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(part, n)| {
                let name = match part {
                    Part::Feedback => "feedback",
                    Part::Stdout => "printed output",
                    _ => "errors",
                };
                format!("{name} on {}", plural(*n, "test"))
            })
            .collect();
        if was != Some(now_score) {
            lines.push(String::new());
            lines.push(match was {
                Some(w) => format!("Score on the tests shown: {} -> {}", fmt(w), fmt(now_score)),
                None => format!("Score on the tests shown: {}", fmt(now_score)),
            });
        }
        if was == Some(now_score) && also.is_empty() && !always {
            return None;
        }
        if !also.is_empty() {
            if was == Some(now_score) {
                lines.push(String::new());
            }
            lines.push(format!("Also changed: {}", also.join(", ")));
        }
    }
    lines.push(String::new());
    lines.push(to.see_them());
    Some(lines.join("\n"))
}

/// The notice for one finished run, if its student should get one.
async fn regrade_notice(
    state: &AppState,
    run: &UnnotifiedRun,
) -> anyhow::Result<Option<(To, String)>> {
    let (Some(user), Some(a)) = (
        run.user_id,
        builds_db::assignment_by_id(&state.pool, run.assignment_id).await?,
    ) else {
        return Ok(None);
    };
    let Some(to) = student(state, &a, user).await? else {
        return Ok(None);
    };
    let Some(after) = submissions::get_run(&state.pool, run.run_id).await? else {
        return Ok(None);
    };
    let before =
        submissions::previous_graded_run(&state.pool, run.submission_id, run.run_id).await?;
    let p = policy_of(state, &a)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.error))?;
    Ok(regrade_body(&to, &p, before.as_ref(), &after, run.always).map(|body| (to, body)))
}

/// Queue notices for finished runs marked `notify` (each run once).
pub async fn queue_regrades(state: &AppState) -> anyhow::Result<usize> {
    let mut queued = 0;
    for run in db::unnotified_runs(&state.pool, 100).await? {
        let notice = regrade_notice(state, &run).await?;
        let subject = notice
            .as_ref()
            .map(|(to, _)| format!("{} results updated", to.a.slug));
        let new = notice
            .as_ref()
            .zip(subject.as_deref())
            .map(|((to, body), subject)| NewNotice {
                user_id: to.user_id,
                assignment_id: Some(to.a.id),
                kind: NoticeKind::Regrade,
                subject,
                body,
            });
        queued += usize::from(new.is_some());
        db::settle_run(&state.pool, run.run_id, new.as_ref()).await?;
    }
    Ok(queued)
}

/// Every active student with a submission to the assignment (or the one asked for).
async fn recipients(
    state: &AppState,
    a: &Assignment,
    only: Option<i64>,
) -> anyhow::Result<Vec<(To, i64)>> {
    let mut out = Vec::new();
    for c in submissions::counting(&state.pool, a.id).await? {
        if only.is_some_and(|u| u != c.user_id) {
            continue;
        }
        if let Some(to) = student(state, a, c.user_id).await? {
            out.push((to, c.submission_id));
        }
    }
    Ok(out)
}

/// Tell students an assignment's results are released (no numbers).
pub async fn queue_release(state: &AppState, a: &Assignment) -> anyhow::Result<usize> {
    let subject = format!("{} results released", a.slug);
    let mut n = 0;
    for (to, _) in recipients(state, a, None).await? {
        let body = [
            format!("{},", to.greeting),
            String::new(),
            format!("Results for {} are released.", to.about()),
            String::new(),
            to.see_them(),
        ]
        .join("\n");
        db::queue(
            &state.pool,
            &NewNotice {
                user_id: to.user_id,
                assignment_id: Some(a.id),
                kind: NoticeKind::Release,
                subject: &subject,
                body: &body,
            },
        )
        .await?;
        n += 1;
    }
    Ok(n)
}

#[derive(Deserialize)]
pub struct NotifyQuery {
    #[serde(rename = "for")]
    for_: Option<String>,
}

/// Staff tell students (all, or one) their results were updated, with what
/// they may see now.
pub async fn notify(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
    Query(q): Query<NotifyQuery>,
) -> Result<Response, ApiError> {
    let access = require(&state, &course, &current, Need::Manage).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let only = match &q.for_ {
        Some(email) => Some(
            accounts::user_by_email(&state.pool, email)
                .await
                .map_err(|e| internal("reading the student", e))?
                .ok_or_else(|| refused(StatusCode::NOT_FOUND, format!("no user {email}")))?
                .id,
        ),
        None => None,
    };
    let p = policy_of(&state, &a).await?;
    let now = OffsetDateTime::now_utc();
    let (ended, released) = (p.dates().ended(now), p.released_at.is_some());
    let subject = format!("{} results updated", a.slug);
    let mut queued = 0;
    for (to, submission) in recipients(&state, &a, only)
        .await
        .map_err(|e| internal("finding the students", e))?
    {
        let run = submissions::latest_runs(&state.pool, &[submission])
            .await
            .map_err(|e| internal("reading the run", e))?
            .into_iter()
            .next();
        let mut lines = vec![
            format!("{},", to.greeting),
            String::new(),
            format!("Your results for {} were updated.", to.about()),
        ];
        if let Some(r) = run.as_ref().and_then(parsed) {
            let (s, t) = shown_score(&p, &r, ended, released);
            lines.push(String::new());
            lines.push(format!("Score on the tests shown: {s}/{t}"));
        }
        lines.push(String::new());
        lines.push(to.see_them());
        db::queue(
            &state.pool,
            &NewNotice {
                user_id: to.user_id,
                assignment_id: Some(a.id),
                kind: NoticeKind::Manual,
                subject: &subject,
                body: &lines.join("\n"),
            },
        )
        .await
        .map_err(|e| internal("queueing the notice", e))?;
        queued += 1;
    }
    if only.is_some() && queued == 0 {
        return Err(refused(
            StatusCode::NOT_FOUND,
            "that student has no submission to tell them about",
        ));
    }
    tracing::info!(by = current.user.id, %course, assignment = %slug, queued, "notices queued by hand");
    Ok(Json(serde_json::json!({ "queued": queued })).into_response())
}

/// Send what's due, at most `per_second`; returns how many were sent.
pub async fn send_due(state: &AppState, per_second: u32) -> anyhow::Result<usize> {
    let gap = Duration::from_secs(1) / per_second.max(1);
    let mut sent = 0;
    for n in db::lease_due(&state.pool, i64::from(per_second.max(1)) * 10).await? {
        let mail = Mail {
            to: n.email.clone(),
            subject: n.subject.clone(),
            body: n.body.clone(),
        };
        match state.mailer.send(mail).await {
            Ok(()) => {
                db::sent(&state.pool, n.id).await?;
                sent += 1;
            }
            Err(e) => {
                let gave_up = db::failed(&state.pool, n.id, n.attempts, &e.to_string()).await?;
                if gave_up {
                    tracing::error!(notice = n.id, to = %n.email, error = %e, "gave up sending a notice");
                } else {
                    tracing::warn!(notice = n.id, to = %n.email, error = %e, "sending a notice failed; will retry");
                }
            }
        }
        tokio::time::sleep(gap).await;
    }
    Ok(sent)
}

/// Background: queue regrade notices and send the outbox, for the life of
/// the service.
pub async fn run(state: AppState) {
    let mut last_queue = tokio::time::Instant::now() - QUEUE_EVERY;
    loop {
        if last_queue.elapsed() >= QUEUE_EVERY {
            if let Err(e) = queue_regrades(&state).await {
                tracing::error!(error = %e, "queueing regrade notices failed");
            }
            last_queue = tokio::time::Instant::now();
        }
        match send_due(&state, state.config.mail_per_second).await {
            Ok(0) => tokio::time::sleep(SEND_EVERY).await,
            Ok(_) => {}
            Err(e) => {
                tracing::error!(error = %e, "sending notices failed");
                tokio::time::sleep(SEND_EVERY).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pedagog_store::db::accounts::{self, Actor};
    use pedagog_store::db::assignments::*;
    use pedagog_store::db::notices as db;
    use pedagog_store::db::submissions::{self as subs, NewSubmission, Owner, RunOutcome};
    use pedagog_store::{
        BuildSettings, CourseRole, Isolation, JoinSource, PlatformKind, SubmissionSource,
        ValidationStatus,
    };
    use sqlx::PgPool;

    use super::*;
    use crate::testing::{delete, json, login, post, serve_full};

    fn results(score: f64, stdout: &str, hidden_stdout: &str) -> String {
        serde_json::json!({
            "run": {"status": "complete", "output": ""},
            "summary": {"score": score, "total": 3.0},
            "tests": [
                {"node": "t.py::a", "name": "a", "status": "pass", "score": score, "max": 2.0, "stdout": stdout,
                 "visibility": {"score": "visible", "feedback": "visible", "stdout": "visible", "error": "after_end"}},
                {"node": "t.py::b", "name": "b", "status": "pass", "score": 0.0, "max": 1.0, "stdout": hidden_stdout,
                 "visibility": {"score": "hidden", "feedback": "hidden", "stdout": "hidden", "error": "hidden"}}
            ]
        })
        .to_string()
    }

    /// cop3502-f26 with prof, ta (grader), alice and bob (students) and an
    /// open `linked-list`; returns the assignment and its build.
    async fn course(pool: &PgPool) -> sqlx::Result<(i64, i64)> {
        let prof = accounts::upsert_user(pool, "prof@ufl.edu", "Prof").await?;
        accounts::create_course(pool, "cop3502-f26", "Programming 1", prof.id, &[prof.id]).await?;
        let me = Actor {
            id: prof.id,
            admin: false,
        };
        for (email, name, role) in [
            ("ta@ufl.edu", "Tara", CourseRole::Grader),
            ("alice@ufl.edu", "Alice", CourseRole::Student),
            ("bob@ufl.edu", "", CourseRole::Student),
            ("cat@ufl.edu", "Cat", CourseRole::Student),
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

    /// A graded submission by `email`, then a regrade of it to `second`,
    /// marked `notify`.
    async fn regraded(
        pool: &PgPool,
        a: i64,
        b: i64,
        email: &str,
        second: &str,
    ) -> sqlx::Result<()> {
        let u = accounts::user_by_email(pool, email).await?.unwrap();
        let acc = subs::record_submission(
            pool,
            &NewSubmission {
                assignment_id: a,
                owner: Owner::User(u.id),
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
        subs::start_grading(&mut conn, acc.run_id, "g", Isolation::Process).await?;
        let first = results(1.0, "", "");
        subs::finish_run(
            pool,
            acc.run_id,
            &RunOutcome {
                score: Some((1.0, 3.0)),
                results: Some(&first),
                ..Default::default()
            },
        )
        .await?;
        let batch: i64 = sqlx::query_scalar(
            "INSERT INTO regrade_batches (assignment_id, build_id, notify, runs) VALUES ($1, $2, 'changed', 1)
             ON CONFLICT (build_id) DO UPDATE SET runs = regrade_batches.runs + 1 RETURNING id",
        )
        .bind(a)
        .bind(b)
        .fetch_one(pool)
        .await?;
        let run: i64 = sqlx::query_scalar(
            "INSERT INTO grading_runs (submission_id, build_id, reason, regrade_batch_id, status)
             VALUES ($1, $2, 'regrade', $3, 'grading') RETURNING id",
        )
        .bind(acc.submission_id)
        .bind(b)
        .bind(batch)
        .fetch_one(pool)
        .await?;
        let changes = serde_json::to_string(&pedagog_core::changes::compare(
            serde_json::from_str(&first).ok().as_ref(),
            serde_json::from_str(second).ok().as_ref(),
        ))
        .unwrap();
        subs::finish_run(
            pool,
            run,
            &RunOutcome {
                score: Some((2.0, 3.0)),
                results: Some(second),
                changes: Some(&changes),
                notify: true,
                ..Default::default()
            },
        )
        .await?;
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn regrades_tell_students_what_they_can_see_changed(pool: PgPool) -> sqlx::Result<()> {
        let (a, b) = course(&pool).await?;
        let better = results(2.0, "a new hint\n", "");
        regraded(&pool, a, b, "alice@ufl.edu", &better).await?;
        regraded(&pool, a, b, "bob@ufl.edu", &better).await?;
        regraded(
            &pool,
            a,
            b,
            "cat@ufl.edu",
            &results(1.0, "", "hidden hint\n"),
        )
        .await?;
        let bob = accounts::user_by_email(&pool, "bob@ufl.edu")
            .await?
            .unwrap();
        let prof = accounts::user_by_email(&pool, "prof@ufl.edu")
            .await?
            .unwrap();
        accounts::drop_member(
            &pool,
            "cop3502-f26",
            bob.id,
            Actor {
                id: prof.id,
                admin: false,
            },
        )
        .await?
        .unwrap();
        let (state, outbox) = crate::testing::state(pool.clone());

        assert_eq!(
            queue_regrades(&state).await.unwrap(),
            1,
            "dropped students aren't told"
        );
        assert_eq!(queue_regrades(&state).await.unwrap(), 0, "each run once");
        assert_eq!(send_due(&state, 1000).await.unwrap(), 1);
        let mail = outbox.lock().unwrap()[0].clone();
        assert_eq!(
            (mail.to.as_str(), mail.subject.as_str()),
            ("alice@ufl.edu", "linked-list results updated")
        );
        assert!(mail.body.starts_with("Hi Alice,"), "{}", mail.body);
        assert!(
            mail.body.contains("Score on the tests shown: 1/2 -> 2/2"),
            "{}",
            mail.body
        );
        assert!(
            mail.body.contains("Also changed: printed output on 1 test"),
            "{}",
            mail.body
        );
        assert!(
            mail.body
                .contains("hammer results show linked-list -c cop3502-f26")
        );
        assert!(
            !mail.body.contains("/3"),
            "the hidden test's points stay hidden"
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn release_and_manual_notices(pool: PgPool) -> sqlx::Result<()> {
        let (a, b) = course(&pool).await?;
        regraded(&pool, a, b, "alice@ufl.edu", &results(2.0, "", "")).await?;
        let (url, outbox, _) = serve_full(pool.clone()).await;
        let base = format!("{url}/courses/cop3502-f26/assignments/linked-list");
        let prof = login(&url, &outbox, "prof@ufl.edu").await;
        let ta = login(&url, &outbox, "ta@ufl.edu").await;

        let r = json(
            post(
                &format!("{base}/release"),
                serde_json::json!({}),
                Some(&prof),
            )
            .await,
        )
        .await;
        assert_eq!(r["told"], 1);
        let again = json(
            post(
                &format!("{base}/release"),
                serde_json::json!({}),
                Some(&prof),
            )
            .await,
        )
        .await;
        assert_eq!(again["told"], 0, "only the first release tells");
        delete(&format!("{base}/release"), &prof).await;

        assert_eq!(
            post(&format!("{base}/notify"), serde_json::json!({}), Some(&ta))
                .await
                .status(),
            403
        );
        let r = json(
            post(
                &format!("{base}/notify?for=alice@ufl.edu"),
                serde_json::json!({}),
                Some(&prof),
            )
            .await,
        )
        .await;
        assert_eq!(r["queued"], 1);
        assert_eq!(
            post(
                &format!("{base}/notify?for=ta@ufl.edu"),
                serde_json::json!({}),
                Some(&prof)
            )
            .await
            .status(),
            404
        );
        let notices = db::for_assignment(&pool, a).await?;
        let kinds: Vec<_> = notices.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, [NoticeKind::Release, NoticeKind::Manual]);
        assert!(notices[0].body.contains("are released") && !notices[0].body.contains("Score"));
        assert!(
            notices[1].body.contains("Score on the tests shown: 2/2"),
            "{}",
            notices[1].body
        );
        Ok(())
    }
}
