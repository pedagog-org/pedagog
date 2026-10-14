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
            if t.score < 0.0 || t.max < 0.0 || t.score > t.max {
                problems.push(format!(
                    "test {:?}: score {} outside 0..={}",
                    t.id, t.score, t.max
                ));
            }
        }
        if self.summary.score > self.summary.total {
            problems.push(format!(
                "summary score {} exceeds total {}",
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

    #[test]
    fn round_trips_through_json() {
        let r = example();
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Results>(&json).unwrap(), r);
    }
}
