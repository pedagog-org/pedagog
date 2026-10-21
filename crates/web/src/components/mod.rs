//! Pieces the pages share, each drawn with pedagog.css's classes.

pub mod results;
pub mod steps;

use leptos::prelude::*;
use pedagog_core::api::DatesView;

use crate::api::Problem;
use crate::describe::{Badge as Words, Moments};

/// A badge: glyph and words in the badge's colour.
#[component]
pub fn Badge(words: Words, #[prop(optional)] style: &'static str) -> impl IntoView {
    view! {
        <span class=format!("badge {}", words.class) style=style>
            <span class="gl" aria-hidden="true">{words.glyph}</span>
            {words.text}
        </span>
    }
}

/// The dates, formatted in the viewer's zone.
pub fn moments(d: &DatesView) -> Moments {
    let at = |s: &Option<String>| {
        s.as_deref()
            .filter(|s| *s != "always")
            .and_then(crate::time::moment)
    };
    Moments {
        open: at(&d.open),
        due: at(&d.due),
        late_due: at(&d.late_due),
    }
}

/// A time with its zone on hover, as `<time>`.
#[component]
pub fn When(rfc3339: String) -> impl IntoView {
    let m = crate::time::moment(&rfc3339).unwrap_or_default();
    view! { <time class="t" datetime=rfc3339 title=m.zone>{m.text}</time> }
}

/// A refusal or failure, said plainly.
#[component]
pub fn Failed(problem: Problem) -> impl IntoView {
    view! {
        <div class="alert alert-fail" role="alert">
            <span class="ic" aria-hidden="true">"✕"</span>
            <div class="body"><span>{problem.message}</span></div>
        </div>
    }
}

/// Grey bars while something loads.
#[component]
pub fn Loading(#[prop(default = 3)] rows: usize) -> impl IntoView {
    view! {
        <div class="card card-pad stack g-3" aria-busy="true">
            {(0..rows)
                .map(|i| view! { <div class="sk sk-build" style=format!("width: {}%", 40 + (i * 17) % 45)></div> })
                .collect_view()}
        </div>
    }
}
