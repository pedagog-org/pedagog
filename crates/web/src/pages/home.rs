//! Home: your courses, with your role in each. (Per-course summaries, open or
//! due soon for students and what needs attention for staff, come with the
//! api's home summaries.)

use leptos::prelude::*;
use leptos_router::components::A;

use crate::session::use_session;
use crate::shell::Shell;

#[component]
pub fn Home() -> impl IntoView {
    let session = use_session();
    let courses = move || session.me().map(|m| m.courses).unwrap_or_default();
    view! {
        <Shell>
            <div class="page-head">
                <h1 class="title">"Your courses"</h1>
            </div>
            {move || {
                let list = courses();
                if list.is_empty() {
                    view! {
                        <div class="card empty">
                            <span class="strong">"You’re not in any courses yet"</span>
                            <span class="small">"Your instructor adds you when the course starts."</span>
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <div class="card">
                            {list
                                .into_iter()
                                .map(|c| view! {
                                    <A href=format!("/courses/{}", c.id) attr:class="menu-item" attr:style="padding: 14px 18px; border-bottom: 1px solid var(--border)">
                                        <span class="stack">
                                            <span class="strong">{c.name.clone()}</span>
                                            <span class="mono xs faint">{c.id.clone()}</span>
                                        </span>
                                        <span class=format!("chip chip-{}", c.role)>{c.role.clone()}</span>
                                    </A>
                                })
                                .collect_view()}
                        </div>
                    }
                    .into_any()
                }
            }}
        </Shell>
    }
}
