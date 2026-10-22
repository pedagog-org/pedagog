//! An assignment for staff: its window and release, what needs attention (a
//! failed build, raw errors reaching students), the summary cards, and its
//! builds, each openable and (instructors) makeable current. Live as builds
//! progress.

use leptos::prelude::*;
use leptos_router::components::A;
use pedagog_core::api::{BuildSummary, BuildsPage, StaffAssignmentPage};

use crate::cache::{Kind, invalidate, use_cached};
use crate::components::{Badge, Failed, Loading, When, moments};
use crate::describe::window_badge;
use crate::session::use_session;
use crate::shell::Shell;

use super::describe::{build_badge, validation_badge};
use super::make_current::MakeCurrent;

/// The latest build, when it failed and isn't current: what to say.
pub fn failed_alert(latest: &BuildSummary, current: Option<i64>) -> Option<String> {
    if !latest.finished || latest.current {
        return None;
    }
    let what = if latest.build_status == "failed" {
        "didn’t build"
    } else if matches!(latest.validation_status.as_str(), "failed" | "invalid") {
        "failed validation"
    } else {
        return None;
    };
    let after = match current {
        Some(c) => format!("Build {c} is still current; students aren’t affected."),
        None => "No build is current yet, so students can’t submit.".to_owned(),
    };
    Some(format!("Build {} {what}. {after}", latest.id))
}

#[component]
pub fn Overview(course: Signal<String>, slug: Signal<String>) -> impl IntoView {
    let session = use_session();
    let base = move || format!("/courses/{}/assignments/{}", course.get(), slug.get());
    let page = use_cached(base, Kind::Revalidate, |path| async move {
        crate::api::get::<StaffAssignmentPage>(&path).await
    });
    let refresh = move || invalidate(&base());
    Effect::new(move || {
        crate::events::watch(course.get(), slug.get(), move |_| refresh(), refresh);
    });
    let manages = move || super::manages(&session, &course.get());
    let title = move || match page.get() {
        Some(Ok(p)) => p.assignment.name,
        _ => slug.get(),
    };
    view! {
        <Shell course=course>
            <nav class="crumbs" aria-label="Breadcrumb">
                <A href=move || format!("/courses/{}", course.get())>"Assignments"</A>
                <span aria-hidden="true">"/"</span>
                <span aria-current="page">{title}</span>
            </nav>
            {move || match page.get() {
                None => view! { <Loading/> }.into_any(),
                Some(Err(p)) => view! { <Failed problem=p/> }.into_any(),
                Some(Ok(p)) => view! { <Page page=p base=base() manages=manages() refresh=refresh/> }.into_any(),
            }}
        </Shell>
    }
}

