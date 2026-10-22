//! Home: your courses, each with what needs you. Students see what's open or
//! due soon in each; staff see failed builds, students who couldn't be
//! graded, builds and regrades under way, and what's due within a week.

use leptos::prelude::*;
use leptos_router::components::A;
use pedagog_core::api::{HomeCourse, MyCourses, StudentAssignment};

use crate::cache::{Kind, use_cached};
use crate::components::{Badge, Failed, Loading, moments};
use crate::describe::{mine_badge, when_line};
use crate::shell::Shell;

#[component]
pub fn Home() -> impl IntoView {
    let courses = use_cached(
        || "/me/courses".to_owned(),
        Kind::Revalidate,
        |path| async move { crate::api::get::<MyCourses>(&path).await },
    );
    view! {
        <Shell>
            <div style="max-width: 1040px; margin: 0 auto">
                <div class="page-head">
                    <div class="stack g-1">
                        <h1 class="title">"Home"</h1>
                        <p class="muted">"Your courses, with what’s open or due soon."</p>
                    </div>
                </div>
                {move || match courses.get() {
                    None => view! { <Loading/> }.into_any(),
                    Some(Err(p)) => view! { <Failed problem=p/> }.into_any(),
                    Some(Ok(home)) if home.courses.is_empty() => view! { <NoCourses/> }.into_any(),
                    Some(Ok(home)) => view! {
                        <div class="stack g-5">
                            {home.courses.into_iter().map(|c| view! { <CourseCard course=c/> }).collect_view()}
                        </div>
                    }
                    .into_any(),
                }}
            </div>
        </Shell>
    }
}

#[component]
fn NoCourses() -> impl IntoView {
    view! {
        <div class="card"><div class="empty">
            <span class="empty-ic" aria-hidden="true"><svg class="i" viewBox="0 0 24 24"><path d="M4 19V6a2 2 0 0 1 2-2h12v15H6a2 2 0 0 0-2 2z"/></svg></span>
            <span class="h2">"You’re not in any courses yet"</span>
            <span class="small">"Your instructor adds students to their course. Once they do, it shows up here."</span>
        </div></div>
    }
}

#[component]
fn CourseCard(course: HomeCourse) -> impl IntoView {
    let href = format!("/courses/{}", course.id);
    let heading = format!("h-{}", course.id);
    let labelled = heading.clone();
    let student = course.role == "student";
    let items = course.items;
    let staff_items = course.staff_items;
    let id = course.id.clone();
    let staff_id = course.id.clone();
    view! {
        <section class="card" aria-labelledby=labelled>
            <div class="card-head">
                <div class="stack">
                    <A href=href.clone() attr:id=heading attr:class="h2" attr:style="color: var(--text); text-decoration: none">{course.name}</A>
                    <span class="row g-2">
                        <span class="mono xs faint">{course.id}</span>
                        <span class=format!("chip chip-{}", course.role)>{course.role.clone()}</span>
                    </span>
                </div>
                <A href=href attr:class="btn btn-sm">"All assignments"</A>
            </div>
            {student.then(|| if items.is_empty() {
                view! { <p class="small muted" style="padding: 16px 20px">"Nothing needs you right now."</p> }.into_any()
            } else {
                view! {
                    <div class="stack">
                        <div class="eyebrow" style="padding: 12px 20px 4px">"Open or due soon"</div>
                        {items.into_iter().map(|a| view! { <Item course=id.clone() a=a/> }).collect_view()}
                    </div>
                }
                .into_any()
            })}
            {(!student).then(|| if staff_items.is_empty() {
                view! { <p class="small muted" style="padding: 16px 20px">"Nothing needs you right now."</p> }.into_any()
            } else {
                view! {
                    <div class="stack">
                        <div class="eyebrow" style="padding: 12px 20px 4px">"Needs attention"</div>
                        {staff_items.iter().map(|item| {
                            let (said, tail) = crate::staff::describe::staff_item(item);
                            view! {
                                <A href=format!("/courses/{staff_id}{tail}")
                                    attr:style="display: block; padding: 12px 20px; border-top: 1px solid var(--border); text-decoration: none; color: var(--text)">
                                    {said}
                                </A>
                            }
                        }).collect_view()}
                    </div>
                }
                .into_any()
            })}
        </section>
    }
}

#[component]
fn Item(course: String, a: StudentAssignment) -> impl IntoView {
    let when = when_line(a.window, &moments(&a.dates), crate::time::now_millis());
    let state = mine_badge(&a);
    view! {
        <A href=format!("/courses/{course}/assignments/{}", a.slug)
            attr:style="display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 6px 16px; align-items: center; padding: 12px 20px; border-top: 1px solid var(--border); text-decoration: none; color: var(--text)">
            <span class="stack g-1" style="min-width: 0">
                <span class="strong">{a.name}</span>
                <span class="small muted">{when}</span>
            </span>
            <Badge words=state/>
        </A>
    }
}
