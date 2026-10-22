//! One build: its outcome, who started it and how long it took, each phase's
//! log, the reference solution's results against what each test declares,
//! and (instructors) making it current. Live while it runs.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use pedagog_core::api::{BuildPage, StaffAssignmentPage};
use pedagog_core::results::Expect;

use crate::cache::{Kind, invalidate, use_cached};
use crate::components::results::Test;
use crate::components::{Badge, Failed, Loading, When};
use crate::describe::{Badge as Words, points};
use crate::session::use_session;
use crate::shell::Shell;

use super::describe::{build_badge, took, validation_badge};
use super::make_current::{Picked, dialog};

/// Lines shown before "Show all".
const TAIL: usize = 200;
/// Past this, "Show all" downloads the log instead of showing it.
const SHOW_AT_MOST: i32 = 20_000;

/// "expected to pass · failed".
pub fn expect_line(expected: Expect, status: pedagog_core::results::Status) -> String {
    use pedagog_core::results::Status;
    let expected = match expected {
        Expect::Pass => "pass",
        Expect::Fail => "fail",
    };
    let got = match status {
        Status::Pass => "passed",
        Status::Fail => "failed",
        Status::Error => "errored",
        Status::Skip => "was skipped",
    };
    format!("expected to {expected} · {got}")
}

#[component]
pub fn Build() -> impl IntoView {
    let params = use_params_map();
    let session = use_session();
    let course = Signal::derive(move || params.with(|p| p.get("course").unwrap_or_default()));
    let slug = Signal::derive(move || params.with(|p| p.get("assignment").unwrap_or_default()));
    let id = Signal::derive(move || {
        params.with(|p| {
            p.get("build")
                .and_then(|b| b.parse::<i64>().ok())
                .unwrap_or_default()
        })
    });
    let base = move || format!("/courses/{}/assignments/{}", course.get(), slug.get());
    let build = use_cached(
        move || format!("{}/builds/{}", base(), id.get()),
        Kind::Revalidate,
        |path| async move { crate::api::get::<BuildPage>(&path).await },
    );
    // For the name, and what's in flight (make current cancels it).
    let assignment = use_cached(base, Kind::Revalidate, |path| async move {
        crate::api::get::<StaffAssignmentPage>(&path).await
    });
    let refresh = move || invalidate(&base());
    Effect::new(move || {
        crate::events::watch(course.get(), slug.get(), move |_| refresh(), refresh);
    });
    let picking = RwSignal::new(None::<Picked>);
    let name = move || match assignment.get() {
        Some(Ok(a)) => a.assignment.name,
        _ => slug.get(),
    };
    view! {
        <Shell course=course>
            <nav class="crumbs" aria-label="Breadcrumb">
                <A href=move || format!("/courses/{}", course.get())>"Assignments"</A>
                <span aria-hidden="true">"/"</span>
                <A href=move || base()>{name}</A>
                <span aria-hidden="true">"/"</span>
                <span aria-current="page">{move || format!("build {}", id.get())}</span>
            </nav>
            {move || match build.get() {
                None => view! { <Loading/> }.into_any(),
                Some(Err(p)) => view! { <Failed problem=p/> }.into_any(),
                Some(Ok(b)) => {
                    let (in_flight, students, current) = match assignment.get() {
                        Some(Ok(a)) => (
                            a.builds.first().filter(|f| !f.finished && f.id != b.build.id).map(|f| f.id),
                            a.assignment.counts.submitted,
                            a.assignment.current_build,
                        ),
                        _ => (None, 0, None),
                    };
                    view! { <Page b=b base=base() manages=super::manages(&session, &course.get()) in_flight=in_flight students=students current=current picking=picking/> }.into_any()
                }
            }}
            {dialog(base, picking, refresh)}
        </Shell>
    }
}

