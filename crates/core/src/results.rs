//! The standard `results.yaml` that test helpers write to `/pedagog/results/`,
//! and the checks the platform runs on it (see ARCHITECTURE.md, Results YAML).

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Results {
    #[serde(default)]
    pub submission_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    pub summary: Summary,
    pub tests: Vec<TestResult>,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub score: f64,
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestResult {
    /// Display number, e.g. `1` or `"2.3"`.
    #[serde(default, deserialize_with = "number_or_string")]
    pub number: Option<String>,
    pub id: String,
    pub name: String,
    pub status: Status,
    pub score: f64,
    pub max: f64,
    #[serde(default)]
    pub output: String,
    pub visibility: TestVisibility,
    /// What the reference solution is expected to do on this test (default: pass).
    #[serde(default)]
    pub solution_expect: Option<Expect>,
    /// Rubric items the test was scored by; empty for all-or-nothing tests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<RubricItem>,
}

/// One rubric item of a test. Extra-credit items add to the test's score but
/// not to its max.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RubricItem {
    pub id: String,
    pub name: String,
    pub score: f64,
    pub max: f64,
    #[serde(default)]
    pub extra_credit: bool,
    #[serde(default)]
    pub output: String,
}

// Scores are sums of floats written by another program.
const EPSILON: f64 = 1e-9;

impl TestResult {
    /// Points the test can earn beyond its max (from extra-credit items).
    pub fn extra_credit(&self) -> f64 {
        self.items
            .iter()
            .filter(|i| i.extra_credit)
            .map(|i| i.max)
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    Error,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Expect {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestVisibility {
    pub score: Visibility,
    pub output: Visibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Visible,
    Hidden,
    AfterEnd,
    AfterPublish,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub label: String,
    pub path: String,
}

/// A test whose outcome with the reference solution differs from what was declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub id: String,
    pub expected: Expect,
    pub actual: Status,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let expected = match self.expected {
            Expect::Pass => "pass",
            Expect::Fail => "fail",
        };
        let actual = match self.actual {
            Status::Pass => "passed",
            Status::Fail => "failed",
            Status::Error => "errored",
            Status::Skip => "was skipped",
        };
        write!(
            f,
            "{}: expected the solution to {expected}, but it {actual}",
            self.id
        )
    }
}

impl Results {
    /// Structural problems that make the file untrustworthy, regardless of outcome.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut ids = HashSet::new();
        for t in &self.tests {
            if !ids.insert(t.id.as_str()) {
                problems.push(format!("duplicate test id {:?}", t.id));
            }
            if t.score < 0.0 || t.max < 0.0 || t.score > t.max + t.extra_credit() + EPSILON {
                problems.push(format!(
                    "test {:?}: score {} outside 0..={} (+{} extra credit)",
                    t.id,
                    t.score,
                    t.max,
                    t.extra_credit()
                ));
            }
            problems.extend(item_problems(t));
        }
        let extra: f64 = self.tests.iter().map(TestResult::extra_credit).sum();
        if self.summary.score > self.summary.total + extra + EPSILON {
            problems.push(format!(
                "summary score {} exceeds total {} (+{extra} extra credit)",
                self.summary.score, self.summary.total
            ));
        }
        problems
    }

    /// Tests whose outcome with the reference solution contradicts their
    /// declared expectation. Empty means the assignment validates. A skipped
    /// test never meets an expectation; erroring counts as failing.
    pub fn solution_mismatches(&self) -> Vec<Mismatch> {
        self.tests
            .iter()
            .filter_map(|t| {
                let expected = t.solution_expect.unwrap_or(Expect::Pass);
                let ok = matches!(
                    (expected, t.status),
                    (Expect::Pass, Status::Pass) | (Expect::Fail, Status::Fail | Status::Error)
                );
                (!ok).then(|| Mismatch {
                    id: t.id.clone(),
                    expected,
                    actual: t.status,
                })
            })
            .collect()
    }
}

fn item_problems(t: &TestResult) -> Vec<String> {
    let mut problems = Vec::new();
    if t.items.is_empty() {
        return problems;
    }
    let mut ids = HashSet::new();
    for i in &t.items {
        if !ids.insert(i.id.as_str()) {
            problems.push(format!("test {:?}: duplicate item id {:?}", t.id, i.id));
        }
        if i.score < 0.0 || i.max < 0.0 || i.score > i.max + EPSILON {
            problems.push(format!(
                "test {:?}, item {:?}: score {} outside 0..={}",
                t.id, i.id, i.score, i.max
            ));
        }
    }
    let score: f64 = t.items.iter().map(|i| i.score).sum();
    let max: f64 = t
        .items
        .iter()
        .filter(|i| !i.extra_credit)
        .map(|i| i.max)
        .sum();
    if (score - t.score).abs() > EPSILON || (max - t.max).abs() > EPSILON {
        problems.push(format!(
            "test {:?}: score {}/{} doesn't match its items' {score}/{max}",
            t.id, t.score, t.max
        ));
    }
    problems
}

