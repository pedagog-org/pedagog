# Nested feedback, and visibility only from the tests

Decided with the user on 2026-10-22 and 2026-10-23, from the Gilbert Assignment 3 demo
(an Octave assignment ported to pedagog-pytest). Lands after PR 4 (#71), before PR 5.

## Rationale

- **Feedback is two levels deep:** a test's rubric items, each with its checks and
  awards. Real assignments are deeper: Assignment 3's Exercise 3 Part B goes A4 →
  column space → C4 spans col(A4) → each column → "is a pivot column". Today that's
  flattened into long step lists, or split into many tests that lose the structure.
- **Visibility has three layers:** the tests' `@visibility`, assignment.yml's
  `assignment: results:`, and course overrides edited online. The user wants one place:
  "no editing visibility online. just upload a new test." The file layer goes too.

## Decisions

- **Nesting lives inside a test.** Items hold parts, to any depth up to a cap, with checks
  at any level. Tests stay a flat, numbered list.
- **Points:**
  - a part may have its own points, for checks placed directly on it;
  - its score is its own checks plus its children, clamped;
  - any part may clamp; a part with no own points just sums its children.
- **Declared up front:** nested `Rubric` classes, through `part(...)`, so every maximum is
  known even when a test crashes at its first line. Parts aren't built as the test runs.
- **Reuse at another weight (`worth=`):** a sub-rubric declared at one total (say `Bases`,
  10 points) can be placed where it's worth less or more. `part("A2", worth=2.17)` rescales
  everything inside proportionally: items, checks, awards and clamps. Tests keep scoring in
  the sub-rubric's own terms, so one helper function serves every weighting.
- **Display:**
  - parts collapse; branches with a failure open on load, down to the failing check;
  - levels 1–4 indent with a guide line; level 5 and deeper stop indenting and show
    their path ("A4 › Column space › … › Column 2").
- **Depth cap:** results nested more than 8 levels below a test are invalid, like malformed
  ones. That also bounds recursion on files a run uploads, which are untrusted.
- **Size cap:** results.yaml over 4 MiB is invalid, checked before it's read.
- **Visibility per part, tighten only:** a part's `score` and `feedback` are shown only
  when the test's are shown *and* the part's own are. A part whose score is hidden is left
  out of the student's totals, as hidden tests are ("on the tests shown" stays true at
  every level).
- **Extra credit:** any part can be extra credit (bonus, or counted once attempted),
  with everything under it.
- **Visibility only from the tests:**
  - removed: course overrides (api endpoints, hammer commands, the stored column) and
    assignment.yml's `results:` layer;
  - the PR 8 Visibility page is dropped;
  - Release stays.
- **Format version:** results gain `format: 3`. The platform refuses formats newer than it
  knows, and still reads unversioned v1/v2 files.
- **Regrade notices stay per test** ("Part B: 9.3 → 10, feedback changed"). Parts are
  compared to decide "feedback changed", but notices don't list them.

## Design

### Results format 3

```yaml
format: 3
tests:
  - node: tests/test_ex3.py::test_part_b
    id: 3b
    name: "Part B: bases for A2, A3 and A4"
    score: 6.86
    max: 7
    visibility: {score: visible, feedback: visible, stdout: visible, error: after_end}
    items:
      - id: a4
        name: A4 (given)
        points: 0.5          # its own, for checks directly on it
        score: 2.53          # own checks + parts, clamped
        max: 2.67            # own points + parts' counted max
        clamp: [0, 2.67]
        extra-credit: none
        attempted: true
        steps:
          - {result: pass, points: 0.5, worth: 0.5, reason: A4 is the given matrix}
        parts:
          - id: column-space
            name: Column space
            visibility: {feedback: after_end}   # optional; score / feedback only
            ...                                  # the same fields, recursively
```

- A part is the same shape as a rubric item, plus `points` (its own), `parts`, and an
  optional `visibility`. A rescaled part also records `scale` (the factor); its points are
  written already scaled, so consumers never apply it. A v2 item is a part with no `parts`. A missing `points` means its
  `max` is its own.
- **Ids:** unique among siblings. A part is addressed by its path of ids (`3b/a4/column-space`),
  for the trace and for later use.
