//! Uploading an assignment (a new one, or a new version): its folder or an
//! archive, regrade and dates options, then the build live as it goes, the
//! upload's warnings, and how the file's dates differ from the course's.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use pedagog_core::api::{BuildPage, Uploaded};

use crate::components::steps::Progress;
use crate::components::{Failed, When};
use crate::describe::size;
use crate::shell::Shell;
use crate::upload::Picked;

use super::describe::build_steps;
use super::make_current::{RegradeChoice, Tell};

const ASSIGNMENT_FILE: &str = "assignment.yml";

/// What was picked, in a line: "linked-list/ · 14 files · 212 KB · assignment.yml found".
pub fn picked_summary(paths: &[(String, u64)]) -> (String, bool) {
    let total: u64 = paths.iter().map(|(_, s)| s).sum();
    let archive = matches!(paths, [(only, _)] if [".zip", ".tar.gz", ".tgz"].iter().any(|s| only.to_ascii_lowercase().ends_with(s)));
    if archive {
        return (format!("{} · {}", paths[0].0, size(total)), true);
    }
    let folder = paths
        .first()
        .and_then(|(p, _)| p.split_once('/'))
        .map(|(f, _)| f)
        .filter(|f| paths.iter().all(|(p, _)| p.starts_with(&format!("{f}/"))));
    let found = paths.iter().any(|(p, _)| {
        p == ASSIGNMENT_FILE || folder.is_some_and(|f| *p == format!("{f}/{ASSIGNMENT_FILE}"))
    });
    let name = folder.map_or(String::new(), |f| format!("{f}/ · "));
    let files = if paths.len() == 1 {
        "1 file".to_owned()
    } else {
        format!("{} files", paths.len())
    };
    let said = if found {
        format!("{name}{files} · {} · {ASSIGNMENT_FILE} found", size(total))
    } else {
        format!(
            "{name}{files} · {} · no {ASSIGNMENT_FILE} at the top",
            size(total)
        )
    };
    (said, found)
}

/// The upload's query: `?regrade=changed&take_dates=true`.
pub fn upload_query(regrade: Option<Tell>, take_dates: bool) -> String {
    let mut q = Vec::new();
    if let Some(t) = regrade {
        q.push(format!("regrade={}", t.as_str()));
    }
    if take_dates {
        q.push("take_dates=true".to_owned());
    }
    if q.is_empty() {
        String::new()
    } else {
        format!("?{}", q.join("&"))
    }
}

