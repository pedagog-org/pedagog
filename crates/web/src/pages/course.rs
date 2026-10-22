//! A course's assignments: for students, each with its window, when it's due
//! and their own state; for staff, a table with builds and submissions.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use pedagog_core::api::StudentAssignment;

use crate::cache::{Kind, use_cached};
use crate::components::{Badge, Failed, Loading, moments};
use crate::describe::{mine_badge, when_line, window_badge};
use crate::session::use_session;
use crate::shell::Shell;

#[component]
pub fn Course() -> impl IntoView {
    let params = use_params_map();
    let session = use_session();
    let course = Signal::derive(move || params.with(|p| p.get("course").unwrap_or_default()));
    let student = move || {
        session
            .course(&course.get())
            .is_some_and(|c| c.role == "student")
    };
    let (zone, zone_long) = crate::time::zone_now();
    view! {
        <Shell course=course>
            <div class="page-head">
                <div class="stack g-1">
                    <h1 class="title">"Assignments"</h1>
                    <p class="muted small">"Times are in your time zone ("<abbr class="t" title=zone_long.clone()>{zone.clone()}</abbr>")."</p>
                </div>
            </div>
            {move || if student() { view! { <StudentList course=course/> }.into_any() } else { view! { <crate::staff::list::StaffList course=course/> }.into_any() }}
        </Shell>
    }
}

#[component]
fn StudentList(course: Signal<String>) -> impl IntoView {
    let assignments = use_cached(
        move || format!("/courses/{}/assignments", course.get()),
        Kind::Revalidate,
        |path| async move { crate::api::get::<Vec<StudentAssignment>>(&path).await },
    );
    move || {
        match assignments.get() {
        None => view! { <Loading/> }.into_any(),
        Some(Err(p)) => view! { <Failed problem=p/> }.into_any(),
        Some(Ok(list)) if list.is_empty() => view! { <Nothing/> }.into_any(),
        Some(Ok(list)) => view! {
            <ul class="card" style="list-style: none; padding: 0; overflow: hidden" aria-label="Assignments">
                {list.into_iter().map(|a| view! { <Row course=course.get_untracked() a=a/> }).collect_view()}
            </ul>
        }
        .into_any(),
    }
    }
}

#[component]
fn Row(course: String, a: StudentAssignment) -> impl IntoView {
    let m = moments(&a.dates);
    let now = crate::time::now_millis();
    let window = window_badge(a.window, &m);
    let when = when_line(a.window, &m, now);
    let state = mine_badge(&a);
    view! {
        <li style="border-bottom: 1px solid var(--border)">
            <A href=format!("/courses/{course}/assignments/{}", a.slug)
                attr:style="display: flex; flex-wrap: wrap; gap: 8px 24px; align-items: center; padding: 16px 20px; text-decoration: none; color: var(--text)">
                <span class="stack g-1" style="flex: 1 1 320px; min-width: 0">
                    <span class="row g-2"><span class="strong" style="font-size: 15px">{a.name}</span><Badge words=window/></span>
                    <span class="small muted">{when}</span>
                </span>
                <Badge words=state style="height: 26px; padding: 0 10px; font-size: 12.5px"/>
            </A>
        </li>
    }
}

#[component]
fn Nothing() -> impl IntoView {
    view! { <div class="card"><div class="empty"><span class="h2">"Nothing assigned yet"</span><span class="small">"Assignments show up here once your instructor posts them."</span></div></div> }
}
