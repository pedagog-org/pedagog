//! What a regrade changed for a submission: its score, and the tests that
//! appeared, disappeared, or came out differently in any part (score,
//! feedback, stdout, error). Tests are matched by their stable id when they
//! declare one, else by their framework node.

use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumString};

use crate::results::{Part, Results, Status, TestResult};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Changes {
    /// `(score, total)` before and after; `None` when that run had no results.
    pub before: Option<(f64, f64)>,
    pub after: Option<(f64, f64)>,
    /// Tests only in the regrade.
    pub added: Vec<String>,
    /// Tests only in the earlier run.
    pub removed: Vec<String>,
    /// Tests in both that differ in any part.
    pub changed: Vec<TestChange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestChange {
    pub test: String,
    pub before: Outcome,
    pub after: Outcome,
    /// Which parts differ (score covers status and points).
    #[serde(default)]
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub status: Status,
    pub score: f64,
    pub max: f64,
}

/// When a regrade tells students about their new results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString, Serialize, Deserialize)]
#[strum(serialize_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Notify {
    /// Never (the default: the instructor may be testing).
    Never,
    /// Only if their results changed.
    Changed,
    Always,
}

impl Notify {
    /// Whether a regrade with these changes tells the student.
    pub fn tells(self, changes: &Changes) -> bool {
        match self {
            Notify::Never => false,
            Notify::Changed => !changes.is_empty(),
            Notify::Always => true,
        }
    }
}

impl Changes {
    /// Whether the student would see anything different.
    pub fn is_empty(&self) -> bool {
        self.before == self.after
            && self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
    }
}

/// Compare a submission's earlier run with its regrade (either may have no
/// results, e.g. it errored).
pub fn compare(before: Option<&Results>, after: Option<&Results>) -> Changes {
    let score = |r: &Results| (r.summary.score, r.summary.total);
    let old: Vec<&TestResult> = before.map(|r| r.tests.iter().collect()).unwrap_or_default();
    let new: Vec<&TestResult> = after.map(|r| r.tests.iter().collect()).unwrap_or_default();
    let mut changes = Changes {
        before: before.map(score),
        after: after.map(score),
        ..Default::default()
    };
    for now in &new {
        let k = key(now);
        match find(&old, &k) {
            None => changes.added.push(k),
            Some(was) => {
                let parts = differing(was, now);
                if !parts.is_empty() {
                    changes.changed.push(TestChange {
                        test: k,
                        before: outcome(was),
                        after: outcome(now),
                        parts,
                    });
                }
            }
        }
    }
    changes.removed = old
        .iter()
        .map(|t| key(t))
        .filter(|k| find(&new, k).is_none())
        .collect();
    changes
}

/// The parts of a test that differ between two runs.
fn differing(was: &TestResult, now: &TestResult) -> Vec<Part> {
    let feedback = |t: &TestResult| {
        let steps: Vec<_> = t.items.iter().map(|i| (&i.steps, &i.feedback)).collect();
        serde_json::to_string(&(steps, &t.feedback)).unwrap_or_default()
    };
    let mut parts = Vec::new();
    if outcome(was) != outcome(now) {
        parts.push(Part::Score);
    }
    if feedback(was) != feedback(now) {
        parts.push(Part::Feedback);
    }
    if was.stdout != now.stdout {
        parts.push(Part::Stdout);
    }
    if was.error != now.error {
        parts.push(Part::Error);
    }
    parts
}

fn find<'a>(list: &[&'a TestResult], k: &str) -> Option<&'a TestResult> {
    list.iter().copied().find(|t| key(t) == k)
}

fn key(t: &TestResult) -> String {
    t.id.clone().unwrap_or_else(|| t.node.clone())
}

fn outcome(t: &TestResult) -> Outcome {
    Outcome {
        status: t.status,
        score: t.score,
        max: t.max,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../testdata/pedagog-pytest-results.json");

    fn results() -> Results {
        serde_yaml::from_str(FIXTURE).unwrap()
    }

    #[test]
    fn notify_round_trips_and_decides() {
        use std::str::FromStr;
        for n in [Notify::Never, Notify::Changed, Notify::Always] {
            assert_eq!(Notify::from_str(&n.to_string()).unwrap(), n);
        }
        let none = Changes::default();
        let some = Changes {
            added: vec!["t".into()],
            ..Default::default()
        };
        assert!(!Notify::Never.tells(&some));
        assert!(!Notify::Changed.tells(&none) && Notify::Changed.tells(&some));
        assert!(Notify::Always.tells(&none));
    }

    #[test]
    fn the_same_results_change_nothing() {
        let r = results();
        assert!(compare(Some(&r), Some(&r)).is_empty());
    }

    #[test]
    fn a_new_test_and_a_changed_score_are_reported() {
        let before = results();
        let mut after = results();
        let mut extra = after.tests[0].clone();
        extra.id = Some("new-edge-case".into());
        extra.node = "test_ll.py::test_new_edge_case".into();
        after.tests.push(extra);
        after.tests[1].status = Status::Fail;
        after.tests[1].score = 0.0;
        let c = compare(Some(&before), Some(&after));
        assert_eq!(c.added, ["new-edge-case"]);
        assert!(c.removed.is_empty());
        assert_eq!(c.changed.len(), 1);
        assert_eq!(c.changed[0].test, key(&before.tests[1]));
        assert_eq!(c.changed[0].after.status, Status::Fail);
        assert_eq!(c.changed[0].parts, [Part::Score]);
        assert!(!c.is_empty());
    }

    #[test]
    fn feedback_stdout_and_error_count_as_changes() {
        let before = results();
        let mut after = results();
        after.tests[0].stdout = "a new hint\n".into();
        after.tests[2].error = "a different failure".into();
        after.tests[3].feedback.push_str(" (reworded)");
        let c = compare(Some(&before), Some(&after));
        let parts: Vec<(String, Vec<Part>)> = c
            .changed
            .iter()
            .map(|t| (t.test.clone(), t.parts.clone()))
            .collect();
        assert_eq!(
            parts,
            [
                (key(&before.tests[0]), vec![Part::Stdout]),
                (key(&before.tests[2]), vec![Part::Error]),
                (key(&before.tests[3]), vec![Part::Feedback]),
            ]
        );
        assert!(Notify::Changed.tells(&c), "a changed hint is worth telling");
    }

    #[test]
    fn removed_tests_and_errors_are_reported() {
        let before = results();
        let mut after = results();
        let gone = after.tests.remove(0);
        let c = compare(Some(&before), Some(&after));
        assert_eq!(c.removed, [key(&gone)]);

        let c = compare(Some(&before), None);
        assert_eq!(c.after, None);
        assert_eq!(c.removed.len(), before.tests.len());
        assert!(!c.is_empty());
    }
}
