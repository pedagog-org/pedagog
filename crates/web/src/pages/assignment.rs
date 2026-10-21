//! An assignment, for a student: its dates; the results of the submission
//! that counts (or one picked from the history), live while it grades; the
//! history; and submitting, with a check before sending, upload progress
//! and the receipt.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use pedagog_core::api::{
    MyResults, RunState, StudentAssignmentPage, SubmissionRow, SubmitReply, SubmitRules, Window,
};

use crate::api::Problem;
use crate::cache::{Kind, invalidate, use_cached};
use crate::components::results::Results;
use crate::components::steps::{Progress, progress};
use crate::components::{Badge, Failed, Loading, When, moments};
use crate::describe::{history_badge, size, window_badge};
use crate::events::Live;
use crate::shell::Shell;
use crate::upload::{Picked, check};

/// Where the shown submission's run is, from the stream.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Tracked {
    submission: i64,
    status: Option<RunState>,
    position: Option<i64>,
    /// The furthest back in line it's been.
    furthest: Option<i64>,
}

impl Tracked {
    fn apply(&mut self, live: &Live) {
        match *live {
            Live::Position {
                submission,
                queue_position,
            } if submission == self.submission => {
                self.position = Some(queue_position);
                self.furthest = Some(
                    self.furthest
                        .map_or(queue_position, |f| f.max(queue_position)),
                );
            }
            Live::Run {
                submission, status, ..
            } if submission == self.submission => {
                self.status = Some(status);
                if status != RunState::Queued {
                    self.position = None;
                }
            }
            _ => {}
        }
    }
}

#[component]
pub fn Assignment() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let course = Signal::derive(move || params.with(|p| p.get("course").unwrap_or_default()));
    let slug = Signal::derive(move || params.with(|p| p.get("assignment").unwrap_or_default()));
    let picked = Signal::derive(move || {
        query.with(|q| q.get("submission").and_then(|s| s.parse::<i64>().ok()))
    });
    let base = move || format!("/courses/{}/assignments/{}", course.get(), slug.get());

    let page = use_cached(base, Kind::Revalidate, |path| async move {
        crate::api::get::<StudentAssignmentPage>(&path).await
    });
    let results = use_cached(
        move || match picked.get() {
            Some(id) => format!("{}/my-results?submission={id}", base()),
            None => format!("{}/my-results", base()),
        },
        Kind::Revalidate,
        |path| async move {
            // No submissions yet: nothing to show, not a failure.
            match crate::api::get::<MyResults>(&path).await {
                Ok(r) => Ok(Some(r)),
                Err(p) if p.status == 404 => Ok(None),
                Err(p) => Err(p),
            }
        },
    );
    let history = use_cached(
        move || format!("{}/submissions", base()),
        Kind::Revalidate,
        |path| async move { crate::api::get::<Vec<SubmissionRow>>(&path).await },
    );
    let receipt = RwSignal::new(None::<SubmitReply>);
    let tracked = RwSignal::new(Tracked::default());
    let refresh = move || invalidate(&base());

    // Follow the shown submission; refetch when its run finishes.
    Effect::new(move || {
        if let Some(Ok(Some(r))) = results.get() {
            tracked.update(|t| {
                if t.submission != r.submission {
                    *t = Tracked {
                        submission: r.submission,
                        furthest: r.queue_position,
                        ..Default::default()
                    };
                }
                t.position = r.queue_position.or(t.position);
                t.status = Some(r.status);
            });
        }
    });
    Effect::new(move || {
        let (c, a) = (course.get(), slug.get());
        crate::events::watch(
            c,
            a,
            move |live| {
                let before = tracked.get_untracked().status;
                tracked.update(|t| t.apply(&live));
                let now = tracked.get_untracked().status;
                if now != before && matches!(now, Some(RunState::Graded | RunState::Error)) {
                    refresh();
                }
            },
            refresh,
        );
    });

    let title = move || match page.get() {
        Some(Ok(p)) => p.assignment.name,
        _ => slug.get(),
    };
    view! {
        <Shell course=course>
            <nav class="crumbs" aria-label="Breadcrumb">
                <A href=move || format!("/courses/{}", course.get())>"Assignments"</A>
                <span aria-hidden="true">"/"</span>
                <span aria-current="page">{title}</span>
            </nav>
            {move || match page.get() {
                None => view! { <Loading/> }.into_any(),
                Some(Err(p)) => view! { <Failed problem=p/> }.into_any(),
                Some(Ok(p)) => view! { <Header page=p/> }.into_any(),
            }}
            <div style="display: flex; flex-wrap: wrap; gap: 24px; align-items: flex-start">
                <div class="stack g-5" style="flex: 999 1 520px; min-width: 0">
                    {move || {
                        let deadline = match page.get() {
                            Some(Ok(p)) => p.assignment.dates.late_due.or(p.assignment.dates.due),
                            _ => None,
                        };
                        match results.get() {
                            None => view! { <Loading/> }.into_any(),
                            Some(Err(p)) => view! { <Failed problem=p/> }.into_any(),
                            Some(Ok(None)) => ().into_any(),
                            Some(Ok(Some(r))) => view! { <Outcome mine=r tracked=tracked deadline=deadline course=course.get_untracked()/> }.into_any(),
                        }
                    }}
                    <History history=history course=course slug=slug picked=picked/>
                </div>
                {move || match page.get() {
                    Some(Ok(p)) => view! { <Submit page=p receipt=receipt base=Signal::derive(base) refresh=refresh/> }.into_any(),
                    _ => ().into_any(),
                }}
            </div>
        </Shell>
    }
}

