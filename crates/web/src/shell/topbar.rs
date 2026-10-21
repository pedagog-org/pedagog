//! The top bar: the mark (it builds in), the course switcher (inside a
//! course), Admin for admins, the theme toggle, and the account menu.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;

use super::brand::{Mark, Motion};
use super::menu::dropdown;
use crate::session::{initials, use_session};
use crate::theme::{Choice, use_theme};

fn role_chip(role: &str) -> &'static str {
    match role {
        "instructor" => "chip chip-instructor",
        "grader" => "chip chip-grader",
        "admin" => "chip chip-admin",
        "staff" => "chip chip-staff",
        _ => "chip chip-student",
    }
}

#[component]
pub fn TopBar(course: Option<Signal<String>>) -> impl IntoView {
    let session = use_session();
    view! {
        <header>
            <div class="topbar">
                <A href="/" attr:class="brand" attr:style="align-items: flex-start">
                    <Mark size=26 motion=Motion::BuildIn/>
                    <span class="hide-sm" style="font-size: 19.2px; line-height: 1; margin-top: 1px; font-weight: 700; letter-spacing: -0.025em">"pedagog"</span>
                    <span class="sr-only">"Pedagog home"</span>
                </A>
                {course.map(|c| view! { <Switcher course=c/> })}
                <div class="spacer"></div>
                <Show when=move || session.is_admin()>
                    <A href="/admin/users" attr:class="btn btn-ghost btn-sm hide-sm">"Admin"</A>
                </Show>
                <ThemeToggle/>
                <UserMenu/>
            </div>
        </header>
    }
}

#[component]
fn Switcher(course: Signal<String>) -> impl IntoView {
    let session = use_session();
    let menu = dropdown();
    let current = move || session.course(&course.get());
    view! {
        <div node_ref=menu.root style="position: relative; min-width: 0">
            <button
                class="switcher"
                type="button"
                aria-haspopup="menu"
                aria-expanded=move || menu.open.get().to_string()
                aria-label=move || format!("Switch course, current: {}", course.get())
                on:click=move |_| menu.open.update(|o| *o = !*o)
            >
                <span class="mono small hide-sm" style="color: var(--text-2)">{move || course.get()}</span>
                <span class="cname">{move || current().map(|c| c.name).unwrap_or_default()}</span>
                {move || current().map(|c| view! { <span class=format!("{} hide-sm", role_chip(&c.role))>{c.role.clone()}</span> })}
                <svg class="i i-sm" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"></path></svg>
            </button>
            <Show when=move || menu.open.get()>
                <div class="menu" role="menu" style="position: absolute; top: 42px; left: 0; z-index: 20; min-width: 320px">
                    {move || {
                        session
                            .me()
                            .map(|me| me.courses)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|c| {
                                let active = c.id == course.get();
                                view! {
                                    <A href=format!("/courses/{}", c.id) attr:class=if active { "menu-item is-active" } else { "menu-item" } attr:role="menuitem" on:click=move |_| menu.open.set(false)>
                                        <span class="stack">
                                            <span class="small strong">{c.name.clone()}</span>
                                            <span class="mono xs faint">{c.id.clone()}</span>
                                        </span>
                                        <span class=role_chip(&c.role)>{c.role.clone()}</span>
                                    </A>
                                }
                            })
                            .collect_view()
                    }}
                </div>
            </Show>
        </div>
    }
}

#[component]
fn ThemeToggle() -> impl IntoView {
    let theme = use_theme();
    let label = move || {
        let (now, other) = if theme.is_dark() {
            ("dark", "light")
        } else {
            ("light", "dark")
        };
        format!("Theme: {now}. Switch to {other}")
    };
    view! {
        <button class="btn btn-ghost icon-btn" type="button" aria-label=label title="Theme follows your system; click to switch" on:click=move |_| theme.toggle()>
            {move || if theme.is_dark() {
                view! { <svg class="i" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4"></circle><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"></path></svg> }.into_any()
            } else {
                view! { <svg class="i" viewBox="0 0 24 24" aria-hidden="true"><path d="M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5Z"></path></svg> }.into_any()
            }}
        </button>
    }
}

#[component]
fn UserMenu() -> impl IntoView {
    let session = use_session();
    let theme = use_theme();
    let menu = dropdown();
    let navigate = use_navigate();
    let user = move || session.me().map(|m| m.user);
    let name = move || {
        user()
            .map(|u| if u.name.is_empty() { u.email } else { u.name })
            .unwrap_or_default()
    };
    let log_out = move |_| {
        let navigate = navigate.clone();
        leptos::task::spawn_local(async move {
            let _ = crate::api::post::<()>("/auth/logout", &serde_json::json!({})).await;
            session.forget();
            crate::cache::invalidate("");
            navigate("/login", Default::default());
        });
    };
    let seg = move |choice: Choice, label: &'static str| {
        view! {
            <button type="button" aria-pressed=move || (theme.choice.get() == choice).to_string() on:click=move |_| theme.set(choice)>{label}</button>
        }
    };
    view! {
        <div node_ref=menu.root style="position: relative">
            <button
                class="userbtn"
                type="button"
                aria-haspopup="menu"
                aria-expanded=move || menu.open.get().to_string()
                aria-label=move || format!("Account menu for {}", name())
                on:click=move |_| menu.open.update(|o| *o = !*o)
            >
                <span class="avatar" aria-hidden="true">{move || user().map(|u| initials(&u.name, &u.email)).unwrap_or_default()}</span>
                <span class="hide-sm">{name}</span>
                <svg class="i i-sm hide-sm" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"></path></svg>
            </button>
            <Show when=move || menu.open.get()>
                <div class="menu" role="menu" style="position: absolute; top: 42px; right: 0; z-index: 20; min-width: 260px">
                    <div class="stack g-1" style="padding: 8px 10px 10px">
                        <span class="strong">{name}</span>
                        <span class="small muted">{move || user().map(|u| u.email).unwrap_or_default()}</span>
                        <span class="row g-2">
                            <span class=move || role_chip(&user().map(|u| u.role).unwrap_or_default())>{move || user().map(|u| u.role).unwrap_or_default()}</span>
                            <span class="xs faint">"global role"</span>
                        </span>
                    </div>
                    <div class="menu-sep"></div>
                    <div class="stack g-2" style="padding: 6px 10px">
                        <span class="xs faint">"Theme"</span>
                        <div class="seg" role="group" aria-label="Theme">
                            {seg(Choice::System, "System")}
                            {seg(Choice::Light, "Light")}
                            {seg(Choice::Dark, "Dark")}
                        </div>
                    </div>
                    <div class="menu-sep"></div>
                    <button class="menu-item" type="button" role="menuitem" style="width: 100%; border: 0; background: none; text-align: left; font: inherit; cursor: pointer" on:click=log_out.clone()>
                        <span class="small">"Log out"</span>
                    </button>
                </div>
            </Show>
        </div>
    }
}
