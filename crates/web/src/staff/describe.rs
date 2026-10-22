//! Builds in words, as the staff pages show them: each phase's badge, how
//! long it took, where an upload's build is, and Home's attention items.

use pedagog_core::api::{BuildSummary, StaffItem};

use crate::components::steps::{Stage, Step};
use crate::describe::Badge;

fn badge(class: &'static str, glyph: &'static str, text: impl Into<String>) -> Badge {
    Badge {
        class,
        glyph,
        text: text.into(),
    }
}

/// The image build: `Running`, `Succeeded`, `Failed`, `Superseded`.
pub fn build_badge(status: &str) -> Badge {
    match status {
        "running" => badge("b-info", "●", "Building"),
        "succeeded" => badge("b-pass", "✓", "Succeeded"),
        "failed" => badge("b-fail", "✕", "Failed"),
        "superseded" => badge("", "↷", "Cancelled"),
        other => badge("", "", other.to_owned()),
    }
}

/// Validation, which only runs once the image built.
pub fn validation_badge(b: &BuildSummary) -> Option<Badge> {
    if b.build_status != "succeeded" && b.validation_status == "pending" {
        return None;
    }
    Some(match b.validation_status.as_str() {
        "pending" => badge("", "○", "Queued"),
        "running" => badge("b-info", "●", "Validating"),
        "passed" => badge("b-pass", "✓", "Passed"),
        "failed" => badge("b-fail", "✕", "Failed"),
        "skipped" => badge("", "–", "Skipped"),
        "invalid" => badge("b-warn", "!", "Invalid"),
        other => badge("", "", other.to_owned()),
    })
}

