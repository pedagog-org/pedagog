//! What each audience sees of a run's results, given each test's resolved
//! visibility: the cumulative score columns staff see (and which one a
//! student sees now), and the student's own filtered view.

use serde::Serialize;

use crate::results::{Results, Status, Step, TestResult, TestVisibility, Visibility};

/// The stages results show in, each including the ones before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Tests whose score is visible.
    Visible,
    /// … and those shown after the deadline.
    AfterDeadline,
    /// … and those shown on release (everything but hidden).
    AfterRelease,
    /// Every test.
    Hidden,
}

impl Stage {
    pub const ALL: [Stage; 4] = [
        Stage::Visible,
        Stage::AfterDeadline,
        Stage::AfterRelease,
        Stage::Hidden,
    ];

    /// The first stage a test whose score has this visibility counts in.
    fn of(v: Visibility) -> Stage {
        match v {
            Visibility::Visible => Stage::Visible,
            Visibility::AfterEnd => Stage::AfterDeadline,
            Visibility::AfterPublish => Stage::AfterRelease,
            Visibility::Hidden => Stage::Hidden,
        }
    }

    /// The stage a student sees: release shows everything but hidden.
    pub fn now(ended: bool, released: bool) -> Stage {
        if released {
            Stage::AfterRelease
        } else if ended {
            Stage::AfterDeadline
        } else {
            Stage::Visible
        }
    }
}

/// Score and passed tests over the tests counted at one stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Tally {
    pub score: f64,
    pub total: f64,
    pub passed: usize,
    pub tests: usize,
}

/// One tally per stage, cumulative.
pub fn tallies(results: &Results, visibility: &[TestVisibility]) -> Vec<(Stage, Tally)> {
    Stage::ALL
        .iter()
        .map(|&stage| {
            let mut t = Tally::default();
            for (test, v) in results.tests.iter().zip(visibility) {
                if Stage::of(v.score) <= stage {
                    t.score += test.score;
                    t.total += test.max;
                    t.tests += 1;
                    t.passed += usize::from(test.status == Status::Pass);
                }
            }
            (stage, t)
        })
        .collect()
}

/// A test as a student sees it: parts they may not see yet are left out.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShownTest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    pub status: Status,
    pub score: f64,
    pub max: f64,
    /// Each item's steps, when feedback is shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<Vec<ShownItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShownItem {
    pub name: String,
    pub score: f64,
    pub max: f64,
    pub steps: Vec<Step>,
    /// Version 1 helpers' text feedback.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
}

/// What a student sees of a run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StudentView {
    pub tests: Vec<ShownTest>,
    /// Over the tests shown.
    pub score: f64,
    pub total: f64,
    /// When the tests not shown will be: "the deadline", "release", or
    /// nothing for tests never shown. No counts or points.
    pub hidden_until: Vec<String>,
    pub hidden_never: bool,
}

fn feedback_of(t: &TestResult) -> Vec<ShownItem> {
    let mut items: Vec<ShownItem> = t
        .items
        .iter()
        .map(|i| ShownItem {
            name: i.name.clone(),
            score: i.score,
            max: i.max,
            steps: i.steps.clone(),
            text: i.feedback.clone(),
        })
        .collect();
    if items.is_empty() && !t.feedback.is_empty() {
        // A version 1 single-item test: its feedback text at the test level.
        items.push(ShownItem {
            name: t.name.clone(),
            score: t.score,
            max: t.max,
            steps: Vec::new(),
            text: t.feedback.clone(),
        });
    }
    items
}