#[component]
pub fn Upload() -> impl IntoView {
    let params = use_params_map();
    let course = Signal::derive(move || params.with(|p| p.get("course").unwrap_or_default()));
    let existing = Signal::derive(move || params.with(|p| p.get("assignment")));
    let files = RwSignal::new_local(Vec::<Picked>::new());
    let over = RwSignal::new(false);
    let regrade = RwSignal::new(existing.get_untracked().is_some());
    let tell = RwSignal::new(Tell::Changed);
    let take_dates = RwSignal::new(false);
    let sending = RwSignal::new(None::<f64>);
    let refused = RwSignal::new(None::<String>);
    let uploaded = RwSignal::new(None::<Uploaded>);
    let file_input = NodeRef::<leptos::html::Input>::new();
    let folder_input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move || {
        if let Some(i) = folder_input.get() {
            let _ = i.set_attribute("webkitdirectory", "");
        }
    });
    let pick = move |more: Vec<Picked>| {
        refused.set(None);
        if !more.is_empty() {
            files.set(more);
        }
    };
    let on_pick = move |input: NodeRef<leptos::html::Input>| {
        if let Some(i) = input.get_untracked() {
            pick(crate::upload::from_input(&i));
            i.set_value("");
        }
    };
    let summary = Memo::new(move |_| {
        let list: Vec<(String, u64)> =
            files.with(|f| f.iter().map(|p| (p.path.clone(), p.size)).collect());
        (!list.is_empty()).then(|| picked_summary(&list))
    });
    let send = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let list = files.get_untracked();
        if list.is_empty() {
            return;
        }
        refused.set(None);
        sending.set(Some(0.0));
        let query = upload_query(
            regrade.get_untracked().then(|| tell.get_untracked()),
            take_dates.get_untracked(),
        );
        let path = format!("/courses/{}/assignments{query}", course.get_untracked());
        leptos::task::spawn_local(async move {
            match crate::upload::send::<Uploaded>(&path, &list, move |p| sending.set(Some(p))).await
            {
                Ok(u) => {
                    crate::cache::invalidate(&format!(
                        "/courses/{}/assignments",
                        course.get_untracked()
                    ));
                    uploaded.set(Some(u));
                }
                Err(p) => refused.set(Some(p.message)),
            }
            sending.set(None);
        });
    };
    let back = move || match existing.get() {
        Some(a) => format!("/courses/{}/assignments/{a}", course.get()),
        None => format!("/courses/{}", course.get()),
    };
    view! {
        <Shell course=course>
            <div style="max-width: 820px" class="stack g-5">
                <div>
                    <nav class="crumbs" aria-label="Breadcrumb">
                        <A href=move || format!("/courses/{}", course.get())>"Assignments"</A>
                        {move || existing.get().map(|a| view! { <span aria-hidden="true">"/"</span><A href=back()>{a}</A> })}
                        <span aria-hidden="true">"/"</span>
                        <span aria-current="page">"Upload"</span>
                    </nav>
                    <h1 class="title">{move || if existing.get().is_some() { "Upload a new version" } else { "Upload an assignment" }}</h1>
                    <p class="muted" style="margin-top: 4px">"We build it into an image and check its reference solution passes. The current build stays in use until a new one passes."</p>
                </div>
                {move || match uploaded.get() {
                    Some(u) => view! { <Started course=course.get_untracked() up=u/> }.into_any(),
                    None => view! {
                        <form class="stack g-5" on:submit=send>
                            <section class="card card-pad stack g-4" aria-labelledby="up-src">
                                <h2 class="h2" id="up-src">"Assignment"</h2>
                                {move || match summary.get() {
                                    None => view! {
                                        <div class=move || if over.get() { "drop is-over" } else { "drop" } style="padding: 22px"
                                            on:dragover=move |e: web_sys::DragEvent| { e.prevent_default(); over.set(true); }
                                            on:dragleave=move |_| over.set(false)
                                            on:drop=move |e: web_sys::DragEvent| {
                                                e.prevent_default();
                                                over.set(false);
                                                leptos::task::spawn_local(async move { pick(crate::upload::from_drop(&e).await); });
                                            }>
                                            <span class="strong">"Drop the assignment folder or an archive"</span>
                                            <span class="small muted">"The folder with "<span class="mono">"assignment.yml"</span>", or a .tar.gz / .zip of it"</span>
                                            <div class="row g-2" style="justify-content: center">
                                                <button class="btn btn-sm" type="button" on:click=move |_| { if let Some(i) = folder_input.get() { i.click(); } }>"Choose folder"</button>
                                                <button class="btn btn-sm" type="button" on:click=move |_| { if let Some(i) = file_input.get() { i.click(); } }>"Choose archive"</button>
                                            </div>
                                        </div>
                                    }
                                    .into_any(),
                                    Some((said, ok)) => view! {
                                        <div class="row between" style="padding: 10px 12px; border: 1px solid var(--border); border-radius: 8px; background: var(--surface-2)">
                                            <span class="row g-2">
                                                <svg class="i" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>
                                                <span class="small" style=if ok { "" } else { "color: var(--warn-text)" }>{said}</span>
                                            </span>
                                            <button class="btn-link small" type="button" on:click=move |_| files.set(Vec::new())>"Change"</button>
                                        </div>
                                    }
                                    .into_any(),
                                }}
                                <input type="file" accept=".zip,.tgz,.tar.gz,.gz" class="sr-only" tabindex="-1" aria-hidden="true" node_ref=file_input on:change=move |_| on_pick(file_input)/>
                                <input type="file" multiple class="sr-only" tabindex="-1" aria-hidden="true" node_ref=folder_input on:change=move |_| on_pick(folder_input)/>
                            </section>
                            <section class="card card-pad stack g-4" aria-labelledby="up-opt">
                                <h2 class="h2" id="up-opt">"Options"</h2>
                                {move || existing.get().is_some().then(|| view! {
                                    <RegradeChoice regrade=regrade tell=tell hint="Every student’s submission that counts is graded again once the new build passes validation."/>
                                })}
                                <label class="check">
                                    <input type="checkbox" prop:checked=move || take_dates.get() on:change=move |e| take_dates.set(event_target_checked(&e))/>
                                    <span class="stack"><span class="strong">"Use the file’s dates"</span><span class="hint">"Otherwise the course’s dates stay, including any edited here."</span></span>
                                </label>
                            </section>
                            {move || refused.get().map(|m| view! { <Failed problem=crate::api::Problem { status: 400, message: m }/> })}
                            <div class="row g-2">
                                <button class="btn btn-primary btn-lg" type="submit" disabled=move || files.with(Vec::is_empty) || sending.get().is_some()>
                                    {move || match sending.get() {
                                        Some(p) => format!("Uploading… {:.0}%", p * 100.0),
                                        None => "Upload and build".to_owned(),
                                    }}
                                </button>
                                <A href=back attr:class="btn btn-ghost btn-lg">"Cancel"</A>
                            </div>
                        </form>
                    }
                    .into_any(),
                }}
            </div>
        </Shell>
    }
}

