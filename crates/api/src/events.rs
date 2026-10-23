//! Live updates for assignment pages, as server-sent events.
//!
//! A trigger announces every run's and build's status change on Postgres
//! (`db::events::CHANNEL`); one listener per api replica fans them out to the
//! open streams. `GET /api/courses/{c}/assignments/{a}/events` streams only
//! what its viewer may see: a student their own submissions' runs (status and
//! queue position, never a score: they fetch their results, filtered, when a
//! run ends); staff every run (with the student and score) and the
//! assignment's builds. Every few seconds it refreshes queue positions, and
//! every 15 s it re-checks the session and membership, closing the stream
//! when either has ended. A stream that fell behind is told to `resync`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use pedagog_store::QueuedRun;
use pedagog_store::db;
use serde::Deserialize;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::broadcast;

use crate::assignments::find;
use crate::auth::CurrentUser;
use crate::courses::{Need, require};
use crate::error::ApiError;
use crate::state::AppState;
use crate::submissions::is_staff;

const TICK: Duration = Duration::from_secs(5);
/// Ticks between session and membership checks.
const CHECK_EVERY: u32 = 3;

/// A status change, as the trigger announces it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Change {
    Run {
        run: i64,
        submission: i64,
        assignment: i64,
        user: Option<i64>,
        email: Option<String>,
        status: String,
        score: Option<f64>,
        total: Option<f64>,
    },
    Build {
        build: i64,
        assignment: i64,
        build_status: String,
        validation_status: String,
    },
    /// The assignment's current build changed.
    Current { assignment: i64, build: Option<i64> },
}

/// Fans changes out to every open stream in this replica.
#[derive(Clone)]
pub struct Hub(broadcast::Sender<Change>);