fn number_or_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Int(i64),
        Float(f64),
        Str(String),
    }
    Ok(Option::<NumOrStr>::deserialize(d)?.map(|v| match v {
        NumOrStr::Int(i) => i.to_string(),
        NumOrStr::Float(x) => x.to_string(),
        NumOrStr::Str(s) => s,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "\
submission-id: ~
run-id: ~
summary:
  score: 10
  total: 30
tests:
  - number: 1
    id: test_null_ptr
    name: Null pointer check
    status: pass
    score: 10
    max: 10
    output: All assertions passed
    visibility:
      score: visible
      output: after_end
  - number: \"2.1\"
    id: test_leak
    name: Memory leak check
    status: fail
    score: 0
    max: 20
    visibility:
      score: visible
      output: hidden
    solution-expect: fail
artifacts:
  - id: valgrind_log
    label: Valgrind output
    path: valgrind.log
";

    fn example() -> Results {
        serde_yaml::from_str(EXAMPLE).unwrap()
    }

    #[test]
    fn parses_the_documented_format() {
        let r = example();
        assert_eq!(r.tests.len(), 2);
        assert_eq!(r.tests[0].number.as_deref(), Some("1"));
        assert_eq!(r.tests[1].number.as_deref(), Some("2.1"));
        assert_eq!(r.tests[0].visibility.output, Visibility::AfterEnd);
        assert_eq!(r.tests[1].solution_expect, Some(Expect::Fail));
        assert_eq!(r.artifacts[0].path, "valgrind.log");
        assert!(r.problems().is_empty());
    }

    #[test]
    fn declared_failures_validate() {
        assert!(example().solution_mismatches().is_empty());
    }

    #[test]
    fn unexpected_outcomes_are_reported() {
        let mut r = example();
        r.tests[0].status = Status::Error;
        r.tests[1].status = Status::Pass;
        let m = r.solution_mismatches();
        assert_eq!(m.len(), 2);
        assert_eq!(
            m[0].to_string(),
            "test_null_ptr: expected the solution to pass, but it errored"
        );
        assert_eq!(
            m[1].to_string(),
            "test_leak: expected the solution to fail, but it passed"
        );
    }

    #[test]
    fn skipped_tests_never_validate() {
        let mut r = example();
        r.tests[1].status = Status::Skip;
        assert_eq!(r.solution_mismatches().len(), 1);
    }

    #[test]
    fn structural_problems_are_reported() {
        let mut r = example();
        r.tests[1].id = "test_null_ptr".into();
        r.tests[0].score = 11.0;
        r.summary.score = 40.0;
        let p = r.problems();
        assert_eq!(p.len(), 3, "{p:?}");
    }

    const RUBRIC_TEST: &str = "\
id: test_design
name: Design
status: fail
score: 3
max: 2
visibility:
  score: visible
  output: after_end
items:
  - id: memory_safe
    name: No leaks
    score: 2
    max: 2
  - id: recursion
    name: Uses recursion
    score: 1
    max: 1
    extra-credit: true
    output: assert False
";

    fn with_rubric_test() -> Results {
        let mut r = example();
        r.tests.push(serde_yaml::from_str(RUBRIC_TEST).unwrap());
        r.summary.score = 13.0;
        r.summary.total = 32.0;
        r
    }

    #[test]
    fn extra_credit_may_exceed_the_max() {
        let mut r = with_rubric_test();
        assert!(r.problems().is_empty(), "{:?}", r.problems());
        assert_eq!(r.tests[2].extra_credit(), 1.0);
        r.summary.score = 33.0;
        assert!(r.problems().is_empty(), "{:?}", r.problems());
        r.summary.score = 33.5;
        assert_eq!(r.problems().len(), 1);
    }

    #[test]
    fn rubric_items_must_add_up() {
        let mut r = with_rubric_test();
        r.tests[2].items[0].score = 1.0;
        assert_eq!(r.problems().len(), 1, "score mismatch");
        let mut r = with_rubric_test();
        r.tests[2].items[1].extra_credit = false;
        assert_eq!(r.problems().len(), 2, "max mismatch, and over the max");
    }

    #[test]
    fn rubric_item_problems_are_reported() {
        let mut r = with_rubric_test();
        r.tests[2].items[1].id = "memory_safe".into();
        r.tests[2].items[1].score = 2.0;
        r.tests[2].score = 4.0;
        let p = r.problems();
        assert_eq!(p.len(), 3, "{p:?}");
    }

    #[test]
    fn round_trips_through_json() {
        let r = with_rubric_test();
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Results>(&json).unwrap(), r);
    }

    /// Real output of the pedagog-pytest helper (examples/linked-list-python
    /// run against its solution), so the Python and Rust sides of the
    /// results format can't drift apart unnoticed.
    #[test]
    fn parses_real_pedagog_pytest_output() {
        let r: Results =
            serde_yaml::from_str(include_str!("../testdata/pedagog-pytest-results.json")).unwrap();
        assert!(r.problems().is_empty(), "{:?}", r.problems());
        assert_eq!(
            r.summary,
            Summary {
                score: 10.0,
                total: 11.0
            }
        );
        assert_eq!(r.tests.len(), 5);
        let pop_front = &r.tests[2];
        assert_eq!(pop_front.visibility.output, Visibility::Hidden);
        assert_eq!((pop_front.score, pop_front.max), (4.0, 4.0));
        assert_eq!(pop_front.items.len(), 3);
        assert_eq!(r.tests[3].solution_expect, Some(Expect::Fail));
        assert!(
            r.tests[4].items.is_empty(),
            "a single rubric item is the test itself"
        );
        assert!(r.solution_mismatches().is_empty(), "the example validates");
    }
}