#[component]
fn Page(
    b: BuildPage,
    base: String,
    manages: bool,
    in_flight: Option<i64>,
    students: i64,
    current: Option<i64>,
    picking: RwSignal<Option<Picked>>,
) -> impl IntoView {
    let s = b.build.clone();
    let mismatches = b.tests.iter().filter(|t| !t.as_expected).count();
    let still = match current {
        Some(c) if c != s.id => format!("Build {c} is still current; students aren’t affected."),
        _ => String::new(),
    };
    let ui_base = base.clone();
    let summary = s.clone();
    view! {
        <div class="page-head" style="margin-bottom: 20px">
            <div class="stack g-2">
                <h1 class="title">{format!("Build {}", s.id)}</h1>
                <div class="row g-2">
                    <Badge words=build_badge(&s.build_status)/>
                    {validation_badge(&s).map(|v| view! { <Badge words=v/> })}
                    {if s.current {
                        view! { <span class="badge b-accent"><span class="gl" aria-hidden="true">"★"</span>"Current"</span> }.into_any()
                    } else {
                        view! { <span class="badge b-outline">"Not current"</span> }.into_any()
                    }}
                </div>
            </div>
            <div class="row g-2">
                {(manages && s.usable).then(|| view! {
                    <button class="btn" type="button" on:click=move |_| picking.set(Some(Picked { target: summary.clone(), in_flight, students }))>
                        {if s.image_deleted { "Rebuild and make current…" } else { "Make current…" }}
                    </button>
                })}
                {manages.then(|| view! { <A href=format!("{ui_base}/upload") attr:class="btn">"Upload a fix"</A> })}
            </div>
        </div>
        <div class="stack g-5">
            <dl class="kv card card-pad" style="grid-template-columns: max-content minmax(0, 1fr) max-content minmax(0, 1fr)">
                <dt>"Started"</dt><dd><When rfc3339=s.started_at.clone()/></dd>
                <dt>"By"</dt><dd>{s.uploaded_by.clone().unwrap_or_else(|| "—".into())}</dd>
                <dt>"Image"</dt><dd class="mono small" style="overflow-wrap: anywhere">{if s.image_deleted { format!("{} (cleaned up)", b.image_ref) } else { b.image_ref.clone() }}</dd>
                <dt>"Took"</dt><dd>{took(&s.build_status, b.build_ms, b.validation_ms)}</dd>
                {s.rebuild_of.map(|r| view! { <dt>"Rebuild of"</dt><dd><A href=format!("{base}/builds/{r}")>{format!("build {r}")}</A></dd> })}
            </dl>
            {(s.build_status == "failed").then(|| view! {
                <div class="alert alert-fail"><span class="ic" aria-hidden="true">"✕"</span><div class="body">
                    <span class="strong">{format!("The image didn’t build: {}", b.error.clone().unwrap_or_default())}</span>
                    <span class="small muted">{still.clone()}</span>
                </div></div>
            })}
            {(s.finished && matches!(s.validation_status.as_str(), "failed" | "invalid")).then(|| view! {
                <div class="alert alert-fail"><span class="ic" aria-hidden="true">"✕"</span><div class="body">
                    <span class="strong">{if mismatches > 0 {
                        format!("The reference solution disagrees with the tests: {mismatches} {}", if mismatches == 1 { "mismatch" } else { "mismatches" })
                    } else {
                        "Validation couldn’t judge this build".to_owned()
                    }}</span>
                    <ul class="bullets mono small">{b.validation_problems.iter().map(|p| view! { <li>{p.clone()}</li> }).collect_view()}</ul>
                    <span class="small muted">"A failed validation never replaces the current build. "{still.clone()}</span>
                </div></div>
            })}
            {(s.current && s.finished).then(|| view! {
                <div class="alert alert-pass" role="status"><span class="ic" aria-hidden="true">"✓"</span><div class="body"><span class="strong">{format!("Build {} is current: students’ submissions are graded with it.", s.id)}</span></div></div>
            })}
            {(!b.validation_warnings.is_empty()).then(|| view! {
                <div class="alert alert-warn"><span class="ic" aria-hidden="true">"!"</span><div class="body">
                    <span class="strong">{format!("{} {}", b.validation_warnings.len(), if b.validation_warnings.len() == 1 { "warning" } else { "warnings" })}</span>
                    {b.validation_warnings.iter().map(|w| view! { <span class="small">{w.clone()}</span> }).collect_view()}
                </div></div>
            })}
            {(!b.tests.is_empty()).then(|| {
                let expected = b.tests.len() - mismatches;
                view! {
                    <section class="stack g-3" aria-labelledby="ref-h">
                        <div class="row between">
                            <h2 class="h2" id="ref-h">"Reference solution’s results"</h2>
                            <span class="small muted">{format!("{expected} of {} tests as expected · {} / {} points · every part shown", b.tests.len(), points(b.score.unwrap_or(0.0)), points(b.total.unwrap_or(0.0)))}</span>
                        </div>
                        <div class="tests">
                            <div class="tests-head"><span></span><span>"Test · expected vs. got"</span><span>"Validation"</span><span style="text-align: right">"Points"</span></div>
                            {b.tests.clone().into_iter().map(|t| {
                                let note = expect_line(t.expected, t.test.status);
                                let verdict = if t.as_expected {
                                    Words { class: "b-pass", glyph: "✓", text: "As expected".into() }
                                } else {
                                    Words { class: "b-fail", glyph: "✕", text: "Not as expected".into() }
                                };
                                view! { <Test test=t.test verdict=verdict note=note/> }
                            }).collect_view()}
                        </div>
                    </section>
                }
            })}
            <Log base=base.clone() id=s.id phase="build" lines=b.build_log_lines open=s.build_status == "failed"/>
            <Log base=base.clone() id=s.id phase="validation" lines=b.validation_log_lines open=false/>
        </div>
    }
}

