//! What each audience sees of a run's results, given each test's resolved
//! visibility: the cumulative score columns staff see (and which one a
//! student sees now), and the student's own filtered view.

use serde::{Deserialize, Serialize};

use crate::results::{Results, RubricItem, Status, Step, TestResult, TestVisibility, Visibility};

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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShownTest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    pub status: Status,
    pub score: f64,
    pub max: f64,
    /// A test that's only extra credit: the points it could add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_credit: Option<f64>,
    /// Each item's steps, when feedback is shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<Vec<ShownItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `error` is the helper's sanitized report (a summary, a blank line, then
    /// the call chain), not the framework's raw one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub error_sanitized: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShownItem {
    pub name: String,
    pub score: f64,
    pub max: f64,
    /// Adds to the score without being owed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub extra_credit: bool,
    pub steps: Vec<Step>,
    /// Version 1 helpers' text feedback.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Its parts, as far as the student may see them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ShownItem>,
}

/// What a student sees of a run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

/// What one level of items comes to for a student: what they may see, and
/// the points of parts whose score they may not (left out of their totals),
/// with when those will show.
#[derive(Default)]
struct Shown {
    items: Vec<ShownItem>,
    hidden_score: f64,
    hidden_max: f64,
    hidden: Vec<Visibility>,
}

/// `items` as a student sees them: a part's score shows only where its own
/// visibility (and every level above) allows; its steps and parts only where
/// its feedback does too. `shown` decides one visibility now.
fn shown_items(items: &[RubricItem], feedback: bool, shown: &dyn Fn(Visibility) -> bool) -> Shown {
    let mut out = Shown::default();
    for i in items {
        if let Some(v) = i.visibility.and_then(|v| v.score)
            && !shown(v)
        {
            out.hidden_score += i.score;
            out.hidden_max += i.counted_max();
            out.hidden.push(v);
            continue;
        }
        let own_feedback = feedback && i.visibility.and_then(|v| v.feedback).is_none_or(shown);
        let below = shown_items(&i.parts, own_feedback, shown);
        out.hidden.extend(below.hidden.iter().copied());
        out.items.push(ShownItem {
            name: i.name.clone(),
            score: i.score - below.hidden_score,
            max: i.max - below.hidden_max,
            extra_credit: i.extra_credit != crate::results::ExtraCredit::None,
            steps: if own_feedback {
                i.steps.clone()
            } else {
                Vec::new()
            },
            text: if own_feedback {
                i.feedback.clone()
            } else {
                String::new()
            },
            parts: if own_feedback {
                below.items
            } else {
                Vec::new()
            },
        });
        out.hidden_score += below.hidden_score;
        out.hidden_max += below.hidden_max;
    }
    out
}

