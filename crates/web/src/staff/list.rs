//! A course's assignments for staff: each one's window, current and latest
//! build, and how many students have submitted.

use leptos::prelude::*;
use leptos_router::components::A;
use pedagog_core::api::StaffAssignment;

use crate::cache::{Kind, use_cached};
use crate::components::{Badge, Failed, Loading, moments};
use crate::describe::window_badge;
use crate::session::use_session;

use super::describe::{build_badge, validation_badge};

#[component]
pub fn StaffList(course: Signal<String>) -> impl IntoView {
    let session = use_session();
    let assignments = use_cached(
        move || format!("/courses/{}/assignments", course.get()),
        Kind::Revalidate,
        |path| async move { crate::api::get::<Vec<StaffAssignment>>(&path).await },
    );
    view! {
        {move || super::manages(&session, &course.get()).then(|| view! {
            <div class="row" style="justify-content: flex-end; margin: -8px 0 16px">
                <A href=format!("/courses/{}/upload", course.get()) attr:class="btn btn-primary">"Upload assignment"</A>
            </div>
        })}
        {move || match assignments.get() {
            None => view! { <Loading/> }.into_any(),
            Some(Err(p)) => view! { <Failed problem=p/> }.into_any(),
            Some(Ok(list)) if list.is_empty() => view! {
                <div class="card"><div class="empty"><span class="h2">"No assignments yet"</span><span class="small">"Upload one to build it and check its reference solution."</span></div></div>
            }
            .into_any(),
            Some(Ok(list)) => view! {
                <div class="table-wrap">
                    <table class="dt">
                        <thead><tr><th>"Assignment"</th><th>"Window"</th><th>"Current build"</th><th>"Latest build"</th><th class="num">"Submitted"</th></tr></thead>
                        <tbody>
                            {list.into_iter().map(|a| view! { <Row course=course.get_untracked() a=a/> }).collect_view()}
                        </tbody>
                    </table>
                </div>
            }
            .into_any(),
        }}
    }
}

#[component]
fn Row(course: String, a: StaffAssignment) -> impl IntoView {
    let window = window_badge(a.window, &moments(&a.dates));
    let latest = a.latest_build.clone().filter(|b| !b.current);
    let submitted = format!("{} / {}", a.counts.submitted, a.counts.students);
    view! {
        <tr>
            <td>
                <A href=format!("/courses/{course}/assignments/{}", a.slug) attr:class="rowlink strong">{a.name}</A>
                <div class="mono xs faint">{a.slug.clone()}</div>
            </td>
            <td><Badge words=window/></td>
            <td class="nowrap">{match a.current_build {
                Some(id) => view! { <span class="badge b-accent"><span class="gl" aria-hidden="true">"★"</span>{format!("build {id}")}</span> }.into_any(),
                None => view! { <span class="faint">"none yet"</span> }.into_any(),
            }}</td>
            <td class="nowrap">{latest.map(|b| view! {
                <span class="row g-2">
                    <span class="mono small">{format!("build {}", b.id)}</span>
                    <Badge words=build_badge(&b.build_status)/>
                    {validation_badge(&b).map(|v| view! { <Badge words=v/> })}
                </span>
            })}</td>
            <td class="num">{submitted}{(a.counts.not_graded > 0).then(|| view! {
                <div class="xs" style="color: var(--fail)">{format!("{} couldn’t be graded", a.counts.not_graded)}</div>
            })}</td>
        </tr>
    }
}
