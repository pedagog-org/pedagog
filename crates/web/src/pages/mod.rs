//! One module per route.

pub mod assignment;
pub mod course;
pub mod home;
pub mod login;

use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <main class="auth">
            <div class="empty">
                <h1 class="title">"Not found"</h1>
                <p class="muted">"There’s nothing at this address."</p>
                <A href="/" attr:class="btn">"Go home"</A>
            </div>
        </main>
    }
}
