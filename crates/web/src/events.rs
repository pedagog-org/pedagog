//! Live updates on an assignment page: the api's event stream
//! (`/api/courses/{c}/assignments/{a}/events`), and polling with backoff when
//! the stream is down. Polling pauses while the tab is hidden.

use leptos::prelude::*;
use serde::Deserialize;

/// What the stream says.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Live {
    Position {
        submission: i64,
        queue_position: i64,
    },
    Run {
        submission: i64,
        status: pedagog_core::api::RunState,
        /// Staff only.
        student: Option<String>,
        score: Option<f64>,
        total: Option<f64>,
    },
    Build {
        build: i64,
        build_status: String,
        validation_status: String,
    },
    /// Staff only: the assignment's current build changed.
    Current { current: Option<i64> },
}

/// Seconds until the next poll, `elapsed` seconds after polling started:
/// every second for half a minute, then every three.
pub fn poll_delay(elapsed: f64) -> f64 {
    if elapsed < 30.0 { 1.0 } else { 3.0 }
}

fn hidden() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .is_some_and(|d| d.hidden())
}

/// Watch an assignment: `on` gets each update; when the stream is down,
/// `poll` is called on the backoff schedule instead (it refetches).
/// Stops when the calling component goes away.
pub fn watch(
    course: String,
    assignment: String,
    on: impl Fn(Live) + 'static,
    poll: impl Fn() + 'static,
) {
    let url = crate::api::url(&format!(
        "/courses/{course}/assignments/{assignment}/events"
    ));
    let down = RwSignal::new(false);
    let poll = std::rc::Rc::new(poll);
    match gloo_net::eventsource::futures::EventSource::new(&url) {
        Ok(mut source) => {
            let mut streams = Vec::new();
            for name in ["run", "position", "build", "current", "resync"] {
                if let Ok(s) = source.subscribe(name) {
                    streams.push(s);
                }
            }
            let mut all = futures::stream::select_all(streams);
            let on = std::rc::Rc::new(on);
            let refetch = poll.clone();
            leptos::task::spawn_local(async move {
                use futures::StreamExt;
                while let Some(next) = all.next().await {
                    match next {
                        Ok((name, message)) => {
                            let data = message.data().as_string().unwrap_or_default();
                            if name == "resync" {
                                refetch();
                            } else if let Ok(live) = serde_json::from_str::<Live>(&data) {
                                on(live);
                            }
                        }
                        Err(_) => {
                            down.set(true);
                            break;
                        }
                    }
                }
            });
            on_cleanup(move || source.close());
        }
        Err(_) => down.set(true),
    }
    // Polling while the stream is down.
    Effect::new(move || {
        if !down.get() {
            return;
        }
        let poll = poll.clone();
        leptos::task::spawn_local(async move {
            let started = crate::time::now_millis();
            while down.get_untracked() {
                let elapsed = (crate::time::now_millis() - started) / 1000.0;
                sleep(poll_delay(elapsed)).await;
                if !hidden() {
                    poll();
                }
            }
        });
    });
}

async fn sleep(seconds: f64) {
    let (done, wait) = futures::channel::oneshot::channel::<()>();
    set_timeout(
        move || {
            let _ = done.send(());
        },
        std::time::Duration::from_secs_f64(seconds),
    );
    let _ = wait.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_backs_off_after_half_a_minute() {
        assert_eq!(poll_delay(0.0), 1.0);
        assert_eq!(poll_delay(29.0), 1.0);
        assert_eq!(poll_delay(30.0), 3.0);
    }

    #[test]
    fn stream_messages_parse() {
        let run: Live = serde_json::from_str(r#"{"submission": 41, "status": "grading"}"#).unwrap();
        assert!(matches!(run, Live::Run { submission: 41, .. }));
        let pos: Live = serde_json::from_str(r#"{"submission": 41, "queue_position": 3}"#).unwrap();
        assert_eq!(
            pos,
            Live::Position {
                submission: 41,
                queue_position: 3
            }
        );
        let build: Live = serde_json::from_str(
            r#"{"build": 5, "build_status": "succeeded", "validation_status": "running"}"#,
        )
        .unwrap();
        assert!(matches!(build, Live::Build { build: 5, .. }));
    }
}
