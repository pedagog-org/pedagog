//! Data kept across pages, by how often it changes. Slow-changing data (you,
//! your courses, a course, its assignments, members) is reused for the session
//! and refetched only when a change touches it, or after five minutes.
//! Results, submissions and dates show the kept copy at once and refresh in
//! the background. Live statuses don't come through here: they're events.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;

use leptos::prelude::*;

use crate::api::Problem;

/// How long slow-changing data is reused before it's fetched again.
pub const SESSION_FOR_MS: f64 = 5.0 * 60_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Reused while fresh; refetched when stale or invalidated.
    Session,
    /// Shown from the cache at once, and refetched every time.
    Revalidate,
}

/// What to do for a key: show the kept copy, fetch, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub show_cached: bool,
    pub fetch: bool,
}

pub fn plan(kind: Kind, age_ms: Option<f64>) -> Plan {
    match (kind, age_ms) {
        (_, None) => Plan {
            show_cached: false,
            fetch: true,
        },
        (Kind::Session, Some(age)) => Plan {
            show_cached: true,
            fetch: age >= SESSION_FOR_MS,
        },
        (Kind::Revalidate, Some(_)) => Plan {
            show_cached: true,
            fetch: true,
        },
    }
}

struct Entry {
    value: Rc<dyn Any>,
    fetched_ms: f64,
}

thread_local! {
    static ENTRIES: RefCell<HashMap<String, Entry>> = RefCell::new(HashMap::new());
    /// Bumped by every invalidation, so pages showing a forgotten key fetch it again.
    static GENERATION: ArcRwSignal<u64> = ArcRwSignal::new(0);
}

/// Forget every key starting with `prefix` (after a change that affects them),
/// e.g. `/courses/cop3502-f26/members`; pages showing one fetch it again.
pub fn invalidate(prefix: &str) {
    ENTRIES.with(|e| e.borrow_mut().retain(|k, _| !k.starts_with(prefix)));
    GENERATION.with(|g| g.update(|n| *n += 1));
}

/// Data for `key` (the api path it comes from), kept by `kind`: `None` while
/// nothing is known yet, then the latest answer.
pub fn use_cached<T, F, Fut>(
    key: impl Fn() -> String + 'static,
    kind: Kind,
    fetch: F,
) -> ReadSignal<Option<Result<T, Problem>>, LocalStorage>
where
    T: Clone + 'static,
    F: Fn(String) -> Fut + Copy + 'static,
    Fut: Future<Output = Result<T, Problem>> + 'static,
{
    let (value, set_value) = signal_local(None::<Result<T, Problem>>);
    let shown = StoredValue::new_local(None::<String>);
    Effect::new(move || {
        let key = key();
        GENERATION.with(|g| g.track());
        let now = crate::time::now_millis();
        let kept_here = ENTRIES.with(|e| e.borrow().contains_key(&key));
        // Another key's invalidation: this one still stands.
        if kept_here && shown.get_value().as_deref() == Some(key.as_str()) {
            return;
        }
        shown.set_value(Some(key.clone()));
        let kept = ENTRIES.with(|e| {
            e.borrow().get(&key).map(|entry| {
                (
                    entry.value.downcast_ref::<T>().cloned(),
                    now - entry.fetched_ms,
                )
            })
        });
        let (kept, age) = match kept {
            Some((Some(v), age)) => (Some(v), Some(age)),
            _ => (None, None),
        };
        let what = plan(kind, age);
        if what.show_cached {
            set_value.set(kept.map(Ok));
        } else {
            set_value.set(None);
        }
        if what.fetch {
            leptos::task::spawn_local(async move {
                let got = fetch(key.clone()).await;
                if let Ok(v) = &got {
                    let entry = Entry {
                        value: Rc::new(v.clone()),
                        fetched_ms: crate::time::now_millis(),
                    };
                    ENTRIES.with(|e| e.borrow_mut().insert(key, entry));
                }
                set_value.set(Some(got));
            });
        }
    });
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_decide_what_to_show_and_fetch() {
        let never = plan(Kind::Session, None);
        assert_eq!(
            never,
            Plan {
                show_cached: false,
                fetch: true
            }
        );
        assert_eq!(
            plan(Kind::Session, Some(1000.0)),
            Plan {
                show_cached: true,
                fetch: false
            }
        );
        assert_eq!(
            plan(Kind::Session, Some(SESSION_FOR_MS)),
            Plan {
                show_cached: true,
                fetch: true
            }
        );
        assert_eq!(
            plan(Kind::Revalidate, Some(1.0)),
            Plan {
                show_cached: true,
                fetch: true
            }
        );
    }
}