/// A student's view: tests whose score they see now, each with the parts
/// they see; the rest summed up as when they'll show.
pub fn student_view(
    results: &Results,
    visibility: &[TestVisibility],
    ended: bool,
    released: bool,
) -> StudentView {
    let shown = |v: Visibility| v.shown(ended, released);
    let mut view = StudentView {
        tests: Vec::new(),
        score: 0.0,
        total: 0.0,
        hidden_until: Vec::new(),
        hidden_never: false,
    };
    let mut until_deadline = false;
    let mut until_release = false;
    for (t, v) in results.tests.iter().zip(visibility) {
        if !shown(v.score) {
            match v.score {
                Visibility::AfterEnd => until_deadline = true,
                Visibility::AfterPublish => until_release = true,
                _ => view.hidden_never = true,
            }
            continue;
        }
        view.score += t.score;
        view.total += t.max;
        view.tests.push(ShownTest {
            name: t.name.clone(),
            number: t.number.clone(),
            status: t.status,
            score: t.score,
            max: t.max,
            feedback: shown(v.feedback).then(|| feedback_of(t)),
            stdout: (shown(v.stdout) && !t.stdout.is_empty()).then(|| t.stdout.clone()),
            error: (shown(v.error) && !t.error.is_empty()).then(|| t.error.clone()),
        });
    }
    if until_deadline {
        view.hidden_until.push("the deadline".to_owned());
    }
    if until_release {
        view.hidden_until.push("release".to_owned());
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results() -> Results {
        serde_json::from_value(serde_json::json!({
            "summary": {"score": 8.0, "total": 11.0},
            "tests": [
                {"node": "a", "name": "push_pop", "status": "pass", "score": 2.0, "max": 2.0,
                 "stdout": "pushed 3\n", "error": "",
                 "items": [{"id": "p", "name": "push", "score": 2.0, "max": 2.0, "clamp": [0.0, 2.0],
                            "steps": [{"result": "pass", "points": 2.0, "worth": 2.0, "reason": "pushes"}]}],
                 "visibility": {"score": "visible", "feedback": "visible", "stdout": "visible", "error": "visible"}},
                {"node": "b", "name": "resize", "status": "pass", "score": 3.0, "max": 3.0,
                 "visibility": {"score": "visible", "feedback": "after_publish", "stdout": "visible", "error": "visible"}},
                {"node": "c", "name": "stress", "status": "fail", "score": 2.0, "max": 3.0,
                 "error": "assert 2 == 3",
                 "visibility": {"score": "after_publish", "feedback": "visible", "stdout": "visible", "error": "visible"}},
                {"node": "d", "name": "edge", "status": "fail", "score": 1.0, "max": 3.0,
                 "visibility": {"score": "hidden", "feedback": "hidden", "stdout": "hidden", "error": "hidden"}},
                {"node": "e", "name": "late", "status": "pass", "score": 0.0, "max": 0.0,
                 "visibility": {"score": "after_end", "feedback": "visible", "stdout": "visible", "error": "visible"}}
            ]
        }))
        .unwrap()
    }

    fn vis(r: &Results) -> Vec<TestVisibility> {
        r.tests.iter().map(|t| t.visibility).collect()
    }

    #[test]
    fn tallies_add_up_stage_by_stage() {
        let r = results();
        let t = tallies(&r, &vis(&r));
        let at = |s: Stage| t.iter().find(|(stage, _)| *stage == s).unwrap().1;
        assert_eq!(
            (at(Stage::Visible).score, at(Stage::Visible).total),
            (5.0, 5.0)
        );
        assert_eq!(at(Stage::AfterDeadline).tests, 3);
        assert_eq!(
            (at(Stage::AfterRelease).score, at(Stage::AfterRelease).total),
            (7.0, 8.0)
        );
        let all = at(Stage::Hidden);
        assert_eq!(
            (all.score, all.total, all.passed, all.tests),
            (8.0, 11.0, 3, 5)
        );
        assert_eq!(Stage::now(true, false), Stage::AfterDeadline);
        assert_eq!(Stage::now(true, true), Stage::AfterRelease);
    }

    #[test]
    fn students_see_only_what_each_part_allows() {
        let r = results();
        let v = student_view(&r, &vis(&r), false, false);
        assert_eq!(
            v.tests.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["push_pop", "resize"]
        );
        assert_eq!((v.score, v.total), (5.0, 5.0), "only what's shown counts");
        assert_eq!(
            v.tests[0].feedback.as_ref().unwrap()[0].steps[0]
                .reason
                .as_deref(),
            Some("pushes")
        );
        assert_eq!(v.tests[0].stdout.as_deref(), Some("pushed 3\n"));
        assert_eq!(v.tests[1].feedback, None, "its feedback waits for release");
        assert_eq!(v.hidden_until, ["the deadline", "release"]);
        assert!(v.hidden_never);

        let released = student_view(&r, &vis(&r), true, true);
        assert_eq!(released.tests.len(), 4, "everything but the hidden test");
        assert_eq!(released.tests[2].error.as_deref(), Some("assert 2 == 3"));
        assert!(released.hidden_until.is_empty());
    }
}
