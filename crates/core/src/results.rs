//! The standard `results.yaml` that test helpers write to `/pedagog/results/`,
//! and the checks the platform runs on it (see ARCHITECTURE.md, Results YAML).

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

/// How helpers mark a test file that failed to load: `<file>::<collection>`.
pub const COLLECTION_SUFFIX: &str = "::<collection>";

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

/// Inclusive score bounds; `None` is unbounded on that side.
pub type Clamp = (Option<f64>, Option<f64>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestResult {
    /// Display number, e.g. `1` or `"2.3"`.
    #[serde(default, deserialize_with = "number_or_string")]
    pub number: Option<String>,
    /// Stable id, only when the test declares one.
    #[serde(default)]
    pub id: Option<String>,
    /// The framework's own identifier for the test (e.g. pytest's node id).
    pub node: String,
    pub name: String,
    pub status: Status,
    pub score: f64,
    pub max: f64,
    /// The framework's failure text.
    #[serde(default)]
    pub output: String,
    /// What the student is told about the score (subject to output visibility).
    #[serde(default)]
    pub feedback: String,
    /// How the score was calculated; for instructors only.
    #[serde(default)]
    pub trace: String,
    pub visibility: TestVisibility,
    /// What the reference solution is expected to do on this test (default: pass).
    #[serde(default)]
    pub solution_expect: Option<Expect>,
    /// Bounds on the score of a test that is a single rubric item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clamp: Option<Clamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_credit: Option<ExtraCredit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempted: Option<bool>,
    /// Rubric items the test was scored by, for multi-item rubrics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<RubricItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RubricItem {
    pub id: String,
    pub name: String,
    pub score: f64,
    /// The item's declared points.
    pub max: f64,
    pub clamp: Clamp,
    #[serde(default)]
    pub extra_credit: ExtraCredit,
    #[serde(default)]
    pub attempted: bool,
    #[serde(default)]
    pub feedback: String,
}

/// How a rubric item counts toward its test's max.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtraCredit {
    /// Counts in the score and the max.
    #[default]
    None,
    /// Counts in the score, never in the max.
    Bonus,
    /// Counts in the max only once attempted.
    AttemptedWork,
}

impl RubricItem {
    /// What the item adds to its test's max.
    pub fn counted_max(&self) -> f64 {
        match self.extra_credit {
            ExtraCredit::None => self.max,
            ExtraCredit::AttemptedWork if self.attempted => self.max,
            _ => 0.0,
        }
    }
}

impl TestResult {
    /// The declared id, or else the node: what to call the test in messages.
    pub fn key(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.node)
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

// Scores are sums of floats written by another program.
const EPSILON: f64 = 1e-9;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= EPSILON
}

fn within(score: f64, (low, high): Clamp) -> bool {
    low.is_none_or(|l| score >= l - EPSILON) && high.is_none_or(|h| score <= h + EPSILON)
}

fn show(clamp: Clamp) -> String {
    let side = |b: Option<f64>, inf: &str| b.map_or(inf.to_string(), |v| v.to_string());
    format!("[{}, {}]", side(clamp.0, "-inf"), side(clamp.1, "inf"))
}

impl Results {
    /// Structural problems that make the file untrustworthy, regardless of outcome.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut nodes = HashSet::new();
        let mut ids = HashSet::new();
        for t in &self.tests {
            if !nodes.insert(t.node.as_str()) {
                problems.push(format!("duplicate test node {:?}", t.node));
            }
            if let Some(id) = &t.id
                && !ids.insert(id.as_str())
            {
                problems.push(format!("duplicate test id {id:?}"));
            }
            problems.extend(test_problems(t));
        }
        let score: f64 = self.tests.iter().map(|t| t.score).sum();
        let total: f64 = self.tests.iter().map(|t| t.max).sum();
        if !close(self.summary.score, score) || !close(self.summary.total, total) {
            problems.push(format!(
                "summary {}/{} doesn't match its tests' {score}/{total}",
                self.summary.score, self.summary.total
            ));
        }
        problems
    }

    /// Entries the helper reports for test files that couldn't be loaded at
    /// all (e.g. the submission doesn't import), so their tests never ran.
    pub fn collection_errors(&self) -> Vec<&TestResult> {
        self.tests
            .iter()
            .filter(|t| t.node.ends_with(COLLECTION_SUFFIX))
            .collect()
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
                    id: t.key().to_string(),
                    expected,
                    actual: t.status,
                })
            })
            .collect()
    }
}