#[component]
fn Header(page: StudentAssignmentPage) -> impl IntoView {
    let a = page.assignment;
    let m = moments(&a.dates);
    let now = crate::time::now_millis();
    let due_rel = m.due.as_ref().map(|d| crate::time::relative(d.ms, now));
    view! {
        <header class="stack g-3" style="margin-bottom: 24px">
            <div class="row g-3"><h1 class="title">{a.name}</h1><Badge words=window_badge(a.window, &m)/></div>
            <dl style="display: flex; flex-wrap: wrap; gap: 8px 32px; font-size: 13.5px">
                {a.dates.open.clone().filter(|o| o != "always").map(|o| view! {
                    <div class="stack">
                        <dt class="eyebrow">{if a.window == Window::Upcoming { "Opens" } else { "Opened" }}</dt>
                        <dd style="margin: 0"><When rfc3339=o/></dd>
                    </div>
                })}
                {a.dates.due.clone().map(|d| view! {
                    <div class="stack">
                        <dt class="eyebrow">"Due"</dt>
                        <dd style="margin: 0" class="strong"><When rfc3339=d/>" "<span style="color: var(--accent-text)">{due_rel.map(|r| format!("· {r}"))}</span></dd>
                    </div>
                })}
                {a.dates.late_due.clone().map(|l| view! {
                    <div class="stack">
                        <dt class="eyebrow">"Late until"</dt>
                        <dd style="margin: 0"><When rfc3339=l/>" "<span class="faint">"· then submissions are refused"</span></dd>
                    </div>
                })}
                {(a.dates.due.is_none() && a.window == Window::Open).then(|| view! {
                    <div class="stack"><dt class="eyebrow">"Due"</dt><dd style="margin: 0">"No deadline"</dd></div>
                })}
            </dl>
        </header>
    }
}