impl Hub {
    pub fn new() -> Self {
        Self(broadcast::channel(1024).0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.0.subscribe()
    }
}

/// Background: listen for changes for the life of the service, reconnecting
/// after a dropped connection.
pub async fn listen(pool: PgPool, hub: Hub) {
    loop {
        if let Err(e) = relay(&pool, &hub).await {
            tracing::warn!(error = %e, "listening for live updates failed; reconnecting");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn relay(pool: &PgPool, hub: &Hub) -> sqlx::Result<()> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen(db::events::CHANNEL).await?;
    loop {
        let n = listener.recv().await?;
        match serde_json::from_str::<Change>(n.payload()) {
            // No open stream is fine.
            Ok(change) => drop(hub.0.send(change)),
            Err(e) => {
                tracing::warn!(error = %e, payload = n.payload(), "an unreadable live update")
            }
        }
    }
}

/// Who's watching, and what they may see.
#[derive(Clone, Debug)]
struct Viewer {
    user: i64,
    staff: bool,
    assignment: i64,
}

/// What `viewer` gets of `change`: an event name and its JSON, or nothing.
fn shown(viewer: &Viewer, change: &Change) -> Option<(&'static str, serde_json::Value)> {
    match change {
        Change::Run { assignment, .. }
        | Change::Build { assignment, .. }
        | Change::Current { assignment, .. }
            if *assignment != viewer.assignment =>
        {
            None
        }
        Change::Run {
            submission,
            user,
            email,
            status,
            score,
            total,
            ..
        } if viewer.staff => Some((
            "run",
            serde_json::json!({
                "submission": submission, "student": email, "status": status,
                "score": score, "total": total,
            }),
        )),
        Change::Run {
            submission,
            user,
            status,
            ..
        } if *user == Some(viewer.user) => Some((
            "run",
            serde_json::json!({"submission": submission, "status": status}),
        )),
        Change::Build {
            build,
            build_status,
            validation_status,
            ..
        } if viewer.staff => Some((
            "build",
            serde_json::json!({
                "build": build, "build_status": build_status,
                "validation_status": validation_status,
            }),
        )),
        Change::Current { build, .. } if viewer.staff => {
            Some(("current", serde_json::json!({ "current": build })))
        }
        _ => None,
    }
}

struct Watch {
    state: AppState,
    viewer: Viewer,
    course: String,
    current: CurrentUser,
    rx: broadcast::Receiver<Change>,
    tick: tokio::time::Interval,
    ticks: u32,
    /// Runs seen queued (run → submission), to refresh their positions.
    queued: HashMap<i64, i64>,
    /// Events ready to send.
    out: VecDeque<Event>,
}

pub async fn stream(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((course, slug)): Path<(String, String)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let access = require(&state, &course, &current, Need::Member).await?;
    let a = find(&state, &access.course.id, &slug).await?;
    let viewer = Viewer {
        user: current.user.id,
        staff: is_staff(&access),
        assignment: a.id,
    };
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let watch = Watch {
        rx: state.events.subscribe(),
        state,
        viewer,
        course: access.course.id,
        current,
        tick,
        ticks: 0,
        queued: HashMap::new(),
        out: VecDeque::new(),
    };
    let events = futures::stream::unfold(watch, |mut w| async move {
        loop {
            if let Some(event) = w.out.pop_front() {
                return Some((Ok(event), w));
            }
            tokio::select! {
                got = w.rx.recv() => match got {
                    Ok(change) => w.take(&change),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        w.out.push_back(Event::default().event("resync").data("{}"));
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
                _ = w.tick.tick() => {
                    w.ticks += 1;
                    if w.ticks % CHECK_EVERY == 0 && !w.still_allowed().await {
                        return None;
                    }
                    w.refresh_positions().await;
                }
            }
        }
    });
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

impl Watch {
    fn take(&mut self, change: &Change) {
        let Some((name, mut data)) = shown(&self.viewer, change) else {
            return;
        };
        if let Change::Run {
            run,
            submission,
            status,
            ..
        } = change
        {
            if status == "queued" {
                self.queued.insert(*run, *submission);
            } else {
                self.queued.remove(run);
            }
        }
        if let Some(obj) = data.as_object_mut() {
            obj.retain(|_, v| !v.is_null());
        }
        self.out
            .push_back(Event::default().event(name).data(data.to_string()));
    }

    /// Queue positions of the runs seen queued (they move as others start).
    async fn refresh_positions(&mut self) {
        let runs: Vec<(i64, i64)> = self.queued.iter().map(|(r, s)| (*r, *s)).collect();
        let mut released = HashSet::new();
        for (run, submission) in runs {
            match db::queue::position(&self.state.pool, QueuedRun::Grading(run)).await {
                Ok(Some(p)) => self.out.push_back(Event::default().event("position").data(
                    serde_json::json!({"submission": submission, "queue_position": p}).to_string(),
                )),
                Ok(None) => drop(released.insert(run)),
                Err(e) => tracing::warn!(error = %e, run, "reading a queue position failed"),
            }
        }
        self.queued.retain(|run, _| !released.contains(run));
    }

    /// The session is still open and its user still in the course.
    async fn still_allowed(&mut self) -> bool {
        match db::accounts::session_user(&self.state.pool, &self.current.token_hash).await {
            Ok(Some(user)) => {
                self.current.user = user;
                require(&self.state, &self.course, &self.current, Need::Member)
                    .await
                    .is_ok_and(|access| is_staff(&access) == self.viewer.staff)
            }
            Ok(None) => false,
            // A database hiccup isn't a reason to drop the page.
            Err(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(assignment: i64, user: i64, status: &str) -> Change {
        Change::Run {
            run: 7,
            submission: 41,
            assignment,
            user: Some(user),
            email: Some("ana@ufl.edu".into()),
            status: status.into(),
            score: Some(9.0),
            total: Some(10.0),
        }
    }

    #[test]
    fn viewers_see_only_their_assignment_and_what_they_may() {
        let student = Viewer {
            user: 1,
            staff: false,
            assignment: 3,
        };
        let staff = Viewer {
            user: 9,
            staff: true,
            assignment: 3,
        };
        let build = Change::Build {
            build: 5,
            assignment: 3,
            build_status: "succeeded".into(),
            validation_status: "running".into(),
        };

        let (name, data) = shown(&student, &run(3, 1, "graded")).unwrap();
        assert_eq!(name, "run");
        assert_eq!(
            data,
            serde_json::json!({"submission": 41, "status": "graded"}),
            "no score"
        );
        assert_eq!(
            shown(&student, &run(3, 2, "graded")),
            None,
            "someone else's run"
        );
        assert_eq!(shown(&student, &build), None, "builds are for staff");
        assert_eq!(
            shown(&student, &run(4, 1, "graded")),
            None,
            "another assignment"
        );

        let (_, data) = shown(&staff, &run(3, 1, "graded")).unwrap();
        assert_eq!(data["student"], "ana@ufl.edu");
        assert_eq!(data["score"], 9.0);
        assert_eq!(shown(&staff, &build).unwrap().0, "build");
        let current = Change::Current {
            assignment: 3,
            build: Some(2),
        };
        assert_eq!(
            shown(&staff, &current),
            Some(("current", serde_json::json!({ "current": 2 })))
        );
        assert_eq!(shown(&student, &current), None);
        assert_eq!(shown(&staff, &run(4, 1, "graded")), None);
    }

    #[test]
    fn the_triggers_payloads_parse() {
        let payload = r#"{"kind" : "run", "run" : 1, "submission" : 1, "assignment" : 1, "user" : 1,
            "email" : "a@ufl.edu", "status" : "queued", "score" : null, "total" : null}"#;
        assert!(matches!(
            serde_json::from_str::<Change>(payload),
            Ok(Change::Run { .. })
        ));
        let payload = r#"{"kind" : "build", "build" : 2, "assignment" : 1,
            "build_status" : "running", "validation_status" : "pending"}"#;
        assert!(matches!(
            serde_json::from_str::<Change>(payload),
            Ok(Change::Build { .. })
        ));
        let payload = r#"{"kind" : "current", "assignment" : 1, "build" : 2}"#;
        assert_eq!(
            serde_json::from_str::<Change>(payload).unwrap(),
            Change::Current {
                assignment: 1,
                build: Some(2)
            }
        );
    }

    /// What arrives on `stream` within `wait`.
    async fn collect(mut stream: reqwest::Response, wait: Duration) -> String {
        let mut text = String::new();
        let until = tokio::time::Instant::now() + wait;
        while let Ok(Ok(Some(chunk))) = tokio::time::timeout_at(until, stream.chunk()).await {
            text.push_str(&String::from_utf8_lossy(&chunk));
        }
        text
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn streams_carry_only_what_each_viewer_may_see(pool: PgPool) -> sqlx::Result<()> {
        use pedagog_store::db::submissions::{NewSubmission, record_submission};
        let (assignment, build) = crate::testing::course(&pool).await?;
        let (state, outbox) = crate::testing::state(pool.clone());
        tokio::spawn(listen(pool.clone(), state.events.clone()));
        let app = crate::http::router(state).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });

        let events = format!("{url}/courses/cop3502-f26/assignments/linked-list/events");
        let open = |token: String| {
            let events = events.clone();
            async move {
                reqwest::Client::new()
                    .get(&events)
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap()
            }
        };
        let alice = open(crate::testing::login(&url, &outbox, "alice@ufl.edu").await).await;
        let prof = open(crate::testing::login(&url, &outbox, "prof@ufl.edu").await).await;
        assert_eq!(alice.status(), 200);
        assert_eq!(alice.headers()["content-type"], "text/event-stream");
        // Let the listener subscribe before anything happens.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut ids = Vec::new();
        for email in ["alice@ufl.edu", "bob@ufl.edu"] {
            let user = db::accounts::user_by_email(&pool, email).await?.unwrap();
            let acc = record_submission(
                &pool,
                &NewSubmission {
                    assignment_id: assignment,
                    user_id: user.id,
                    submitted_by: None,
                    staff: false,
                    counted_at: None,
                    build_id: build,
                    source: pedagog_store::SubmissionSource::Upload,
                    tree_hash: "t",
                    object_key: "k",
                    size_bytes: 1,
                    receipt: "{}",
                },
            )
            .await?;
            ids.push(acc.submission_id);
        }
        let (alice, prof) = tokio::join!(
            collect(alice, Duration::from_secs(2)),
            collect(prof, Duration::from_secs(2))
        );
        let (mine, bobs) = (ids[0], ids[1]);
        assert!(
            alice.contains(&format!(r#""submission":{mine}"#)),
            "{alice}"
        );
        assert!(
            !alice.contains(&format!(r#""submission":{bobs}"#)),
            "bob's run: {alice}"
        );
        assert!(!alice.contains("student"), "{alice}");
        assert!(
            prof.contains(&format!(r#""submission":{mine}"#))
                && prof.contains(&format!(r#""submission":{bobs}"#)),
            "{prof}"
        );
        assert!(prof.contains("bob@ufl.edu"), "{prof}");
        Ok(())
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn a_stream_needs_a_member(pool: PgPool) -> sqlx::Result<()> {
        crate::testing::course(&pool).await?;
        db::accounts::upsert_user(&pool, "eve@ufl.edu", "").await?;
        let (url, outbox) = crate::testing::serve(pool).await;
        let eve = crate::testing::login(&url, &outbox, "eve@ufl.edu").await;
        let r = reqwest::Client::new()
            .get(format!(
                "{url}/courses/cop3502-f26/assignments/linked-list/events"
            ))
            .bearer_auth(eve)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
        Ok(())
    }
}
