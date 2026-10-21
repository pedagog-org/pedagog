//! The app shell: the top bar, the course nav, and the layout every signed-in
//! page sits in. Pages are wrapped in [`Shell`], which also sends anyone not
//! logged in to the login page (and back afterwards).

pub mod brand;
pub mod menu;
pub mod nav;
pub mod topbar;

use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};

use crate::session::use_session;

/// A signed-in page: the top bar, and, inside a course, its nav beside the
/// page. Shows the loading mark until the session is known.
#[component]
pub fn Shell(
    #[prop(optional, into)] course: Option<Signal<String>>,
    children: ChildrenFn,
) -> impl IntoView {
    let session = use_session();
    let location = use_location();
    let navigate = use_navigate();
    Effect::new(move || {
        if session.loaded() && session.me().is_none() {
            let here = format!("{}{}", location.pathname.get(), location.search.get());
            navigate(&crate::api::login_url(&here), Default::default());
        }
    });
    move || {
        if session.me().is_none() {
            return view! {
                <div class="auth">
                    <div class="loader" role="status">
                        <brand::Mark size=28 motion=brand::Motion::Loading/>
                        <span>"Loading…"</span>
                    </div>
                </div>
            }
            .into_any();
        }
        let children = children.clone();
        view! {
            <topbar::TopBar course=course/>
            <div class="layout">
                {course.map(|c| view! { <nav::CourseNav course=c/> })}
                <main class="main" id="main">{children()}</main>
            </div>
        }
        .into_any()
    }
}