#[component]
fn Page(
    page: StaffAssignmentPage,
    base: String,
    manages: bool,
    refresh: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let a = page.assignment.clone();
    let m = moments(&a.dates);
    let latest = page.builds.first().cloned();
    let failed = latest
        .as_ref()
        .and_then(|b| failed_alert(b, a.current_build));
    let building = latest.clone().filter(|b| !b.finished);
    let in_flight = building.as_ref().map(|b| b.id);
    let ui_base = format!("/{}", base.trim_start_matches('/'));
    let current = page.builds.iter().find(|b| b.current).cloned();
    let picking = RwSignal::new(None::<BuildSummary>);
    let students = a.counts.submitted;
    let due = m.due.clone();
    let late = m.late_due.clone();
    view! {
        <div class="page-head" style="margin-bottom: 16px">
            <div class="stack g-2">
                <div class="row g-3"><h1 class="title">{a.name.clone()}</h1><span class="mono small faint">{a.slug.clone()}</span></div>
                <div class="row g-2">
                    <Badge words=window_badge(a.window, &m)/>
                    {match &page.released_at {
                        Some(_) => view! { <span class="badge b-pass"><span class="gl" aria-hidden="true">"✓"</span>"Results released"</span> }.into_any(),
                        None => view! { <span class="badge"><span class="gl" aria-hidden="true">"○"</span>"Results not released"</span> }.into_any(),
                    }}
                </div>
            </div>
            {manages.then(|| view! { <A href=format!("{ui_base}/upload") attr:class="btn">"Upload new version"</A> })}
        </div>
        <nav class="tabs" aria-label="Assignment">
            <A href=ui_base.clone() attr:class="tab" attr:aria-current="page">"Overview"</A>
        </nav>
        <div class="stack g-5">
            {failed.zip(latest.clone()).map(|(said, b)| view! {
                <div class="alert alert-fail">
                    <span class="ic" aria-hidden="true">"✕"</span>
                    <div class="body"><span class="strong">{said}</span></div>
                    <A href=format!("{}/builds/{}", ui_base, b.id) attr:class="btn btn-sm">{format!("See build {}", b.id)}</A>
                </div>
            })}
            {building.map(|b| view! {
                <div class="alert alert-info" role="status">
                    <span class="ic" aria-hidden="true">"●"</span>
                    <div class="body"><span class="strong">{format!("Build {} is on its way.", b.id)}</span><span class="small muted">"The current build stays in use until it passes validation."</span></div>
                    <A href=format!("{}/builds/{}", ui_base, b.id) attr:class="btn btn-sm">{format!("Open build {}", b.id)}</A>
                </div>
            })}
            {page.students_see_raw_errors.then(|| view! {
                <div class="alert alert-warn">
                    <span class="ic" aria-hidden="true">"!"</span>
                    <div class="body">
                        <span class="strong">"Students see raw errors"</span>
                        <span class="small muted">"The current build’s test helper doesn’t clean up errors, so students who can see a test’s error get the framework’s report, test source included."</span>
                    </div>
                </div>
            })}
            <div class="grid-auto" style="grid-template-columns: repeat(auto-fit, minmax(min(220px, 100%), 1fr))">
                <div class="card card-pad stack g-1">
                    <span class="eyebrow">"Current build"</span>
                    {match current.clone() {
                        Some(b) => view! {
                            <span class="row g-2"><span style="font-size: 20px; font-weight: 600">{b.id}</span>{validation_badge(&b).map(|v| view! { <Badge words=v/> })}</span>
                            <span class="small muted">{page.current_since.clone().map(|s| view! { "since "<When rfc3339=s/> })}</span>
                        }
                        .into_any(),
                        None => view! { <span style="font-size: 20px; font-weight: 600">"None yet"</span><span class="small muted">"students can’t submit until one passes"</span> }.into_any(),
                    }}
                </div>
                <div class="card card-pad stack g-1">
                    <span class="eyebrow">"Submitted"</span>
                    <span style="font-size: 20px; font-weight: 600" class="num">{a.counts.submitted}" "<span class="small muted" style="font-weight: 400">{format!("of {} active students", a.counts.students)}</span></span>
                    <span class="small muted">{match (a.counts.not_graded, a.counts.regrading) {
                        (_, Some(r)) => format!("regrading {} of {}", r.done, r.total),
                        (0, None) => "all graded".to_owned(),
                        (n, None) => format!("{n} couldn’t be graded"),
                    }}</span>
                </div>
                <div class="card card-pad stack g-1">
                    <span class="eyebrow">"Due"</span>
                    <span style="font-size: 20px; font-weight: 600">{due.as_ref().map_or("No deadline".to_owned(), |d| d.day.clone())}</span>
                    <span class="small muted">{match (&due, &late) {
                        (Some(d), Some(l)) => format!("{} · late until {}", d.text, l.day),
                        (Some(d), None) => d.text.clone(),
                        _ => a.status.clone(),
                    }}</span>
                </div>
                <div class="card card-pad stack g-1">
                    <span class="eyebrow">"Released"</span>
                    {match page.released_at.clone() {
                        Some(at) => view! { <span style="font-size: 20px; font-weight: 600"><When rfc3339=at/></span><span class="small muted">"students see everything but hidden tests"</span> }.into_any(),
                        None => view! { <span style="font-size: 20px; font-weight: 600">"Not yet"</span><span class="small muted">"students see visible parts; more after the deadline"</span> }.into_any(),
                    }}
                </div>
            </div>
            <Builds builds=page.builds.clone() base=base.clone() ui_base=ui_base.clone() manages=manages picking=picking/>
        </div>
        {move || picking.get().map(|b| view! {
            <MakeCurrent base=base.clone() target=b in_flight=in_flight students=students
                close=move || picking.set(None) done=refresh/>
        })}
    }
}

