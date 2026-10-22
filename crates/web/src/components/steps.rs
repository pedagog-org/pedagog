//! Grading progress: received, queued (where in line), grading, graded.

use leptos::prelude::*;
use pedagog_core::api::RunState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Done,
    Now,
    Pending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub stage: Stage,
    pub label: &'static str,
    pub sub: String,
}

/// The steps for a run in `status`, at `position` in line now, having been
/// as far back as `furthest` (so a run past the queue can say "was #14").
pub fn progress(
    status: RunState,
    position: Option<i64>,
    furthest: Option<i64>,
    received: &str,
) -> Vec<Step> {
    let step = |stage, label, sub: String| Step { stage, label, sub };
    let queued = status == RunState::Queued;
    vec![
        step(Stage::Done, "Received", received.to_owned()),
        if queued {
            step(
                Stage::Now,
                "Queued",
                position.map_or("waiting for a grader".into(), |p| format!("#{p} in line")),
            )
        } else {
            step(
                Stage::Done,
                "Queued",
                furthest.map_or(String::new(), |p| format!("was #{p}")),
            )
        },
        step(
            if queued { Stage::Pending } else { Stage::Now },
            "Grading",
            "running your tests".into(),
        ),
        step(Stage::Pending, "Graded", "usually under a minute".into()),
    ]
}

#[component]
pub fn Progress(
    steps: Vec<Step>,
    #[prop(default = "Grading progress")] label: &'static str,
) -> impl IntoView {
    view! {
        <ol class="steps" aria-label=label>
            {steps.into_iter().map(|s| {
                let (class, current) = match s.stage {
                    Stage::Done => ("step done", None),
                    Stage::Now => ("step now", Some("step")),
                    Stage::Pending => ("step pending", None),
                };
                view! {
                    <li class=class aria-current=current>
                        <span class="bar"></span>
                        <span class="lbl">
                            {match s.stage {
                                Stage::Done => view! { <span class="gl" aria-hidden="true">"✓"</span> }.into_any(),
                                Stage::Now => view! { <span class="spinner" aria-hidden="true"></span> }.into_any(),
                                Stage::Pending => ().into_any(),
                            }}
                            {s.label}
                        </span>
                        <span class="sub">{s.sub}</span>
                    </li>
                }
            }).collect_view()}
        </ol>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queued_run_says_where_it_is_and_a_running_one_where_it_was() {
        let queued = progress(RunState::Queued, Some(3), Some(14), "3:05 PM");
        assert_eq!(queued[0].sub, "3:05 PM");
        assert_eq!(
            (queued[1].stage, queued[1].sub.as_str()),
            (Stage::Now, "#3 in line")
        );
        assert_eq!(queued[2].stage, Stage::Pending);

        let grading = progress(RunState::Grading, None, Some(14), "3:05 PM");
        assert_eq!(
            (grading[1].stage, grading[1].sub.as_str()),
            (Stage::Done, "was #14")
        );
        assert_eq!(grading[2].stage, Stage::Now);
        assert_eq!(
            progress(RunState::Queued, None, None, "")[1].sub,
            "waiting for a grader"
        );
    }
}