/// A phase's log: its last lines, then all of it on request.
#[component]
fn Log(
    base: String,
    id: i64,
    phase: &'static str,
    lines: Option<i32>,
    open: bool,
) -> impl IntoView {
    let heading = if phase == "build" {
        "Build log"
    } else {
        "Validation log"
    };
    let Some(lines) = lines else {
        return view! {
            <section class="stack g-2">
                <h2 class="h2">{heading}</h2>
                <p class="small muted">"No log kept (logs are kept 180 days, from builds since they were kept)."</p>
            </section>
        }
        .into_any();
    };
    let path = format!("{base}/builds/{id}/log?phase={phase}");
    let text = RwSignal::new(None::<Result<String, String>>);
    let all = RwSignal::new(false);
    let fetch = {
        let path = path.clone();
        move |whole: bool| {
            let path = if whole {
                path.clone()
            } else {
                format!("{path}&tail={TAIL}")
            };
            leptos::task::spawn_local(async move {
                text.set(Some(
                    crate::api::get_text(&path).await.map_err(|p| p.message),
                ));
                all.set(whole);
            });
        }
    };
    let shown = RwSignal::new(open);
    let load = fetch.clone();
    Effect::new(move || {
        if shown.get() && text.with_untracked(Option::is_none) {
            load(false);
        }
    });
    let earlier = (lines as usize).saturating_sub(TAIL);
    let download = crate::api::url(&path);
    view! {
        <details class="stack g-3" open=open on:toggle=move |e| {
            shown.set(event_target::<web_sys::Element>(&e).has_attribute("open"));
        }>
            <summary class="h2" style="cursor: pointer">{format!("{heading} · {lines} lines")}</summary>
            {move || match text.get() {
                None => view! { <Loading rows=2/> }.into_any(),
                Some(Err(m)) => view! { <p class="small muted">{m}</p> }.into_any(),
                Some(Ok(t)) => {
                    let partial = !all.get() && earlier > 0;
                    let more = fetch.clone();
                    view! {
                        <div class="codewrap">
                            <pre class="code" style="max-height: 480px">
                                {partial.then(|| view! { <span class="dim">{format!("… {earlier} earlier lines\n")}</span> })}
                                {t}
                            </pre>
                        </div>
                        <div class="code-foot">
                            <span>{if partial { format!("Last {TAIL} of {lines} lines") } else { format!("{lines} lines") }}</span>
                            {partial.then(|| if lines > SHOW_AT_MOST {
                                view! { <a class="btn-link" href=download.clone() download="">"Download the whole log"</a> }.into_any()
                            } else {
                                view! { <button class="btn-link" type="button" on:click=move |_| more(true)>"Show all"</button> }.into_any()
                            })}
                        </div>
                    }
                    .into_any()
                }
            }}
        </details>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use pedagog_core::results::Status;

    use super::*;

    #[test]
    fn a_reference_test_says_what_it_declared_and_did() {
        assert_eq!(
            expect_line(Expect::Pass, Status::Fail),
            "expected to pass · failed"
        );
        assert_eq!(
            expect_line(Expect::Fail, Status::Fail),
            "expected to fail · failed"
        );
    }
}