/// `48s`, `1m 52s`, `1h 3m`.
pub fn duration(ms: i64) -> String {
    let s = (ms.max(0) + 500) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

/// "1m 52s build · 41s validation", "48s, then failed".
pub fn took(build_status: &str, build_ms: Option<i64>, validation_ms: Option<i64>) -> String {
    match (build_ms, validation_ms) {
        (Some(b), _) if build_status == "failed" => format!("{}, then failed", duration(b)),
        (Some(b), Some(v)) => format!("{} build · {} validation", duration(b), duration(v)),
        (Some(b), None) => format!("{} build", duration(b)),
        (None, _) => "—".to_owned(),
    }
}

/// Where a build is: built, validated, current (live on the upload page).
pub fn build_steps(b: &BuildSummary, build_ms: Option<i64>) -> Vec<Step> {
    let step = |stage, label, sub: String| Step { stage, label, sub };
    let built = b.build_status == "succeeded";
    let validated = b.finished && built;
    let mut steps = vec![match b.build_status.as_str() {
        "running" => step(Stage::Now, "Building", "building the image".into()),
        "succeeded" => step(
            Stage::Done,
            "Built",
            build_ms.map(duration).unwrap_or_default(),
        ),
        "failed" => step(Stage::Done, "Build failed", "see the log".into()),
        _ => step(Stage::Done, "Cancelled", "a newer build replaced it".into()),
    }];
    steps.push(match (built, b.validation_status.as_str()) {
        (false, _) => step(Stage::Pending, "Validate", String::new()),
        (true, "pending") => step(Stage::Now, "Queued for validation", String::new()),
        (true, "running") => step(
            Stage::Now,
            "Validating",
            "running the reference solution".into(),
        ),
        (true, "skipped") => step(
            Stage::Done,
            "Validation skipped",
            "no reference solution".into(),
        ),
        (true, "passed") => step(Stage::Done, "Validated", String::new()),
        (true, _) => step(
            Stage::Done,
            "Validation failed",
            "see the mismatches".into(),
        ),
    });
    let usable = validated && matches!(b.validation_status.as_str(), "passed" | "skipped");
    steps.push(if usable {
        step(
            Stage::Done,
            "Current",
            "students' submissions use it".into(),
        )
    } else if b.finished {
        step(
            Stage::Pending,
            "Not current",
            "the current build stays".into(),
        )
    } else {
        step(Stage::Pending, "Current", "once it passes".into())
    });
    steps
}

/// A Home item: what it says, and the page it opens (under the course).
pub fn staff_item(item: &StaffItem) -> (String, String) {
    let page = |slug: &str| format!("/assignments/{slug}");
    match item {
        StaffItem::BuildFailed {
            slug,
            name,
            build,
            validation,
        } => (
            format!(
                "{name}: build {build} {}",
                if *validation {
                    "failed validation"
                } else {
                    "failed to build"
                }
            ),
            format!("{}/builds/{build}", page(slug)),
        ),
        StaffItem::NotGraded { slug, name, count } => (
            format!(
                "{name}: {count} {} couldn’t be graded",
                if *count == 1 {
                    "submission"
                } else {
                    "submissions"
                }
            ),
            page(slug),
        ),
        StaffItem::DueSoon {
            slug,
            name,
            due,
            submitted,
            students,
        } => (
            format!(
                "{name}: due {} · {submitted} of {students} submitted",
                crate::time::moment(due).map(|m| m.text).unwrap_or_default()
            ),
            page(slug),
        ),
        StaffItem::Building {
            slug,
            name,
            build,
            validating,
        } => (
            format!(
                "{name}: build {build} {}",
                if *validating {
                    "validating"
                } else {
                    "building"
                }
            ),
            format!("{}/builds/{build}", page(slug)),
        ),
        StaffItem::Regrading {
            slug,
            name,
            done,
            total,
        } => (format!("{name}: regrading {done} of {total}"), page(slug)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(build: &str, validation: &str, finished: bool) -> BuildSummary {
        BuildSummary {
            id: 14,
            build_status: build.into(),
            validation_status: validation.into(),
            finished,
            current: false,
            started_at: "2026-10-20T13:58:00Z".into(),
            uploaded_by: None,
            rebuild_of: None,
            image_deleted: false,
            usable: false,
        }
    }

    #[test]
    fn durations_read_like_a_clock() {
        assert_eq!(duration(48_200), "48s");
        assert_eq!(duration(112_000), "1m 52s");
        assert_eq!(duration(3_780_000), "1h 3m");
        assert_eq!(
            took("succeeded", Some(112_000), Some(41_000)),
            "1m 52s build · 41s validation"
        );
        assert_eq!(took("failed", Some(48_000), None), "48s, then failed");
        assert_eq!(took("running", None, None), "—");
    }

    #[test]
    fn validation_has_no_badge_until_the_image_built() {
        assert_eq!(validation_badge(&build("failed", "pending", true)), None);
        assert_eq!(
            validation_badge(&build("succeeded", "running", false))
                .unwrap()
                .text,
            "Validating"
        );
        assert_eq!(build_badge("superseded").text, "Cancelled");
    }

    #[test]
    fn steps_follow_the_build() {
        let stages = |b: &BuildSummary| {
            build_steps(b, Some(112_000))
                .into_iter()
                .map(|s| (s.stage, s.label))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            stages(&build("running", "pending", false)),
            [
                (Stage::Now, "Building"),
                (Stage::Pending, "Validate"),
                (Stage::Pending, "Current")
            ]
        );
        assert_eq!(
            stages(&build("succeeded", "running", false))[1],
            (Stage::Now, "Validating")
        );
        assert_eq!(
            stages(&build("succeeded", "passed", true))[2],
            (Stage::Done, "Current")
        );
        assert_eq!(
            stages(&build("succeeded", "failed", true))[2],
            (Stage::Pending, "Not current")
        );
        assert_eq!(
            build_steps(&build("succeeded", "passed", true), Some(112_000))[0].sub,
            "1m 52s"
        );
    }

    #[test]
    fn home_items_say_what_needs_staff() {
        let failed = StaffItem::BuildFailed {
            slug: "ll".into(),
            name: "Linked lists".into(),
            build: 14,
            validation: true,
        };
        assert_eq!(
            staff_item(&failed),
            (
                "Linked lists: build 14 failed validation".into(),
                "/assignments/ll/builds/14".into()
            )
        );
        let ungraded = StaffItem::NotGraded {
            slug: "ll".into(),
            name: "Linked lists".into(),
            count: 1,
        };
        assert_eq!(
            staff_item(&ungraded).0,
            "Linked lists: 1 submission couldn’t be graded"
        );
    }
}
