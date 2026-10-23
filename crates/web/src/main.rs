//! Pedagog's web UI: a Leptos app rendered in the browser (built with trunk),
//! calling the api under `/api` on the same host. Styles are the design's
//! `pedagog.css`; components map to its classes.

mod api;
mod cache;
mod components;
mod describe;
mod events;
mod pages;
mod report;
mod session;
mod shell;
mod staff;
mod theme;
mod time;
mod upload;

#[cfg(all(test, target_arch = "wasm32"))]
mod testing;

use leptos::prelude::*;
use leptos_meta::{Title, provide_meta_context};
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

fn main() {
    report::install();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    provide_meta_context();
    theme::provide();
    session::provide();
    view! {
        <Title text="pedagog"/>
        <a class="sr-only" href="#main">"Skip to content"</a>
        <Router>
            <Routes fallback=pages::NotFound>
                <Route path=path!("/login") view=pages::login::Login/>
                <Route path=path!("/") view=pages::home::Home/>
                <Route path=path!("/courses/:course") view=pages::course::Course/>
                <Route path=path!("/courses/:course/upload") view=staff::upload::Upload/>
                <Route path=path!("/courses/:course/assignments/:assignment") view=pages::assignment::Assignment/>
                <Route path=path!("/courses/:course/assignments/:assignment/upload") view=staff::upload::Upload/>
                <Route path=path!("/courses/:course/assignments/:assignment/builds/:build") view=staff::build::Build/>
            </Routes>
        </Router>
    }
}
