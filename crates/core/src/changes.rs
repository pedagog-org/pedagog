//! What a regrade changed for a submission: its score, and the tests that
//! appeared, disappeared, or came out differently. Tests are matched by their
//! stable id when they declare one, else by their framework node.

use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumString};

use crate::results::{Results, Status, TestResult};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Changes {
    /// `(score, total)` before and after; `None` when that run had no results.
    pub before: Option<(f64, f64)>,
    pub after: Option<(f64, f64)>,
    /// Tests only in the regrade.
    pub added: Vec<String>,
    /// Tests only in the earlier run.
    pub removed: Vec<String>,
    /// Tests in both whose status or score differ.
    pub changed: Vec<TestChange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestChange {
    pub test: String,
    pub before: Outcome,
    pub after: Outcome,
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
    let tests = |r: Option<&Results>| -> Vec<(String, Outcome)> {
        r.map(|r| r.tests.iter().map(|t| (key(t), outcome(t))).collect())
            .unwrap_or_default()
    };
    let old = tests(before);
    let new = tests(after);
    let find = |list: &[(String, Outcome)], k: &str| {
        list.iter().find(|(key, _)| key == k).map(|(_, o)| *o)
    };
    let mut changes = Changes {
        before: before.map(score),
        after: after.map(score),
        ..Default::default()
    };
    for (k, now) in &new {
        match find(&old, k) {
            None => changes.added.push(k.clone()),
            Some(was) if was != *now => changes.changed.push(TestChange {
                test: k.clone(),
                before: was,
                after: *now,
            }),
            Some(_) => {}
        }
    }
    changes.removed = old
        .iter()
        .filter(|(k, _)| find(&new, k).is_none())
        .map(|(k, _)| k.clone())
        .collect();
    changes
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
        assert!(!c.is_empty());
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
