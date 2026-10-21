//! A course's nav: Assignments for everyone; Members for staff; Settings for
//! instructors (and admins).

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::session::use_session;

/// Which nav items a role sees: (label, path under the course).
pub fn items(role: &str, admin: bool) -> Vec<(&'static str, &'static str)> {
    let mut items = vec![("Assignments", "")];
    if admin || matches!(role, "instructor" | "grader") {
        items.push(("Members", "/members"));
    }
    if admin || role == "instructor" {
        items.push(("Settings", "/settings"));
    }
    items
}

#[component]
pub fn CourseNav(course: Signal<String>) -> impl IntoView {
    let session = use_session();
    let location = use_location();
    let current = move || session.course(&course.get());
    view! {
        <div class="nav-col">
            <nav aria-label="Course">
                <div class="nav-course">
                    <span class="mono xs faint">{move || course.get()}</span>
                    <span class="strong small">{move || current().map(|c| c.name).unwrap_or_default()}</span>
                </div>
                <div class="sidenav">
                    {move || {
                        let role = current().map(|c| c.role).unwrap_or_default();
                        let base = format!("/courses/{}", course.get());
                        items(&role, session.is_admin())
                            .into_iter()
                            .map(|(label, tail)| {
                                let href = format!("{base}{tail}");
                                let here = href.clone();
                                let cur = move || {
                                    let path = location.pathname.get();
                                    let on = if tail.is_empty() { path == here || path.starts_with(&format!("{here}/assignments")) } else { path.starts_with(&here) };
                                    on.then_some("page")
                                };
                                view! { <A href=href attr:class="navlink" attr:aria-current=cur>{label}</A> }
                            })
                            .collect_view()
                    }}
                </div>
            </nav>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_see_their_items() {
        let labels = |role, admin| {
            items(role, admin)
                .into_iter()
                .map(|(l, _)| l)
                .collect::<Vec<_>>()
        };
        assert_eq!(labels("student", false), ["Assignments"]);
        assert_eq!(labels("grader", false), ["Assignments", "Members"]);
        assert_eq!(
            labels("instructor", false),
            ["Assignments", "Members", "Settings"]
        );
        assert_eq!(
            labels("student", true),
            ["Assignments", "Members", "Settings"],
            "admins act as instructors"
        );
    }
}