- **Validation** (`Results::problems`, recursive with a depth bound):
  - **Structure:** depth ≤ 8; part ids unique among siblings; step ids unique within
    their part.
  - **Points:**
    - `max` = `points` + the parts' counted max;
    - the score lies within the clamp;
    - a test's score and max are the sums of its items'.
  - **Format:** `format` unknown or greater than 3 makes the results invalid ("written by a
    newer pedagog-pytest; update pedagog").
- **Size:** jobs checks the object's size (a HEAD) before reading it. Over 4 MiB is invalid
  for validation, and "couldn't be graded" for grading, with the reason given to staff.

### pedagog-pytest 3 (its repo; major version)

```python
class Square(Rubric):
    partic: Score = item("sol_3_partic", points=0.7)
    cramer: Score = item("sol_3_cramer", points=0.7)

class PartB(Rubric):
    tall: Square = part("(i) m > n", points=1.3)
    square: Square = part("(iii) m = n", points=1.3, feedback=AFTER_END)
    given: Square = part("A4 (given)", points=0.5, worth=1.2)   # Square (1.4) scaled to 0.7

@rubric(PartB, id="2b", name="Part B: random systems")
def test_part_b(scores: PartB):
    with scores.square.check(points=0.65, reason="A3 is random"):   # the part's own points
        ...
    with scores.square.cramer.check(reason="sol_3_cramer"):           # a child item
        ...
```

- `part(name, *, points=0, worth=None, id=None, clamp=…, extra_credit=NONE, score=…, feedback=…)`
  declares a nested `Rubric` attribute. Its type annotation names the sub-rubric (or
  `rubric=` does).
- **`worth=`** is the part's total, including its own `points`:
  - the sub-rubric's declared total is scaled to `worth - points`, a factor applied when
    points are recorded (checks, awards, `set`, clamps);
  - tests write `check(points=2 / n)` in the sub-rubric's terms;
  - the factor nests: a scaled part inside a scaled part multiplies;
  - `worth` must be ≥ `points`, and the sub-rubric's total must be > 0 to scale;
  - students see scaled points, shown to two decimals, with totals summed before
    rounding; the trace shows the factor ("parts scaled ×0.2167 (Bases 10 → 2.17)").
- A `Rubric` instance scores like a `Score` for its own points: `check`, `award`, `set`,
  `nil`, `attempt`. Leaves stay `Score`s. `@rubric_item` is unchanged.
- Clamps apply once per part, to own + children, so awards and deductions still commute
  at every level.
- `max` is computed from the declarations, so a crash keeps the full maximum (as today).
- **What it writes:**
  - `format: 3`, the nested items, and per-part `visibility`;
  - the trace nests by indentation, with each part's own line and its clamp.
- **Breaking:** visibility no longer mentions course overrides. The README and the "courses
  can override it" docstrings change.
- `recipes`: `toolchains/ubuntu-22/pedagog-pytest/3.yaml` (a pinned v3.0.0 bundle), and
  the python addon lists `["1", "2", "3"]`.

### Core

- **`results.rs`:** `RubricItem` gains `points: Option<f64>`, `parts: Vec<RubricItem>` and
  `visibility: Option<PartVisibility>` (`score`, `feedback`). `counted_max` and
  `extra_credit_worth` recurse; `Results.format: Option<u32>`.
- **`view.rs`:**
  - `ShownItem` gains `parts: Vec<ShownItem>`;
  - `feedback_of` recurses;
  - a part whose score isn't shown is left out (and out of the test's shown score and max);
  - a part whose feedback isn't shown keeps its score, but its steps and parts are hidden;
  - `hidden_until` covers parts too;
  - `full_test` and `reference_tests` show every part.
- **`changes.rs`:** "feedback changed" compares the whole items tree. Per-test parts stay
  as they are.
- **`recipe/policy.rs`:** `ResultsPolicy` and the course/file layers go. A test's
  visibility is its own (`TestVisibility`), and a part's is the conjunction with its own.
  `explain` reduces to "set by the test" or "the default".

### Api, jobs, hammer, web

- **jobs:** the size check before reading results, and the format check in `judge` (both
  grading and validation).
