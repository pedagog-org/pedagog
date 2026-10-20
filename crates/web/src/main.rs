//! Pedagog's web UI: a Leptos app rendered in the browser (built with trunk),
//! calling the api under `/api` on the same host. A skeleton for now: it shows
//! whether the api answers.

use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let api = LocalResource::new(check_api);
    view! {
        <main>
            <h1>"Pedagog"</h1>
            {move || {
                let (class, text) = match api.get() {
                    None => ("status", "checking the api…"),
                    Some(Api::Up) => ("status ok", "the api is up"),
                    Some(Api::Down) => ("status bad", "the api is unreachable"),
                };
                view! { <p class=class role="status">{text}</p> }
            }}
        </main>
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Api {
    Up,
    Down,
}

async fn check_api() -> Api {
    let reply = gloo_net::http::Request::get("/api/healthz").send().await;
    match reply {
        Ok(r) => api_from(r.status(), &r.text().await.unwrap_or_default()),
        Err(_) => Api::Down,
    }
}

/// The api's health answer (`200 ok`) means it's up; anything else, down.
fn api_from(status: u16, body: &str) -> Api {
    if status == 200 && body.trim() == "ok" {
        Api::Up
    } else {
        Api::Down
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_healthy_answer_is_up() {
        assert_eq!(api_from(200, "ok"), Api::Up);
        assert_eq!(
            api_from(200, "<html>"),
            Api::Down,
            "e.g. the dev server's fallback page"
        );
        assert_eq!(api_from(502, ""), Api::Down);
    }
}