fn feedback_of(t: &TestResult, shown: &dyn Fn(Visibility) -> bool) -> Shown {
    let mut out = shown_items(&t.items, true, shown);
    if t.items.is_empty() && !t.feedback.is_empty() {
        // A version 1 single-item test: its feedback text at the test level.
        out.items.push(ShownItem {
            name: t.name.clone(),
            score: t.score,
            max: t.max,
            extra_credit: t.extra_credit_worth().is_some(),
            steps: Vec::new(),
            text: t.feedback.clone(),
            parts: Vec::new(),
        });
    }
    out
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
        let parts = feedback_of(t, &shown);
        for hidden in &parts.hidden {
            match hidden {
                Visibility::AfterEnd => until_deadline = true,
                Visibility::AfterPublish => until_release = true,
                _ => view.hidden_never = true,
            }
        }
        let (score, max) = (t.score - parts.hidden_score, t.max - parts.hidden_max);
        view.score += score;
        view.total += max;
        view.tests.push(ShownTest {
            name: t.name.clone(),
            number: t.number.clone(),
            status: t.status,
            score,
            max,
            extra_credit: t.extra_credit_worth(),
            feedback: shown(v.feedback).then_some(parts.items),
            stdout: (shown(v.stdout) && !t.stdout.is_empty()).then(|| t.stdout.clone()),
            error: (shown(v.error) && !t.error_for_students().is_empty())
                .then(|| t.error_for_students().to_owned()),
            error_sanitized: !t.error_sanitized.is_empty(),
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

/// A test with every part shown, its error as the framework reported it
/// (staff, and the reference solution's results).
pub fn full_test(t: &TestResult) -> ShownTest {
    ShownTest {
        name: t.name.clone(),
        number: t.number.clone(),
        status: t.status,
        score: t.score,
        max: t.max,
        extra_credit: t.extra_credit_worth(),
        feedback: Some(feedback_of(t, &|_| true).items),
        stdout: (!t.stdout.is_empty()).then(|| t.stdout.clone()),
        error: (!t.error.is_empty()).then(|| t.error.clone()),
        error_sanitized: false,
    }
}

/// A validation's tests: each with what the reference solution was declared
/// to do and whether it did.
pub fn reference_tests(results: &Results) -> Vec<crate::api::ReferenceTest> {
    let mismatched: std::collections::HashSet<String> = results
        .solution_mismatches()
        .into_iter()
        .map(|m| m.id)
        .collect();
    results
        .tests
        .iter()
        .map(|t| crate::api::ReferenceTest {
            test: full_test(t),
            expected: t.solution_expect.unwrap_or(crate::results::Expect::Pass),
            as_expected: !mismatched.contains(t.key()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format_3() -> Results {
        serde_yaml::from_str(include_str!("../testdata/pedagog-pytest-3-results.json")).unwrap()
    }

    #[test]
    fn hidden_parts_leave_the_totals_and_hidden_feedback_its_details() {
        let r = format_3();
        // Before the deadline, before release: each Bases' row space (score
        // after_publish) is hidden; column space shows its score, not its details.
        let early = student_view(&r, &r.visibilities(), false, false);
        let a2 = &early.tests[0].feedback.as_ref().unwrap()[0];
        assert_eq!(
            a2.parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Column space", "Bonus: rank theorem"],
            "row space hidden until release"
        );
        let col = &a2.parts[0];
        assert!(
            col.steps.is_empty() && col.parts.is_empty(),
            "details after the deadline"
        );
        assert!(
            (col.score - 4.0 / 3.0).abs() < 1e-9,
            "its score still shows"
        );
        // Totals cover only what's shown: 8.5 minus both row spaces (2/3 and 2).
        assert!(
            (early.total - (8.5 - 2.0 / 3.0 - 2.0)).abs() < 1e-9,
            "{}",
            early.total
        );
        assert_eq!(early.hidden_until, ["release"]);

        let after = student_view(&r, &r.visibilities(), true, true);
        assert!((after.total - 8.5).abs() < 1e-9);
        let a4 = &after.tests[0].feedback.as_ref().unwrap()[1];
        let each = &a4.parts[0].parts[1].parts[1];
        assert_eq!(each.steps.len(), 3, "the details once the deadline passed");
        assert!(after.hidden_until.is_empty());
    }

    #[test]
    fn reference_tests_say_which_went_as_declared() {
        let r: Results = serde_json::from_value(serde_json::json!({
            "summary": {"score": 2.0, "total": 4.0},
            "tests": [
                {"node": "a", "name": "append", "status": "pass", "score": 2.0, "max": 2.0, "visibility": {"score": "visible", "feedback": "visible", "stdout": "visible", "error": "visible"}},
                {"node": "b", "name": "pop", "status": "fail", "score": 0.0, "max": 2.0,
                 "error": "Traceback ...\nValueError: empty", "visibility": {"score": "visible", "feedback": "visible", "stdout": "visible", "error": "visible"}},
                {"node": "c", "name": "reverse", "status": "fail", "score": 0.0, "max": 0.0,
                 "solution-expect": "fail", "visibility": {"score": "visible", "feedback": "visible", "stdout": "visible", "error": "visible"}}
            ]
        }))
        .unwrap();
        let tests = reference_tests(&r);
        let got: Vec<(&str, bool)> = tests
            .iter()
            .map(|t| (t.test.name.as_str(), t.as_expected))
            .collect();
        assert_eq!(got, [("append", true), ("pop", false), ("reverse", true)]);
        assert_eq!(tests[2].expected, crate::results::Expect::Fail);
        assert_eq!(
            tests[1].test.error.as_deref(),
            Some("Traceback ...\nValueError: empty"),
            "the raw error, for staff"
        );
    }

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
