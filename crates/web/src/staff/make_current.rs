//! "Make current…": an earlier build back in use, with a regrade if asked.
//! Whatever is building is cancelled; a build whose image was cleaned up is
//! built again first.

use leptos::prelude::*;
use pedagog_core::api::{BuildSummary, MadeCurrent};

/// What the dialog warns about before anything happens.
pub fn notes(build: &BuildSummary, in_flight: Option<i64>) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(flying) = in_flight {
        notes.push(format!(
            "This cancels build {flying}, which is still building or validating."
        ));
    }
    if build.image_deleted {
        notes.push(format!(
            "Build {}’s image was cleaned up, so it’s built again first, and becomes current once it validates.",
            build.id
        ));
    }
    notes
}

/// What happened, in a sentence.
pub fn outcome(build: i64, made: &MadeCurrent) -> String {
    match made {
        MadeCurrent::Current { .. } => format!("Build {build} is current now."),
        MadeCurrent::Rebuilding { build: new, .. } => format!(
            "Build {build} is being built again as build {new}; it becomes current once it validates."
        ),
    }
}

/// The regrade choice: `None` for no regrade, else who's told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tell {
    Never,
    Changed,
    Always,
}

impl Tell {
    pub fn as_str(self) -> &'static str {
        match self {
            Tell::Never => "never",
            Tell::Changed => "changed",
            Tell::Always => "always",
        }
    }
}

/// Regrade every submission, and whom to tell: the upload form's and this dialog's.
#[component]
pub fn RegradeChoice(
    regrade: RwSignal<bool>,
    tell: RwSignal<Tell>,
    #[prop(into)] hint: String,
) -> impl IntoView {
    let seg = move |value: Tell, label: &'static str| {
        view! {
            <button type="button" aria-pressed=move || (tell.get() == value).to_string()
                on:click=move |_| tell.set(value)>{label}</button>
        }
    };
    view! {
        <div class="stack g-3">
            <label class="check">
                <input type="checkbox" prop:checked=move || regrade.get() on:change=move |e| regrade.set(event_target_checked(&e))/>
                <span class="stack"><span class="strong">"Regrade every student’s submission"</span><span class="hint">{hint}</span></span>
            </label>
            {move || regrade.get().then(|| view! {
                <div class="stack g-2" style="margin-left: 26px">
                    <span class="label" id="tell-l">"Tell students"</span>
                    <div class="seg" role="group" aria-labelledby="tell-l">
                        {seg(Tell::Never, "Never")}
                        {seg(Tell::Changed, "If their results changed")}
                        {seg(Tell::Always, "Always")}
                    </div>
                </div>
            })}
        </div>
    }
}

#[component]
pub fn MakeCurrent(
    base: String,
    target: BuildSummary,
    in_flight: Option<i64>,
    students: i64,
    close: impl Fn() + Copy + Send + Sync + 'static,
    done: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let regrade = RwSignal::new(false);
    let tell = RwSignal::new(Tell::Changed);
    let busy = RwSignal::new(false);
    let refused = RwSignal::new(None::<String>);
    let made = RwSignal::new(None::<String>);
    let id = target.id;
    let warnings = notes(&target, in_flight);
    let path = format!("{base}/builds/{id}/current");
    let go = move |_| {
        busy.set(true);
        refused.set(None);
        let path = path.clone();
        let body = serde_json::json!({
            "regrade": regrade.get_untracked().then(|| tell.get_untracked().as_str()),
        });
        leptos::task::spawn_local(async move {
            match crate::api::post::<MadeCurrent>(&path, &body).await {
                Ok(m) => {
                    made.set(Some(outcome(id, &m)));
                    done();
                }
                Err(p) => refused.set(Some(p.message)),
            }
            busy.set(false);
        });
    };
    view! {
        <div class="backdrop">
            <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="mc-h">
                <h2 class="dialog-h" id="mc-h">{format!("Make build {id} current?")}</h2>
                <div class="dialog-b stack g-4">
                    {move || match made.get() {
                        Some(said) => view! { <p role="status">{said}</p> }.into_any(),
                        None => view! {
                            <p>"Students’ new submissions are graded with it. Dates and visibility don’t change."</p>
                            {warnings.iter().map(|w| view! {
                                <div class="alert alert-warn"><span class="ic" aria-hidden="true">"!"</span><div class="body"><span>{w.clone()}</span></div></div>
                            }).collect_view()}
                            <RegradeChoice regrade=regrade tell=tell hint=format!("Every student’s submission that counts ({students}) is graded again with it.")/>
                            {move || refused.get().map(|m| view! {
                                <div class="alert alert-fail" role="alert"><span class="ic" aria-hidden="true">"✕"</span><div class="body"><span>{m}</span></div></div>
                            })}
                        }
                        .into_any(),
                    }}
                </div>
                <div class="dialog-f">
                    {move || if made.with(Option::is_some) {
                        view! { <button class="btn btn-primary" type="button" on:click=move |_| close()>"Done"</button> }.into_any()
                    } else {
                        view! {
                            <button class="btn btn-ghost" type="button" on:click=move |_| close()>"Cancel"</button>
                            <button class="btn btn-primary" type="button" disabled=move || busy.get() on:click=go.clone()>{format!("Make build {id} current")}</button>
                        }
                        .into_any()
                    }}
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(image_deleted: bool) -> BuildSummary {
        BuildSummary {
            id: 12,
            build_status: "succeeded".into(),
            validation_status: "passed".into(),
            finished: true,
            current: false,
            started_at: "2026-10-14T19:10:00Z".into(),
            uploaded_by: None,
            rebuild_of: None,
            image_deleted,
            usable: true,
        }
    }

    #[test]
    fn the_dialog_warns_about_what_it_cancels_and_rebuilds() {
        assert!(notes(&build(false), None).is_empty());
        let both = notes(&build(true), Some(15));
        assert_eq!(both.len(), 2);
        assert!(both[0].contains("cancels build 15"));
        assert!(both[1].contains("built again first"));
        assert_eq!(
            outcome(
                12,
                &MadeCurrent::Rebuilding {
                    build: 16,
                    cancelled: None
                }
            ),
            "Build 12 is being built again as build 16; it becomes current once it validates."
        );
    }
}