#[component]
fn Builds(
    builds: Vec<BuildSummary>,
    base: String,
    ui_base: String,
    manages: bool,
    picking: RwSignal<Option<BuildSummary>>,
) -> impl IntoView {
    let more = RwSignal::new(Vec::<BuildSummary>::new());
    let next = RwSignal::new(builds.last().map(|b| b.id).filter(|_| builds.len() >= 10));
    let loading = RwSignal::new(false);
    let load_more = move |_| {
        let Some(before) = next.get_untracked() else {
            return;
        };
        loading.set(true);
        let path = format!("{base}/builds?before={before}");
        leptos::task::spawn_local(async move {
            if let Ok(page) = crate::api::get::<BuildsPage>(&path).await {
                more.update(|m| m.extend(page.builds));
                next.set(page.next);
            }
            loading.set(false);
        });
    };
    let row = move |b: BuildSummary| {
        let href = format!("{ui_base}/builds/{}", b.id);
        let pick = b.clone();
        view! {
            <tr>
                <td><A href=href.clone() attr:class="rowlink mono">{format!("build {}", b.id)}</A>
                    {b.rebuild_of.map(|r| view! { <div class="xs faint">{format!("rebuild of {r}")}</div> })}</td>
                <td class="nowrap"><When rfc3339=b.started_at.clone()/></td>
                <td class="muted">{b.uploaded_by.clone().unwrap_or_else(|| "—".into())}</td>
                <td><Badge words=build_badge(&b.build_status)/></td>
                <td>{validation_badge(&b).map(|v| view! { <Badge words=v/> })}</td>
                <td>{b.current.then(|| view! { <span class="badge b-accent"><span class="gl" aria-hidden="true">"★"</span>"Current"</span> })}</td>
                <td style="text-align: right" class="nowrap">
                    {(manages && b.usable).then(|| view! {
                        <button class="btn btn-sm" type="button"
                            title=if b.image_deleted { "Its image was cleaned up: it’s built again first" } else { "Students’ new submissions are graded with this build" }
                            on:click=move |_| picking.set(Some(pick.clone()))>
                            {if b.image_deleted { "Rebuild and make current…" } else { "Make current…" }}
                        </button>
                    })}
                    " "<A href=href attr:class="btn btn-ghost btn-sm" attr:aria-label=format!("Open build {}", b.id)>"Open"</A>
                </td>
            </tr>
        }
    };
    let row2 = row.clone();
    view! {
        <section class="stack g-3" aria-labelledby="builds-h">
            <h2 class="h2" id="builds-h">"Builds"</h2>
            {if builds.is_empty() {
                view! { <p class="muted">"No builds yet."</p> }.into_any()
            } else {
                view! {
                    <div class="table-wrap">
                        <table class="dt">
                            <thead><tr><th>"Build"</th><th>"Uploaded"</th><th>"By"</th><th>"Build"</th><th>"Validation"</th><th>"Current"</th><th><span class="sr-only">"Actions"</span></th></tr></thead>
                            <tbody>
                                {builds.into_iter().map(row).collect_view()}
                                {move || more.get().into_iter().map(row2.clone()).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
                .into_any()
            }}
            {move || next.get().map(|_| view! {
                <div><button class="btn btn-sm" type="button" disabled=move || loading.get() on:click=load_more.clone()>"Show older builds"</button></div>
            })}
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(build: &str, validation: &str) -> BuildSummary {
        BuildSummary {
            id: 14,
            build_status: build.into(),
            validation_status: validation.into(),
            finished: true,
            current: false,
            started_at: "2026-10-20T13:58:00Z".into(),
            uploaded_by: None,
            rebuild_of: None,
            image_deleted: false,
            usable: false,
        }
    }

    #[test]
    fn a_failed_latest_build_says_the_current_one_stays() {
        assert_eq!(
            failed_alert(&build("succeeded", "failed"), Some(13)).as_deref(),
            Some(
                "Build 14 failed validation. Build 13 is still current; students aren’t affected."
            )
        );
        assert_eq!(
            failed_alert(&build("failed", "pending"), None).as_deref(),
            Some("Build 14 didn’t build. No build is current yet, so students can’t submit.")
        );
        assert_eq!(failed_alert(&build("succeeded", "passed"), Some(14)), None);
        assert_eq!(
            failed_alert(&build("superseded", "pending"), Some(13)),
            None
        );
    }
}
