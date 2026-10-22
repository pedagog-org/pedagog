//! A student's results: the score over the tests shown, each test with its
//! feedback, printed output and error as far as they may see them, and a
//! note on what's still hidden.

use leptos::prelude::*;
use pedagog_core::results::{Status, Step, StepResult};
use pedagog_core::view::{ShownTest, StudentView};

use crate::describe::{Badge as Words, points};

use super::Badge;

/// A test's outcome as a badge.
pub fn status_badge(s: Status) -> Words {
    let (class, glyph, text) = match s {
        Status::Pass => ("b-pass", "✓", "Pass"),
        Status::Fail => ("b-fail", "✕", "Fail"),
        Status::Error => ("b-fail", "✕", "Error"),
        Status::Skip => ("", "–", "Skipped"),
    };
    Words {
        class,
        glyph,
        text: text.into(),
    }
}

/// Points for extra credit: `+1 extra` once earned, else `+0 of 1`.
pub fn extra_points(score: f64, worth: f64) -> String {
    if score > 0.0 {
        format!("+{} extra", points(score))
    } else {
        format!("+0 of {}", points(worth))
    }
}

/// A test's badge: its outcome, or that it's extra credit (never a failure).
pub fn test_badge(t: &ShownTest) -> Words {
    match t.extra_credit {
        Some(_) => Words {
            class: "",
            glyph: "+",
            text: "Extra credit".into(),
        },
        None => status_badge(t.status),
    }
}

/// A sanitized error's summary and the rest (the call chain), split at the
/// first blank line.
pub fn split_error(error: &str) -> (&str, Option<&str>) {
    match error.split_once("\n\n") {
        Some((summary, rest)) if !rest.trim().is_empty() => (summary, Some(rest.trim_end())),
        _ => (error.trim_end(), None),
    }
}

/// A rubric step: its class (`pass`, `fail`, `award`), glyph, screen-reader
/// word and points (`+2`, `0 / 1`).
pub fn step_parts(s: &Step) -> (&'static str, &'static str, &'static str, String) {
    match s.result {
        Some(StepResult::Pass) => ("pass", "✓", "passed", format!("+{}", points(s.points))),
        Some(StepResult::Fail) => (
            "fail",
            "✕",
            "failed",
            format!(
                "{} / {}",
                points(s.points),
                points(s.worth.unwrap_or(s.points))
            ),
        ),
        None => ("award", "+", "award", format!("+{}", points(s.points))),
    }
}

/// What's still hidden, if anything: "Some tests are hidden until the
/// deadline (Fri, Oct 23 · 11:59 PM)."
pub fn hidden_note(until: &[String], never: bool, deadline: Option<&str>) -> Option<String> {
    let when: Vec<String> = until
        .iter()
        .map(|u| match (u.as_str(), deadline) {
            ("the deadline", Some(d)) => format!("the deadline ({d})"),
            _ => u.clone(),
        })
        .collect();
    match (when.as_slice(), never) {
        ([], false) => None,
        ([], true) => Some("Some tests aren’t shown to students.".into()),
        (w, _) => Some(format!("Some tests are hidden until {}.", w.join(" or "))),
    }
}