/// The shown submission: grading (live), couldn't be graded, or results.
#[component]
fn Outcome(
    mine: MyResults,
    tracked: RwSignal<Tracked>,
    deadline: Option<String>,
    course: String,
) -> impl IntoView {
    let at = crate::time::clock(&mine.counted_at);
    let counts = mine.counts;
    move || {
        let t = tracked.get();
        // Until the stream follows this submission, what my-results said.
        let (status, position, furthest) = if t.submission == mine.submission {
            (t.status.unwrap_or(mine.status), t.position, t.furthest)
        } else {
            (mine.status, mine.queue_position, mine.queue_position)
        };
        match status {
            RunState::Queued | RunState::Grading => view! {
                <section class="card card-pad stack g-4" aria-labelledby="res-h" aria-live="polite">
                    <div class="row between">
                        <h2 class="h2" id="res-h">"Grading your submission"</h2>
                        <span class="badge b-info"><span class="live-dot" aria-hidden="true"></span>"Grading"</span>
                    </div>
                    <Progress steps=progress(status, position, furthest, &at)/>
                    <p class="small muted">{format!("Your results appear here when grading finishes; no need to reload. Your submission from {at} is {}.", if counts { "the one that counts" } else { "not the one that counts" })}</p>
                </section>
            }
            .into_any(),
            RunState::Error => view! {
                <section class="card card-pad stack g-3" aria-labelledby="res-err-h">
                    <div class="row between">
                        <h2 class="h2" id="res-err-h">{format!("Your submission from {at}")}</h2>
                        <span class="badge b-fail"><span class="gl" aria-hidden="true">"✕"</span>"Couldn’t be graded"</span>
                    </div>
                    <p style="font-size: 16px; font-weight: 500">{capitalized(mine.why.as_deref().unwrap_or("it couldn’t be graded"))}"."</p>
                    <p class="muted">{capitalized(mine.next.as_deref().unwrap_or_default())}"."</p>
                    {counts.then(|| view! { <p class="small faint">"This is the submission that counts until you submit again."</p> })}
                </section>
            }
            .into_any(),
            RunState::Graded => match mine.results.clone() {
                // Graded, but the stream got here before the results.
                None => view! { <Loading rows=2/> }.into_any(),
                Some(view) => view! {
                    <section class="stack g-4" aria-labelledby="res-h2">
                        <div class="row between">
                            <h2 class="h2" id="res-h2">"Your results"</h2>
                            <span class="small muted">"from your submission at "<When rfc3339=mine.counted_at.clone()/>
                                {counts.then(|| view! { <span class="badge b-accent" style="margin-left: 6px"><span class="gl" aria-hidden="true">"★"</span>"Counts"</span> })}
                                <DownloadLink href=download_url(&course, mine.submission) label="Download the files of this submission".into() text="Download"/>
                            </span>
                        </div>
                        <Results view=view deadline=deadline.as_deref().and_then(crate::time::moment).map(|m| m.text)/>
                    </section>
                }
                .into_any(),
            },
        }
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// The rows shown at once (the latest, and the one that counts if older),
/// and the rest, latest first.
fn split_history(rows: Vec<SubmissionRow>) -> (Vec<SubmissionRow>, Vec<SubmissionRow>) {
    let mut shown = Vec::new();
    let mut rest = Vec::new();
    for (i, r) in rows.into_iter().enumerate() {
        if i == 0 || r.counts {
            shown.push(r);
        } else {
            rest.push(r);
        }
    }
    (shown, rest)
}

/// Where a submission's files download from (owner and staff).
fn download_url(course: &str, submission: i64) -> String {
    crate::api::url(&format!(
        "/courses/{course}/submissions/{submission}/download"
    ))
}

#[component]
fn History(
    history: ReadSignal<Option<Result<Vec<SubmissionRow>, Problem>>, LocalStorage>,
    course: Signal<String>,
    slug: Signal<String>,
    picked: Signal<Option<i64>>,
) -> impl IntoView {
    let row = move |h: SubmissionRow| view! { <HistoryRow h=h course=course.get_untracked() slug=slug.get_untracked() picked=picked.get_untracked()/> };
    move || match history.get() {
        Some(Ok(rows)) if !rows.is_empty() => {
            let (shown, rest) = split_history(rows);
            let more = rest.len();
            view! {
                <section class="stack g-3" aria-labelledby="hist-h">
                    <h2 class="h2" id="hist-h">"Your submissions"</h2>
                    <div class="card" style="overflow: hidden">
                        <ul style="list-style: none; padding: 0; margin: 0">{shown.into_iter().map(row).collect_view()}</ul>
                        {(more > 0).then(|| view! {
                            <details class="history-more">
                                <summary class="row g-2 small" style="cursor: pointer; padding: 10px 20px">
                                    <svg class="i i-sm chev" viewBox="0 0 24 24" aria-hidden="true"><path d="m9 6 6 6-6 6"/></svg>
                                    {format!("{more} earlier submission{}", if more == 1 { "" } else { "s" })}
                                </summary>
                                <ul style="list-style: none; padding: 0; margin: 0">{rest.into_iter().map(row).collect_view()}</ul>
                            </details>
                        })}
                    </div>
                </section>
            }
            .into_any()
        }
        _ => ().into_any(),
    }
}

#[component]
fn HistoryRow(
    h: SubmissionRow,
    course: String,
    slug: String,
    picked: Option<i64>,
) -> impl IntoView {
    let href = format!("/courses/{course}/assignments/{slug}?submission={}", h.id);
    let current = (picked == Some(h.id)).then_some("true");
    let when = crate::time::moment(&h.counted_at)
        .map(|m| m.text)
        .unwrap_or_default();
    let note = match &h.submitted_by {
        Some(by) => format!("Submitted by {by} on your behalf"),
        None => format!(
            "{} file{} · {}",
            h.files,
            if h.files == 1 { "" } else { "s" },
            size(h.size_bytes)
        ),
    };
    let (late_class, late) = match &h.late {
        Some(l) => ("b-warn", format!("Late by {l}")),
        None => ("b-outline", "On time".to_owned()),
    };
    let label = format!("Download your submission from {when}");
    view! {
        <li class="row" style="border-bottom: 1px solid var(--border); padding-right: 12px">
            <A href=href attr:aria-current=current attr:style="flex: 1; display: flex; flex-wrap: wrap; gap: 6px 20px; align-items: center; padding: 12px 8px 12px 20px; text-decoration: none; color: var(--text); min-width: 0">
                <span class="stack g-1" style="flex: 1 1 240px; min-width: 0">
                    <span class="row g-2"><span class="strong">{when}</span><span class=format!("badge {late_class}")>{late}</span></span>
                    <span class="small muted">{note}</span>
                </span>
                <span class="row g-2">
                    <Badge words=history_badge(h.status, h.shown)/>
                    {h.counts.then(|| view! { <span class="badge b-accent"><span class="gl" aria-hidden="true">"★"</span>"Counts"</span> })}
                </span>
            </A>
            <DownloadLink href=download_url(&course, h.id) label=label/>
        </li>
    }
}

#[component]
fn DownloadLink(
    href: String,
    label: String,
    #[prop(optional)] text: Option<&'static str>,
) -> impl IntoView {
    view! {
        <a class="btn btn-ghost btn-sm" href=href download="" aria-label=label.clone() title=label>
            <svg class="i i-sm" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4v12M7 11l5 5 5-5M4 20h16"/></svg>
            {text}
        </a>
    }
}

/// The submit column: pick, check, send, receipt.
#[component]
fn Submit(
    page: StudentAssignmentPage,
    receipt: RwSignal<Option<SubmitReply>>,
    base: Signal<String>,
    refresh: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let files = RwSignal::new_local(Vec::<Picked>::new());
    let sending = RwSignal::new(None::<f64>);
    let refused = RwSignal::new(None::<String>);
    let over = RwSignal::new(false);
    let rules = page.rules.clone().unwrap_or_default();
    let accepting = page.assignment.accepting;
    let closed_why = page.assignment.status.clone();
    let window = page.assignment.window;
    let due = moments(&page.assignment.dates).due;
    let checked = {
        let rules = rules.clone();
        Memo::new(move |_| {
            let list: Vec<(String, u64)> =
                files.with(|f| f.iter().map(|p| (p.path.clone(), p.size)).collect());
            check(&list, &rules)
        })
    };
    let add = move |more: Vec<Picked>| {
        refused.set(None);
        files.update(|f| {
            for p in more {
                f.retain(|x| x.path != p.path);
                f.push(p);
            }
        });
    };
    let send = move || {
        let list = files.get_untracked();
        if list.is_empty() || !checked.get_untracked().ok() {
            return;
        }
        refused.set(None);
        sending.set(Some(0.0));
        let path = format!("{}/submissions", base.get_untracked());
        leptos::task::spawn_local(async move {
            match crate::upload::send(&path, &list, move |p| sending.set(Some(p))).await {
                Ok(r) => {
                    receipt.set(Some(r));
                    files.set(Vec::new());
                    refresh();
                }
                Err(p) => refused.set(Some(p.message)),
            }
            sending.set(None);
        });
    };
    // Once there's a submission, a small card until asked for the drop zone.
    let submitted = page.assignment.mine.is_some();
    let expanded = RwSignal::new(!submitted);
    let compact =
        move || !expanded.get() && files.with(Vec::is_empty) && receipt.with(Option::is_none);
    let title = move || {
        if receipt.with(Option::is_some) {
            "Receipt"
        } else if submitted {
            "Submit again"
        } else {
            "Submit"
        }
    };
    let rules_line = rules_sentence(&rules);
    let file_input = NodeRef::<leptos::html::Input>::new();
    let folder_input = NodeRef::<leptos::html::Input>::new();
    // Not an attribute leptos knows.
    Effect::new(move || {
        if let Some(i) = folder_input.get() {
            let _ = i.set_attribute("webkitdirectory", "");
        }
    });
    let on_pick = move |input: NodeRef<leptos::html::Input>| {
        if let Some(i) = input.get_untracked() {
            add(crate::upload::from_input(&i));
            i.set_value("");
        }
    };
    view! {
        <aside class="card submit-col" style="flex: 1 1 340px; max-width: 100%" aria-labelledby="sub-h">
            <div class="card-head"><h2 class="h2" id="sub-h">{title}</h2></div>
            <div class="card-pad stack g-4">
                {move || if let Some(r) = receipt.get() {
                    view! { <Receipt reply=r again=move || { receipt.set(None); expanded.set(true); }/> }.into_any()
                } else if !accepting {
                    let why = if window == Window::Upcoming || window == Window::NoDates || window == Window::Closed {
                        format!("Submissions aren’t open: {closed_why}.")
                    } else {
                        "This assignment isn’t ready for submissions yet.".to_owned()
                    };
                    view! { <p class="muted">{why}</p> }.into_any()
                } else if compact() {
                    view! {
                        <div class="stack g-3">
                            <p class="small muted">{rules_line.clone()}" Submitting again replaces which one counts; earlier ones stay in your history."</p>
                            <button class="btn btn-block" type="button" on:click=move |_| expanded.set(true)>
                                <svg class="i" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 16V4M7 9l5-5 5 5M4 20h16"/></svg>
                                "Choose files to submit"
                            </button>
                        </div>
                    }
                    .into_any()
                } else if files.with(Vec::is_empty) {
                    view! {
                        <div class="stack g-3">
                            <div class=move || if over.get() { "drop is-over" } else { "drop" }
                                on:dragover=move |e: web_sys::DragEvent| { e.prevent_default(); over.set(true); }
                                on:dragleave=move |_| over.set(false)
                                on:drop=move |e: web_sys::DragEvent| {
                                    e.prevent_default();
                                    over.set(false);
                                    leptos::task::spawn_local(async move { add(crate::upload::from_drop(&e).await); });
                                }>
                                <span class="dz-ic" aria-hidden="true"><svg class="i" viewBox="0 0 24 24"><path d="M12 16V4M7 9l5-5 5 5M4 20h16"/></svg></span>
                                <span class="strong">"Drop your files or folder here"</span>
                                <span class="small muted">"or one .zip / .tar.gz archive"</span>
                                <div class="row g-2" style="justify-content: center">
                                    <button class="btn btn-sm" type="button" on:click=move |_| { if let Some(i) = file_input.get() { i.click(); } }>"Choose files"</button>
                                    <button class="btn btn-sm" type="button" on:click=move |_| { if let Some(i) = folder_input.get() { i.click(); } }>"Choose folder"</button>
                                </div>
                            </div>
                            <p class="small muted">{rules_line.clone()}" Submitting again replaces which one counts; earlier ones stay in your history."</p>
                        </div>
                    }
                    .into_any()
                } else {
                    view! { <Chosen files=files checked=checked sending=sending refused=refused send=send max=rules.max_bytes due=due.clone()/> }.into_any()
                }}
                <input type="file" multiple class="sr-only" tabindex="-1" aria-hidden="true" node_ref=file_input on:change=move |_| on_pick(file_input)/>
                <input type="file" multiple class="sr-only" tabindex="-1" aria-hidden="true" node_ref=folder_input on:change=move |_| on_pick(folder_input)/>
            </div>
        </aside>
    }
}

/// "Must include linked_list.py. Up to 10.0 MB."
fn rules_sentence(r: &SubmitRules) -> String {
    let mut parts = Vec::new();
    if !r.required.is_empty() {
        parts.push(format!("Must include {}.", r.required.join(", ")));
    }
    if let Some(max) = r.max_bytes {
        parts.push(format!("Up to {}.", size(max)));
    }
    parts.join(" ")
}

#[component]
fn Chosen(
    files: RwSignal<Vec<Picked>, LocalStorage>,
    checked: Memo<crate::upload::Check>,
    sending: RwSignal<Option<f64>>,
    refused: RwSignal<Option<String>>,
    send: impl Fn() + Copy + Send + Sync + 'static,
    max: Option<u64>,
    due: Option<crate::time::Moment>,
) -> impl IntoView {
    let summary = move || {
        let c = checked.get();
        let n = files.with(Vec::len);
        let of = max.map(|m| format!(" of {}", size(m))).unwrap_or_default();
        format!(
            "{n} file{} · {}{of}",
            if n == 1 { "" } else { "s" },
            size(c.total)
        )
    };
    let timing = due.map(|d| {
        let now = crate::time::now_millis();
        if d.ms >= now {
            format!("On time: due {}.", crate::time::relative(d.ms, now))
        } else {
            "Late: the due date has passed.".to_owned()
        }
    });
    view! {
        <div class="stack g-3">
            <ul class="filelist" aria-label="Files to submit">
                {move || files.get().into_iter().map(|f| {
                    let path = f.path.clone();
                    let label = format!("Remove {}", f.path);
                    view! {
                        <li>
                            <span class="fn">{f.path.clone()}</span>
                            <span class="row g-2">
                                <span class="small faint num">{size(f.size)}</span>
                                <button class="btn btn-ghost btn-sm" type="button" aria-label=label
                                    disabled=move || sending.get().is_some()
                                    on:click=move |_| files.update(|l| l.retain(|x| x.path != path))>"✕"</button>
                            </span>
                        </li>
                    }
                }).collect_view()}
            </ul>
            <div class="row between small">
                <span class="muted">{summary}</span>
                <button class="btn-link" type="button" disabled=move || sending.get().is_some() on:click=move |_| { files.set(Vec::new()); refused.set(None); }>"Clear"</button>
            </div>
            {move || {
                let c = checked.get();
                let mut problems = Vec::new();
                if !c.missing.is_empty() {
                    problems.push(format!("missing required files: {}", c.missing.join(", ")));
                }
                if c.over_limit {
                    problems.push(format!("larger than the limit ({})", max.map(size).unwrap_or_default()));
                }
                if let Some(r) = refused.get() {
                    problems.push(r);
                }
                (!problems.is_empty()).then(|| view! {
                    <div class="alert alert-fail" role="alert"><span class="ic" aria-hidden="true">"✕"</span><div class="body">
                        {problems.into_iter().map(|p| view! { <span>{p}</span> }).collect_view()}
                    </div></div>
                })
            }}
            {move || match sending.get() {
                Some(share) => view! {
                    <div class="stack g-2" role="status" aria-live="polite">
                        <div class="row between small"><span class="muted">"Uploading"</span><span class="num">{format!("{:.0}%", share * 100.0)}</span></div>
                        <div class="meter" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow=format!("{:.0}", share * 100.0) aria-label="Upload progress">
                            <span style=format!("width: {:.0}%", share * 100.0)></span>
                        </div>
                    </div>
                }
                .into_any(),
                None => view! {
                    <button class="btn btn-primary btn-lg btn-block" type="button"
                        disabled=move || !checked.get().ok()
                        on:click=move |_| send()>
                        {move || { let n = files.with(Vec::len); format!("Submit {n} file{}", if n == 1 { "" } else { "s" }) }}
                    </button>
                }
                .into_any(),
            }}
            {timing.map(|t| view! { <p class="xs faint">{t}</p> })}
        </div>
    }
}

#[component]
fn Receipt(reply: SubmitReply, again: impl Fn() + 'static) -> impl IntoView {
    let r = reply.submitted.receipt;
    let at = crate::time::clock(&reply.counted_at);
    let on_time = match &reply.late {
        Some(l) => format!("late by {l}"),
        None => "on time".to_owned(),
    };
    let queued = match reply.submitted.queue_position {
        Some(p) => format!("Queued for grading (#{p} in line)."),
        None => "Queued for grading.".to_owned(),
    };
    let mut notes = Vec::new();
    if let Some(a) = &r.unpacked {
        notes.push(format!("Archive {a} unpacked"));
    }
    if let Some(s) = &r.stripped {
        notes.push(format!("wrapping folder {s}/ removed"));
    }
    notes.push(size(r.size_bytes));
    let list = |items: Vec<String>, ignored: bool| {
        view! {
            <ul class="filelist">
                {items.into_iter().map(|f| view! { <li class=ignored.then_some("is-ignored")><span class="fn">{f}</span></li> }).collect_view()}
            </ul>
        }
    };
    view! {
        <div class="stack g-4" role="status">
            <div class="alert alert-pass"><span class="ic" aria-hidden="true">"✓"</span><div class="body">
                <span class="strong">{format!("Submitted at {at} · {on_time}")}</span>
                <span class="small muted">{queued}</span>
            </div></div>
            <div class="stack g-2"><span class="eyebrow">{format!("Kept ({})", r.files.len())}</span>{list(r.files, false)}</div>
            {(!r.ignored.is_empty()).then(|| view! {
                <div class="stack g-2"><span class="eyebrow">{format!("Ignored by the assignment’s rules ({})", r.ignored.len())}</span>{list(r.ignored.clone(), true)}</div>
            })}
            {(r.junk > 0).then(|| view! { <p class="small muted">{format!("Dropped {} file{} of clutter (.DS_Store, __MACOSX/ and the like).", r.junk, if r.junk == 1 { "" } else { "s" })}</p> })}
            <p class="small muted">{notes.join(" · ")}</p>
            <button class="btn btn-block" type="button" on:click=move |_| again()>"Submit again"</button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_moves_only_the_shown_submission() {
        let mut t = Tracked {
            submission: 7,
            ..Default::default()
        };
        t.apply(&Live::Position {
            submission: 7,
            queue_position: 14,
        });
        t.apply(&Live::Position {
            submission: 7,
            queue_position: 3,
        });
        assert_eq!((t.position, t.furthest), (Some(3), Some(14)));
        t.apply(&Live::Run {
            submission: 8,
            status: RunState::Graded,
            student: None,
            score: None,
            total: None,
        });
        assert_eq!(t.status, None, "someone else's run");
        t.apply(&Live::Run {
            submission: 7,
            status: RunState::Grading,
            student: None,
            score: None,
            total: None,
        });
        assert_eq!((t.status, t.position), (Some(RunState::Grading), None));
    }

    #[test]
    fn the_history_shows_the_latest_and_the_one_that_counts() {
        let row = |id, counts| SubmissionRow {
            id,
            counted_at: String::new(),
            late: None,
            submitted_by: None,
            staff: false,
            status: Some(RunState::Graded),
            counts,
            activated_by: None,
            shown: None,
            files: 1,
            size_bytes: 1,
            created_at: None,
            score: None,
            total: None,
        };
        let ids = |rows: &[SubmissionRow]| rows.iter().map(|r| r.id).collect::<Vec<_>>();
        let (shown, rest) = split_history(vec![
            row(4, false),
            row(3, false),
            row(2, true),
            row(1, false),
        ]);
        assert_eq!(
            (ids(&shown), ids(&rest)),
            (vec![4, 2], vec![3, 1]),
            "an activated older one stays in view"
        );
        let (shown, rest) = split_history(vec![row(2, true), row(1, false)]);
        assert_eq!((ids(&shown), ids(&rest)), (vec![2], vec![1]));
        assert_eq!(
            download_url("cop3502-f26", 7),
            "/api/courses/cop3502-f26/submissions/7/download"
        );
    }

    #[test]
    fn the_rules_are_said_in_a_line() {
        let r = SubmitRules {
            required: vec!["linked_list.py".into()],
            max_bytes: Some(10 << 20),
            ..Default::default()
        };
        assert_eq!(
            rules_sentence(&r),
            "Must include linked_list.py. Up to 10.0 MB."
        );
        assert_eq!(rules_sentence(&SubmitRules::default()), "");
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod browser {
    use pedagog_core::api::{Receipt as Kept, Submitted};
    use pedagog_core::results::Status;
    use pedagog_core::view::{ShownTest, StudentView};
    use wasm_bindgen_test::*;

    use super::*;
    use crate::testing::{all, mount, text};

    wasm_bindgen_test_configure!(run_in_browser);

    fn mine(status: RunState, results: Option<StudentView>) -> MyResults {
        MyResults {
            submission: 7,
            counts: true,
            activated: false,
            counted_at: "2026-10-20T15:05:00-04:00".into(),
            submitted_by: None,
            late: None,
            status,
            queue_position: (status == RunState::Queued).then_some(3),
            why: (status == RunState::Error)
                .then(|| "the tests couldn't run with your code".into()),
            next: (status == RunState::Error).then(|| "check your files and submit again".into()),
            results,
        }
    }

    fn graded() -> StudentView {
        let test = |name: &str, status, score, max| ShownTest {
            name: name.into(),
            number: None,
            status,
            score,
            max,
            extra_credit: None,
            feedback: None,
            stdout: None,
            error: Some("AttributeError".into()).filter(|_| status == Status::Fail),
        };
        StudentView {
            tests: vec![
                test("append", Status::Pass, 2.0, 2.0),
                test("pop_front", Status::Fail, 3.0, 4.0),
            ],
            score: 5.0,
            total: 6.0,
            hidden_until: vec!["the deadline".into()],
            hidden_never: false,
        }
    }

    fn outcome(m: MyResults) -> web_sys::HtmlElement {
        mount(move || {
            let tracked = RwSignal::new(Tracked::default());
            view! { <Outcome mine=m tracked=tracked deadline=None course="cop3502-f26".to_owned()/> }
        })
    }

    #[wasm_bindgen_test]
    fn graded_results_show_the_score_and_open_the_failures() {
        let page = outcome(mine(RunState::Graded, Some(graded())));
        assert_eq!(text(&page, ".score .big"), "5");
        assert_eq!(all(&page, "details.test").len(), 2);
        assert_eq!(all(&page, "details.test[open]").len(), 1, "the failing one");
        assert!(text(&page, ".hidden-note").contains("until the deadline"));
        assert_eq!(all(&page, ".badge.b-accent").len(), 1, "it counts");
    }

    #[wasm_bindgen_test]
    fn a_queued_run_shows_its_place_and_an_error_what_to_do() {
        let page = outcome(mine(RunState::Queued, None));
        assert_eq!(text(&page, ".step.now .lbl"), "Queued");
        assert_eq!(text(&page, ".step.now .sub"), "#3 in line");
        let page = outcome(mine(RunState::Error, None));
        assert!(text(&page, "p").starts_with("The tests couldn't run"));
        assert_eq!(all(&page, ".badge.b-fail").len(), 1);
    }

    #[wasm_bindgen_test]
    fn missing_required_files_hold_back_submitting() {
        let page = mount(|| {
            let files = RwSignal::new_local(Vec::<Picked>::new());
            let rules = SubmitRules {
                required: vec!["linked_list.py".into()],
                max_bytes: Some(10 << 20),
                unpack_archives: true,
                strip_wrapper: true,
            };
            let blob = web_sys::File::new_with_str_sequence(
                &js_sys::Array::of1(&"x".into()),
                "linkedlist.py",
            )
            .ok();
            files.set(
                blob.map(|f| Picked {
                    path: "linkedlist.py".into(),
                    size: 1,
                    file: std::rc::Rc::new(f),
                })
                .into_iter()
                .collect(),
            );
            let checked = Memo::new(move |_| {
                let list: Vec<(String, u64)> =
                    files.with(|f| f.iter().map(|p| (p.path.clone(), p.size)).collect());
                check(&list, &rules)
            });
            view! {
                <Chosen files=files checked=checked sending=RwSignal::new(None) refused=RwSignal::new(None)
                    send=|| {} max=Some(10 << 20) due=None/>
            }
        });
        assert!(text(&page, "[role=alert]").contains("missing required files: linked_list.py"));
        let button = &all(&page, "button.btn-primary")[0];
        assert!(button.has_attribute("disabled"));
        assert_eq!(button.text_content().unwrap_or_default(), "Submit 1 file");
    }

    #[wasm_bindgen_test]
    fn the_receipt_lists_what_was_kept_and_ignored() {
        let page = mount(|| {
            let reply = SubmitReply {
                submitted: Submitted {
                    submission_id: 7,
                    run_id: 9,
                    build_id: 1,
                    tree_hash: "t".into(),
                    queue_position: Some(2),
                    receipt: Kept {
                        files: vec!["linked_list.py".into(), "node.py".into()],
                        ignored: vec!["scratch_test.py".into()],
                        junk: 2,
                        unpacked: None,
                        stripped: Some("hw3".into()),
                        size_bytes: 8_300,
                    },
                },
                for_: "ana@ufl.edu".into(),
                submitted_by: None,
                staff: false,
                counted_at: "2026-10-20T15:05:00-04:00".into(),
                late: None,
                due: None,
            };
            view! { <Receipt reply=reply again=|| {}/> }
        });
        assert!(text(&page, ".alert-pass .strong").ends_with("on time"));
        assert_eq!(all(&page, ".filelist li").len(), 3);
        assert_eq!(all(&page, ".filelist li.is-ignored").len(), 1);
        assert!(page.inner_html().contains("wrapping folder hw3/ removed"));
    }
}