/// After the upload: its build, live, and what to know.
#[component]
fn Started(course: String, up: Uploaded) -> impl IntoView {
    let build_id = up.build["build_id"].as_i64().unwrap_or_default();
    let base = format!("/courses/{course}/assignments/{}", up.assignment);
    let path = format!("{base}/builds/{build_id}");
    let build = RwSignal::new(None::<BuildPage>);
    let fetch = {
        let path = path.clone();
        move || {
            let path = path.clone();
            leptos::task::spawn_local(async move {
                if let Ok(b) = crate::api::get::<BuildPage>(&path).await {
                    build.set(Some(b));
                }
            });
        }
    };
    fetch();
    {
        let (fetch, poll) = (fetch.clone(), fetch.clone());
        crate::events::watch(
            course.clone(),
            up.assignment.clone(),
            move |_| fetch(),
            poll,
        );
    }
    let ui_build = path.clone();
    view! {
        <div class="stack g-5">
            <section class="card card-pad stack g-4" aria-live="polite" aria-labelledby="pr-h">
                <div class="row between">
                    <h2 class="h2" id="pr-h">{format!("Build {build_id} started")}</h2>
                    <A href=ui_build attr:class="btn btn-sm">{format!("Open build {build_id}")}</A>
                </div>
                {move || build.get().map(|b| view! { <Progress steps=build_steps(&b.build, b.build_ms) label="Build progress"/> })}
                <p class="small muted">"The current build stays in use until this one passes; you can leave this page."</p>
            </section>
            {(!up.warnings.is_empty()).then(|| view! {
                <div class="alert alert-warn"><span class="ic" aria-hidden="true">"!"</span><div class="body">
                    <span class="strong">{format!("{} {}", up.warnings.len(), if up.warnings.len() == 1 { "warning" } else { "warnings" })}</span>
                    {up.warnings.iter().map(|w| view! { <span class="mono small">{w.clone()}</span> }).collect_view()}
                </div></div>
            })}
            {(!up.date_diff.is_empty()).then(|| view! {
                <section class="card" aria-labelledby="dd-h">
                    <div class="card-head"><h2 class="h2" id="dd-h">"The file’s dates differ from the course’s"</h2></div>
                    <div class="table-wrap" style="border: 0; border-radius: 0">
                        <table class="dt">
                            <thead><tr><th>"Field"</th><th>"Course’s (in use)"</th><th>"File’s"</th></tr></thead>
                            <tbody>
                                {up.date_diff.iter().map(|d| view! {
                                    <tr>
                                        <td class="mono">{d.field.clone()}</td>
                                        <td class="strong">{date_cell(d.course.clone())}</td>
                                        <td class="muted">{date_cell(d.file.clone())}</td>
                                    </tr>
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                    <div class="card-foot"><span class="small muted">{up.dates_note.clone().unwrap_or_default().replace("--take-dates", "“Use the file’s dates”")}</span></div>
                </section>
            })}
        </div>
    }
}

fn date_cell(value: Option<String>) -> AnyView {
    match value {
        Some(v) if v != "always" && crate::time::moment(&v).is_some() => {
            view! { <When rfc3339=v/> }.into_any()
        }
        Some(v) => v.into_any(),
        None => "—".into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picked(list: &[(&str, u64)]) -> Vec<(String, u64)> {
        list.iter().map(|(p, s)| ((*p).to_owned(), *s)).collect()
    }

    #[test]
    fn what_was_picked_is_said_in_a_line() {
        assert_eq!(
            picked_summary(&picked(&[
                ("linked-list/assignment.yml", 1_000),
                ("linked-list/tests/t.py", 2_000)
            ])),
            (
                "linked-list/ · 2 files · 2.9 KB · assignment.yml found".to_owned(),
                true
            )
        );
        assert!(!picked_summary(&picked(&[("hw/notes.txt", 10)])).1);
        assert!(
            picked_summary(&picked(&[("ll.ZIP", 10)])).1,
            "an archive is checked by the server"
        );
    }

    #[test]
    fn the_query_carries_the_options() {
        assert_eq!(upload_query(None, false), "");
        assert_eq!(
            upload_query(Some(Tell::Changed), true),
            "?regrade=changed&take_dates=true"
        );
    }
}