fn test_problems(t: &TestResult) -> Vec<String> {
    let mut problems = Vec::new();
    let key = t.key();
    if t.max < 0.0 {
        problems.push(format!("test {key:?}: negative max {}", t.max));
    }
    if !t.items.is_empty() {
        let mut item_ids = HashSet::new();
        for i in &t.items {
            if !item_ids.insert(i.id.as_str()) {
                problems.push(format!("test {key:?}: duplicate item id {:?}", i.id));
            }
            if i.max < 0.0 || !within(i.score, i.clamp) {
                problems.push(format!(
                    "test {key:?}, item {:?}: score {} outside {} (max {})",
                    i.id,
                    i.score,
                    show(i.clamp),
                    i.max
                ));
            }
        }
        let score: f64 = t.items.iter().map(|i| i.score).sum();
        let max: f64 = t.items.iter().map(RubricItem::counted_max).sum();
        if !close(score, t.score) || !close(max, t.max) {
            problems.push(format!(
                "test {key:?}: score {}/{} doesn't match its items' {score}/{max}",
                t.score, t.max
            ));
        }
    } else if let Some(clamp) = t.clamp {
        if !within(t.score, clamp) {
            problems.push(format!(
                "test {key:?}: score {} outside {}",
                t.score,
                show(clamp)
            ));
        }
    } else if t.score < 0.0 || t.score > t.max + EPSILON {
        // No declared bounds: the classic 0..=max.
        problems.push(format!(
            "test {key:?}: score {} outside 0..={}",
            t.score, t.max
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
    id: null-ptr
    node: test_null_ptr
    name: Null pointer check
    status: pass
    score: 10
    max: 10
    output: All assertions passed
    visibility:
      score: visible
      output: after_end
  - number: \"2.1\"
    node: test_leak
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
        assert_eq!(r.tests[0].key(), "null-ptr");
        assert_eq!(r.tests[1].key(), "test_leak", "no id: the node");
        assert_eq!(r.tests[0].visibility.output, Visibility::AfterEnd);
        assert_eq!(r.tests[1].solution_expect, Some(Expect::Fail));
        assert_eq!(r.artifacts[0].path, "valgrind.log");
        assert!(r.problems().is_empty(), "{:?}", r.problems());
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
            "null-ptr: expected the solution to pass, but it errored"
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
        r.tests[1].node = "test_null_ptr".into();
        r.tests[1].id = Some("null-ptr".into());
        r.tests[0].score = 11.0;
        let p = r.problems();
        // duplicate node, duplicate id, score over max, summary mismatch
        assert_eq!(p.len(), 4, "{p:?}");
    }

    const RUBRIC_TEST: &str = "\
id: design
node: test_design.py::test_design
name: Design
status: fail
score: 2
max: 4
visibility:
  score: visible
  output: after_end
items:
  - id: memory_safe
    name: No leaks
    score: 2
    max: 2
    clamp: [0, 2]
    attempted: true
    feedback: \"✓ +2 no leaks\"
  - id: recursion
    name: Uses recursion
    score: 1
    max: 1
    clamp: [0, 1]
    extra-credit: bonus
    attempted: true
  - id: reverse
    name: reverse()
    score: 0
    max: 2
    clamp: [0, 2]
    extra-credit: attempted-work
    attempted: true
  - id: stretch
    name: stretch
    score: 0
    max: 5
    clamp: [0, 5]
    extra-credit: attempted-work
  - id: late
    name: late penalty
    score: -1
    max: 0
    clamp: [null, 0]
";

    const SINGLE_ITEM_TEST: &str = "\
node: test_len.py::test_len
name: len
status: pass
score: 3.5
max: 2
clamp: [0, null]
extra-credit: none
attempted: true
visibility:
  score: visible
  output: after_end
";

    fn with_rubric_tests() -> Results {
        let mut r = example();
        r.tests.push(serde_yaml::from_str(RUBRIC_TEST).unwrap());
        r.tests
            .push(serde_yaml::from_str(SINGLE_ITEM_TEST).unwrap());
        r.summary.score = 15.5;
        r.summary.total = 36.0;
        r
    }

    #[test]
    fn rubric_scores_follow_their_items_and_bounds() {
        let r = with_rubric_tests();
        assert!(r.problems().is_empty(), "{:?}", r.problems());
        let items = &r.tests[2].items;
        let counted: Vec<f64> = items.iter().map(RubricItem::counted_max).collect();
        assert_eq!(counted, [2.0, 0.0, 2.0, 0.0, 0.0]);
        assert_eq!(r.tests[3].clamp, Some((Some(0.0), None)));
    }

    #[test]
    fn rubric_items_must_add_up() {
        let mut r = with_rubric_tests();
        r.tests[2].items[0].score = 1.0;
        assert_eq!(r.problems().len(), 1, "score mismatch");
        let mut r = with_rubric_tests();
        r.tests[2].items[3].attempted = true;
        assert_eq!(r.problems().len(), 1, "an attempted item counts in the max");
    }

    #[test]
    fn scores_must_respect_their_clamp() {
        let mut r = with_rubric_tests();
        r.tests[2].items[4].score = 1.0;
        r.tests[2].score = 4.0;
        r.summary.score = 17.5;
        assert_eq!(r.problems().len(), 1, "{:?}", r.problems());
        let mut r = with_rubric_tests();
        r.tests[3].score = -1.0;
        r.summary.score = 11.0;
        assert_eq!(r.problems().len(), 1, "{:?}", r.problems());
    }

    #[test]
    fn duplicate_item_ids_are_reported() {
        let mut r = with_rubric_tests();
        r.tests[2].items[1].id = "memory_safe".into();
        assert_eq!(r.problems().len(), 1, "{:?}", r.problems());
    }

    #[test]
    fn summary_must_match_the_tests() {
        let mut r = with_rubric_tests();
        r.summary.total = 30.0;
        assert_eq!(r.problems().len(), 1, "{:?}", r.problems());
    }

    #[test]
    fn collection_errors_are_found_by_their_node() {
        let mut r = with_rubric_tests();
        assert!(r.collection_errors().is_empty());
        r.tests[0].node = "test_ll.py::<collection>".into();
        assert_eq!(r.collection_errors().len(), 1);
    }

    #[test]
    fn round_trips_through_json() {
        let r = with_rubric_tests();
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
                total: 10.0
            }
        );
        let keys: Vec<&str> = r.tests.iter().map(TestResult::key).collect();
        assert_eq!(keys, ["append", "len", "pop-front", "reverse", "many"]);
        let pop_front = &r.tests[2];
        assert_eq!(pop_front.visibility.output, Visibility::Hidden);
        assert_eq!((pop_front.score, pop_front.max), (4.0, 4.0));
        assert_eq!(pop_front.items.len(), 3);
        assert!(pop_front.trace.contains("test: 2 + 1 + 1 = 4"));
        let reverse = &r.tests[3];
        assert_eq!(reverse.solution_expect, Some(Expect::Fail));
        assert_eq!(reverse.extra_credit, Some(ExtraCredit::AttemptedWork));
        assert_eq!(reverse.attempted, Some(false));
        assert_eq!(reverse.max, 0.0, "not attempted: not in the total");
        assert!(r.tests[1].feedback.contains("+1 empty list"));
        assert!(r.solution_mismatches().is_empty(), "the example validates");
    }
}