- **api:**
  - the staff result shows the nested items (raw); `my-results` and the notices use the
    new view;
  - the `/visibility` endpoints are removed (404);
  - `policy_of` no longer reads overrides.
- **hammer:**
  - `results show` / `results mine` render parts indented, with paths past 4 levels;
  - the `visibility` commands are removed, with a message pointing to the tests;
  - the staff "student sees" table stays (it reads the tests' visibility).
- **web:**
  - a recursive part component in `components/results.rs`, as in the mock: failing
    branches open, indent to level 4, then a path;
  - used by the student results and the build page's reference tests.

### Removing the visibility layers

- **Store, migration 0018:**
  - drops `assignments.visibility_overrides`;
  - the down migration re-adds it empty;
  - stored overrides are lost (kind data only so far).
- **assignment.yml:** an `assignment: results:` block is refused at upload, and by
  hammer's local check: "visibility lives in the tests (@visibility); remove
  assignment.results".
- **e2e `visibility_and_release`:**
  - today it hides the example's `many` test with a course override;
  - instead, the upload example's test gets `@visibility(score=AFTER_PUBLISH)`
    (a recipes PR);
  - the check keeps "hidden before release, shown after".
- **Docs:**
  - ARCHITECTURE's Result Visibility, Policy and role lines;
  - the web UI plan's step 8 becomes "Release and notices (with `released_by`)";
  - the pytest README.

## Alternatives considered

- **Tests form the tree:** groups of tests. A single test still couldn't nest.
- **Parts built as the test runs** (`with score.group(...)`): easiest to write, but unreached
  parts vanish from the maximum.
- **Points on leaves only:** simpler totals, but every direct check would need a tiny leaf.
- **`scale=` (a factor) instead of `worth=`:** says the same thing less directly, and goes
  stale when the sub-rubric's total changes. A sub-rubric per weighting means duplicated
  classes.
- **Per-test visibility only:** fewer rules, but a part that should wait for release would
  have to be split into its own test.
- **Keeping course overrides,** or the assignment.yml layer: a quick change without
  rebuilding, but three places decide what students see.
- **Per-part change summaries in notices:** more precise, but longer emails, and they
  depend on stable part paths across uploads.
- **No depth cap:** relies on serde's limit of 128, and our own recursion would need care.
- **No version field:** an older platform would silently drop the nesting.

## Open questions

- **The keyword's name** (`part`), and whether its sub-rubric comes from the annotation or
  an argument. I'd accept both, the annotation first.
- **A part's `feedback` visibility:** whether it should also hide its sub-parts' scores
  (as written above), or only its own steps.
- **Grading's "couldn't be graded" wording** for oversized or too-new results, for
  students (staff get the reason).

## Rollback

- **PR A** (removal): revert it. Migration 0018's down migration re-adds the column empty,
  so overrides don't come back.
- **pedagog-pytest 3:** a new major version beside 2. Assignments stay on `pytest:2` until
  they choose 3; reverting is pinning back.
- **Core, web and hammer:** v1/v2 files read as before, and format 3 is additive. Reverting
  makes format-3 results invalid again ("too new").

## Steps (PRs)

1. **PR A, removal** (`feat!/visibility-from-tests`):
   - store migration 0018;
   - core `policy` without layers;
   - api endpoints removed;
   - assignment.yml `results:` refused;
   - hammer commands removed;
   - recipes PR: the example's `@visibility`;
   - e2e and docs; the web UI plan's step 8 revised.
2. **pedagog-pytest 3** (its repo): `part(...)` with `worth=`, own points, per-part visibility,
   `format: 3`, the nested trace, README; tag v3.0.0; recipes `pedagog-pytest/3.yaml`.
3. **Core and jobs** (`feat/nested-results`):
   - format 3 parsing and validation (depth 8, ids, sums within tolerance for scaled parts, clamps);
   - the size check;
   - views and change comparison;
   - fixtures from the real helper output.
4. **Hammer, api and web** (`feat/nested-feedback-views`):
   - the recursive part component, as in the mock;
   - hammer's indented results;
   - an Assignment-3-like example in `recipes/examples` (deep parts, a part visible after
     release);
   - e2e: a nested part's feedback reaches the student page, and a hidden part leaves the
     totals.
