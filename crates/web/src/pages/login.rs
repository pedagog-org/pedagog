//! Log in: an email, then the three-word code mailed to it. The api always
//! answers the same to the email step (it doesn't say who's registered); a
//! new code can be sent after a minute. Afterwards, back to where the viewer
//! was (`?next=`), or home.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};

use crate::api::{Problem, safe_next};
use crate::session::use_session;
use crate::shell::brand::Lockup;

/// How long before another code can be sent (the api's limit).
const RESEND_AFTER_S: u32 = 60;

/// `0:42`.
pub fn countdown(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Email,
    Code,
}

#[component]
pub fn Login() -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let query = use_query_map();
    let next = move || safe_next(query.with(|q| q.get("next")).as_deref());
    let returning = move || next() != "/";

    let step = RwSignal::new(Step::Email);
    let email = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let wait = RwSignal::new(0u32);

    let tick = move || {
        wait.set(RESEND_AFTER_S);
        let handle = set_interval_with_handle(
            move || wait.update(|w| *w = w.saturating_sub(1)),
            std::time::Duration::from_secs(1),
        );
        if let Ok(h) = handle {
            on_cleanup(move || h.clear());
        }
    };

    let send = move || {
        busy.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let body = serde_json::json!({ "email": email.get_untracked().trim() });
            match crate::api::post::<serde_json::Value>("/auth/start", &body).await {
                Ok(_) => {
                    step.set(Step::Code);
                    tick();
                }
                Err(p) => error.set(Some(p.message)),
            }
            busy.set(false);
        });
    };

    let verify = {
        let navigate = navigate.clone();
        move || {
            busy.set(true);
            error.set(None);
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                let body = serde_json::json!({
                    "email": email.get_untracked().trim(),
                    "code": code.get_untracked().trim(),
                    "client": "web",
                });
                match crate::api::post::<serde_json::Value>("/auth/finish", &body).await {
                    Ok(_) => {
                        session.reload();
                        navigate(&next(), Default::default());
                    }
                    Err(Problem { message, .. }) => error.set(Some(message)),
                }
                busy.set(false);
            });
        }
    };

    view! {
        <main class="auth">
            <Lockup size=40 shuffle=true/>
            <div class="card auth-card">
                <div class="stack g-1">
                    <h1 class="title">"Log in"</h1>
                    <Show when=returning>
                        <p class="small muted">"Your session ended. Log in and we’ll take you back to where you were."</p>
                    </Show>
                    <Show when=move || step.get() == Step::Email && !returning()>
                        <p class="small muted">"We’ll email you a login code. No password needed."</p>
                    </Show>
                </div>
                {move || match step.get() {
                    Step::Email => view! {
                        <form class="stack g-4" on:submit=move |e| { e.prevent_default(); send(); }>
                            <div class="field">
                                <label class="label" for="login-email">"University email"</label>
                                <input class="input input-lg" id="login-email" type="email" required autocomplete="email" placeholder="you@ufl.edu"
                                    prop:value=move || email.get() on:input=move |e| email.set(event_target_value(&e))/>
                                <Show when=move || error.get().is_some()>
                                    <span class="field-error" role="alert"><span aria-hidden="true">"✕"</span>{move || error.get().unwrap_or_default()}</span>
                                </Show>
                            </div>
                            <button class="btn btn-primary btn-lg btn-block" type="submit" disabled=move || busy.get() aria-busy=move || busy.get().to_string()>
                                {move || if busy.get() { "Sending…" } else { "Send login code" }}
                            </button>
                        </form>
                    }.into_any(),
                    Step::Code => {
                        let verify = verify.clone();
                        view! {
                            <form class="stack g-4" on:submit=move |e| { e.prevent_default(); verify(); }>
                                <div class="alert alert-info" role="status"><span class="ic" aria-hidden="true">"i"</span><div class="body"><span>"If this email is registered, a login code is on its way."</span></div></div>
                                <div class="row between small">
                                    <span class="muted">{move || email.get()}</span>
                                    <button class="btn-link" type="button" on:click=move |_| { step.set(Step::Email); code.set(String::new()); error.set(None); }>"Use a different email"</button>
                                </div>
                                <div class="field">
                                    <label class="label" for="login-code">"Login code"</label>
                                    <input class="input input-lg mono" id="login-code" required autocomplete="one-time-code" autocapitalize="off" spellcheck="false" placeholder="tiger-maple-river"
                                        aria-invalid=move || error.get().is_some().to_string() aria-describedby="login-code-hint login-code-err"
                                        prop:value=move || code.get() on:input=move |e| code.set(event_target_value(&e))/>
                                    <span class="hint" id="login-code-hint">"Three words. Capitals, spaces and dashes don’t matter."</span>
                                    <Show when=move || error.get().is_some()>
                                        <span class="field-error" id="login-code-err" role="alert"><span aria-hidden="true">"✕"</span>{move || error.get().unwrap_or_default()}</span>
                                    </Show>
                                </div>
                                <button class="btn btn-primary btn-lg btn-block" type="submit" disabled=move || busy.get() aria-busy=move || busy.get().to_string()>
                                    {move || if busy.get() { "Logging in…" } else { "Log in" }}
                                </button>
                                <div class="row between small">
                                    <span class="faint">"Didn’t get it? Check spam."</span>
                                    <button class="btn btn-ghost btn-sm" type="button" disabled=move || wait.get() > 0 on:click=move |_| send()>
                                        {move || if wait.get() > 0 { format!("Resend in {}", countdown(wait.get())) } else { "Resend code".into() }}
                                    </button>
                                </div>
                            </form>
                        }.into_any()
                    }
                }}
            </div>
            <p class="xs faint" style="text-align: center; max-width: 420px">"Your instructor adds you to courses. If you can’t log in, ask them to check the email they used."</p>
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdowns_read_as_minutes_and_seconds() {
        assert_eq!(countdown(60), "1:00");
        assert_eq!(countdown(42), "0:42");
        assert_eq!(countdown(5), "0:05");
    }
}
