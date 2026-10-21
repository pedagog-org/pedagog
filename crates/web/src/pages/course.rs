//! A course: its assignments (the student and staff views come with the
//! student slice; for now, names).

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use serde::Deserialize;

use crate::cache::{Kind, use_cached};
use crate::shell::Shell;

#[derive(Clone, Debug, Deserialize)]
pub struct AssignmentRow {
    pub slug: String,
    pub name: String,
}

#[component]
pub fn Course() -> impl IntoView {
    let params = use_params_map();
    let course = Signal::derive(move || params.with(|p| p.get("course").unwrap_or_default()));
    let assignments = use_cached(
        move || format!("/courses/{}/assignments", course.get()),
        Kind::Session,
        |path| async move { crate::api::get::<Vec<AssignmentRow>>(&path).await },
    );
    view! {
        <Shell course=course>
            <div class="page-head">
                <h1 class="title">"Assignments"</h1>
            </div>
            {move || match assignments.get() {
                None => view! { <div class="card card-pad stack g-3"><div class="sk sk-build" style="width: 40%"></div><div class="sk sk-build" style="width: 60%"></div></div> }.into_any(),
                Some(Err(p)) => view! { <div class="alert alert-fail" role="alert"><span class="ic" aria-hidden="true">"✕"</span><div class="body"><span>{p.message}</span></div></div> }.into_any(),
                Some(Ok(list)) if list.is_empty() => view! { <div class="card empty"><span class="strong">"Nothing assigned yet"</span></div> }.into_any(),
                Some(Ok(list)) => view! {
                    <div class="card">
                        {list.into_iter().map(|a| view! {
                            <div class="row between" style="padding: 14px 18px; border-bottom: 1px solid var(--border)">
                                <span class="strong">{a.name}</span>
                                <span class="mono xs faint">{a.slug}</span>
                            </div>
                        }).collect_view()}
                    </div>
                }.into_any(),
            }}
        </Shell>
    }
}