/// The score card and the tests.
#[component]
pub fn Results(view: StudentView, deadline: Option<String>) -> impl IntoView {
    // Extra credit isn't owed, so it isn't counted as passed or not.
    let owed = || view.tests.iter().filter(|t| t.extra_credit.is_none());
    let passed = owed().filter(|t| t.status == Status::Pass).count();
    let tests = owed().count();
    let pct = if view.total > 0.0 {
        (view.score / view.total * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let note = hidden_note(&view.hidden_until, view.hidden_never, deadline.as_deref());
    let label = format!("{} of {} points", points(view.score), points(view.total));
    view! {
        <div class="card card-pad stack g-3">
            <div class="score"><span class="big">{points(view.score)}</span><span class="of">{format!("/ {} on the tests shown", points(view.total))}</span></div>
            <div class="meter" role="img" aria-label=label><span style=format!("width: {pct:.0}%")></span></div>
            <span class="small muted">{format!("{passed} of {tests} tests shown passed")}</span>
        </div>
        <div class="tests">
            <div class="tests-head"><span></span><span>"Test"</span><span>"Result"</span><span style="text-align: right">"Points"</span></div>
            {view.tests.into_iter().map(|t| view! { <Test test=t/> }).collect_view()}
            {note.map(|n| view! {
                <div class="hidden-note">
                    <svg class="i" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 3l18 18M10.6 5.1A10 10 0 0 1 12 5c6 0 9.5 7 9.5 7a17 17 0 0 1-2.6 3.4M6.3 6.3A17 17 0 0 0 2.5 12S6 19 12 19a9.6 9.6 0 0 0 4.2-1"/></svg>
                    <span>{n}</span>
                </div>
            })}
        </div>
    }
}

/// A test, opened when it failed; `verdict` replaces its badge and `note`
/// goes under its name (a validation's "expected to pass · failed").
#[component]
pub fn Test(
    test: ShownTest,
    #[prop(optional)] verdict: Option<Words>,
    #[prop(optional)] note: Option<String>,
) -> impl IntoView {
    let failed = match &verdict {
        Some(v) => v.class == "b-fail",
        None => test.extra_credit.is_none() && matches!(test.status, Status::Fail | Status::Error),
    };
    let badge = verdict.unwrap_or_else(|| test_badge(&test));
    let pts = match test.extra_credit {
        Some(worth) => extra_points(test.score, worth),
        None => format!("{} / {}", points(test.score), points(test.max)),
    };
    let feedback = test.feedback.unwrap_or_default();
    let stdout = test.stdout.filter(|s| !s.is_empty());
    let error = test.error.filter(|s| !s.is_empty());
    let sanitized = test.error_sanitized;
    let nothing = feedback.is_empty() && stdout.is_none() && error.is_none();
    view! {
        <details class="test" open=failed>
            <summary>
                <svg class="i i-sm chev" viewBox="0 0 24 24" aria-hidden="true"><path d="m9 6 6 6-6 6"/></svg>
                <span class="stack" style="min-width: 0">
                    <span class="tname">{test.name}</span>
                    {note.map(|n| view! { <span class="small muted">{n}</span> })}
                </span>
                <Badge words=badge/>
                <span class="pts">{pts}</span>
            </summary>
            <div class="test-body">
                {(!feedback.is_empty()).then(|| view! {
                    <div class="part">
                        <div class="part-h">"Feedback"</div>
                        <ul class="items">
                            {feedback.into_iter().map(|item| view! {
                                <li class="item">
                                    <div class="item-h"><span>{item.name}</span><span class="pts">{if item.extra_credit { extra_points(item.score, item.max) } else { format!("{} / {}", points(item.score), points(item.max)) }}</span></div>
                                    {(!item.text.is_empty()).then(|| view! { <p class="small">{item.text.clone()}</p> })}
                                    {(!item.steps.is_empty()).then(|| view! { <ul class="isteps">
                                        {item.steps.into_iter().map(|s| {
                                            let (class, glyph, sr, pts) = step_parts(&s);
                                            view! {
                                                <li>
                                                    <span class=format!("sg {class}") role="img" aria-label=sr>{glyph}</span>
                                                    <span class="sp">{pts}</span>
                                                    <span>{s.reason.unwrap_or_default()}{s.message.map(|m| view! { <span class="msg">{m}</span> })}</span>
                                                </li>
                                            }
                                        }).collect_view()}
                                    </ul> })}
                                </li>
                            }).collect_view()}
                        </ul>
                    </div>
                })}
                {stdout.map(|out| view! {
                    <div class="part"><div class="part-h">"Printed output"</div><div class="codewrap"><pre class="code">{out.clone()}</pre><Copy text=out/></div></div>
                })}
                {error.map(|err| if sanitized {
                    let (summary, chain) = split_error(&err);
                    let (summary, chain) = (summary.to_owned(), chain.map(str::to_owned));
                    view! {
                        <div class="part">
                            <div class="part-h">"Error"</div>
                            <pre class="code" style="color: var(--fail)">{summary}</pre>
                            {chain.map(|c| view! {
                                <details class="chain">
                                    <summary class="small muted" style="cursor: pointer; margin-top: 6px">"How it got there"</summary>
                                    <pre class="code">{c}</pre>
                                </details>
                            })}
                        </div>
                    }
                    .into_any()
                } else {
                    view! { <div class="part"><div class="part-h">"Error"</div><pre class="code" style="color: var(--fail)">{err}</pre></div> }.into_any()
                })}
                {nothing.then(|| view! { <p class="small faint">"No details for this test."</p> })}
            </div>
        </details>
    }
}

#[component]
fn Copy(text: String) -> impl IntoView {
    let copied = RwSignal::new(false);
    let copy = move |_| {
        if let Some(w) = web_sys::window() {
            let _ = w.navigator().clipboard().write_text(&text);
            copied.set(true);
        }
    };
    view! {
        <button class="btn btn-ghost btn-sm copy" type="button" aria-label="Copy printed output" on:click=copy>
            {move || if copied.get() { "Copied" } else { "Copy" }}
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(result: Option<StepResult>, points: f64, worth: Option<f64>) -> Step {
        Step {
            id: None,
            result,
            points,
            worth,
            reason: None,
            message: None,
        }
    }

    #[test]
    fn steps_read_as_checks_and_awards() {
        assert_eq!(
            step_parts(&step(Some(StepResult::Pass), 2.0, Some(2.0))).3,
            "+2"
        );
        assert_eq!(
            step_parts(&step(Some(StepResult::Fail), 0.0, Some(1.0))).3,
            "0 / 1"
        );
        assert_eq!(step_parts(&step(None, 1.0, None)).0, "award");
    }

    #[test]
    fn extra_credit_never_reads_as_a_failure() {
        let test = |extra_credit| ShownTest {
            name: "stretch goal".into(),
            number: None,
            status: Status::Fail,
            score: 0.0,
            max: 0.0,
            extra_credit,
            feedback: None,
            stdout: None,
            error: None,
            error_sanitized: false,
        };
        assert_eq!(test_badge(&test(Some(1.0))).text, "Extra credit");
        assert_eq!(test_badge(&test(None)).text, "Fail");
        assert_eq!(extra_points(0.0, 1.0), "+0 of 1");
        assert_eq!(extra_points(1.0, 1.0), "+1 extra");
    }

    #[test]
    fn a_sanitized_error_splits_into_summary_and_chain() {
        let err = "linked_list.py:42: AttributeError: oops\n\ntest.py:70, in test_find\n    items.find(3)\n";
        assert_eq!(
            split_error(err),
            (
                "linked_list.py:42: AttributeError: oops",
                Some("test.py:70, in test_find\n    items.find(3)")
            )
        );
        assert_eq!(split_error("just this\n"), ("just this", None));
    }

    #[test]
    fn the_hidden_note_says_until_when() {
        assert_eq!(hidden_note(&[], false, None), None);
        assert_eq!(
            hidden_note(
                &["the deadline".into()],
                false,
                Some("Fri, Oct 23 · 11:59 PM")
            ),
            Some("Some tests are hidden until the deadline (Fri, Oct 23 · 11:59 PM).".into())
        );
        assert_eq!(
            hidden_note(&["the deadline".into(), "release".into()], true, None),
            Some("Some tests are hidden until the deadline or release.".into())
        );
        assert_eq!(
            hidden_note(&[], true, None),
            Some("Some tests aren’t shown to students.".into())
        );
    }
}
