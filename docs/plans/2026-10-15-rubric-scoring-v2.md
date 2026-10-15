# Rubric Scoring v2 (pytest helper)

Reworks scoring in the `pedagog-pytest` helper before its first release (v1.0.0 is not
tagged yet, so breaking changes are free), and the matching `results.yaml` format and
platform checks.

## Rationale

- `award()` replaced the item's score, and a failed `check()` reset it to 0, so one item
  couldn't be built up from several checks.
- There was no way to explain *why* an item got its points, and a single `@rubric_item`
  test's failure details were dropped from results.yaml (a bug).
- Tests had no stable id: the pytest node id changes when a file or function is renamed.
- Extra credit had one mode (adds to the score, not the max); optional work that counts
  once attempted wasn't expressible.
- Two scoring decorators (`@weight`, `@rubric_item`) with different rules for one concept
  were confusing.

## Design

### Decorators

| Decorator | Fixture | Scoring |
| --- | --- | --- |
| `@rubric_item(id=None, name=None, points=0, clamp=(0, FULL), extra_credit=NONE)` | `score` | The test is one item; points only from explicit calls |
| `@rubric(cls, id=None, name=None)` | `scores` | Several items declared on a `Rubric` subclass |
| *(none)* | | Pass/fail only: score 0, max 0 (informational) |

- **`@weight` is removed.** Every scored test uses `@rubric_item` and awards explicitly; an
  item that is never awarded scores 0.
- `@number`, `@visibility`, `@solution_expect` and `@timeout` are unchanged.
- **Items in a rubric:** `item(id=None, name=None, points=0, clamp=(0, FULL),
  extra_credit=NONE)`. `id` defaults to the attribute name; `name` defaults to the id.
- **Ids:** `id=` on `@rubric_item` / `@rubric` is the *test's* stable id. There is no
  separate `@test_id`. Without it the test has no id (see Results). Rubric items always
  have an id (the attribute name unless `id=` is given).
- **Unique ids are enforced:** duplicate declared test ids fail collection (a parametrized
  test with an id gets `id[param-id]`, so its cases stay distinct); duplicate item ids in
  one `Rubric` are a `TypeError` when the class is defined.
- **Name:** `name=` on `@rubric_item` / `@rubric` overrides the display name; otherwise the docstring's first
  line, otherwise the function name (as for every test).

```python
@rubric_item(id="len", name="len counts elements", points=2)
def test_len(score: Score):
    assert len(make_list(0)) == 0
    score.award(1, reason="empty list")
    assert len(make_list(3)) == 3          # fails: the +1 stands, the rest is never awarded
    score.award(1, reason="three items")


class PopFront(Rubric):
    returns_front: Score = item(name="returns values from the front", points=2)
    raises_when_empty: Score = item(name="raises IndexError when empty", points=1)
    reverse: Score = item(id="reverse", name="reverse()", points=2,
                          extra_credit=ExtraCredit.ATTEMPTED_WORK)
    late: Score = item(name="late-file penalty", clamp=(None, 0))


@rubric(PopFront, id="pop-front", name="pop_front")
def test_pop_front(scores: PopFront):
    ...
```

### `Score`

- `award(points=FULL, reason=None)` **adds** points (may be negative). `FULL` adds the rest
  of the item's points (`points - running total`), or 0 if the total is already there.
- `set(points, reason=None)` **replaces** the running total (for computed scores).
- `nil()` resets the running total, the item's feedback lines, and "attempted".
- `check(points=FULL, reason=None)`: a context manager. If the block completes, it adds
  `points`. If it raises, it adds 0, records the failure, and the test continues (it ends
  as failed). Time limits still propagate.
- `attempt()` marks an `ATTEMPTED_WORK` item attempted with no points.
- **Awards stand** if the test fails afterwards. An uncaught failure just stops later awards.

### Clamp

- Per item: `clamp=(low, high)`, default `(0, FULL)` where `FULL` is the item's points. Either
  bound may be `None` (unbounded); `clamp=None` means `(None, None)`.
- Applied **once to the final total**, so awards and deductions commute.
- Unbounded items can score below 0 or above their max; test and summary totals are sums.

### Extra credit

- `ExtraCredit.NONE` (default): counts in the score and the max.
- `ExtraCredit.BONUS`: counts in the score, never the max.
- `ExtraCredit.ATTEMPTED_WORK`: counts in the score, and in the max only if attempted.
  Attempted = any positive points awarded, or `attempt()` called.

### Feedback

A checklist per item, plus test-level lines:

```text
✓ +1 empty list                         check() that completed
✗ +0 one element (worth 1)              check() that raised, then its cleaned failure:
  test_sort.py:9: AssertionError: assert [] == [5]
+1 first 10 elements correct            award(reason=)
=3 3/4 cases                            set(reason=)
✗ test_len.py:6: AssertionError: assert 2 == 3      uncaught failure (test level)
```

- Cleaned failure: the innermost frame in the test suite (path relative to the tests root)
  and the exception with pytest's assertion detail. No helper or RPyC frames.
- Feedback follows the test's **output** visibility (it can reveal what a hidden test checks).
- Feedback size is capped like `output`, per field.
- `@rubric_item` also takes `extra_credit` (a whole test as extra credit), with the same
  meaning as on items.
- Rubric items carry `feedback`; `@rubric_item` tests (and uncaught failures in `@rubric`
  tests) carry it at the test level. Pytest's own failure text stays in `output`.

### Grading trace (instructors)

A step-by-step account of how each score was calculated, for instructors only (never shown
to students, whatever the visibility settings):

- In results.yaml as a per-test `trace` field, so the platform can show "how was this
  calculated" per submission.
- In pytest's terminal summary, so build and validation logs (and local runs) show it too.

```text
pedagog grading: pop-front — pop_front (rubric PopFront)
  returns_front  points 2, clamp [0, 2], none
    check ✓ +2  returns values from the front      = 2
    final 2                                         2 / 2
  raises_when_empty  points 1, clamp [0, 1], none
    check ✓ +1  raises IndexError when empty       = 1
    final 1                                         1 / 1
  reverse  points 2, clamp [0, 2], attempted-work
    attempt()                                       attempted
    check ✗ +0  reverses (worth 2)                 = 0
    final 0; attempted -> counts in max             0 / 2
  late  points 0, clamp [-inf, 0], none
    award -1  one file late                         = -1
    final -1                                       -1 / 0
  test: 2 + 1 + 0 - 1 = 2;  max 2 + 1 + 2 + 0 = 5   2 / 5
  status: fail (check on 'reverse' failed)

summary: score 2 + ... = 14, total 5 + ... = 20
```

- Every step is listed: `award`/`set`/`nil`/`check`/`attempt` with running totals, where the
  clamp changed the total, why an extra-credit item did or didn't count in the max, and why
  the test passed or failed.

### Results format

```yaml
tests:
  - id: pop-front                 # only when declared
    node: test_linked_list.py::test_pop_front   # always; pytest's node id
    name: pop_front
    status: fail
    score: 3
    max: 5                        # 2 + 1 + reverse's 2 (attempted) + late's 0
    output: "..."                 # pytest failure text
    feedback: ""
    trace: "..."                  # instructor-only grading trace
    visibility: { score: visible, output: after_end }
    items:
      - id: returns_front
        name: returns values from the front
        score: 2
        max: 2
        clamp: [0, 2]
        extra-credit: none        # none | bonus | attempted-work
        attempted: true
        feedback: "✓ +2 returns values from the front"
      - id: late
        name: late-file penalty
        score: -1
        max: 0
        clamp: [null, 0]
        extra-credit: none
        attempted: true
        feedback: "+-1 one file late"
  - node: test_linked_list.py::test_len           # @rubric_item test
    name: len counts elements
    score: 1
    max: 2
    clamp: [0, 2]
    extra-credit: none
    attempted: true
    feedback: |
      +1 empty list
      ✗ test_len.py:6: AssertionError: assert 2 == 3
```

- Item `output` becomes `feedback`; `extra-credit` changes from a bool to an enum; `id`
  becomes optional and `node` is added.

### Platform checks (`core::results`)

- Unique `node`s; unique declared `id`s; unique item ids within a test.
- Each item's (or `@rubric_item` test's) score is within its `clamp` bounds.
- A test with items: score = sum of item scores; max = sum of the maxes of `none` items and
  attempted `attempted-work` items.
- Summary: score = sum of test scores, total = sum of test maxes. (This replaces "score ≤
  total", which no longer holds with unbounded clamps.)
- `solution_mismatches` is unchanged.

## Alternatives considered

- **Keep `@weight`** for all-or-nothing tests: less boilerplate, but two scoring rules for
  one concept. Rejected in favour of one explicit rule.
- **Untouched item = full points if the test passes:** removes boilerplate, but
  deduction-only paths silently lose the rest of the points, and validation may not catch
  it.
- **Fixture-based rule** (asking for `score` switches to explicit scoring): static, but it
  has a silent-zero trap.
- **`award` replaces, `check(add=True)` accumulates:** two behaviours behind one flag.
- **Clamp per call, or after every call:** order-dependent results, and an item's range
  isn't visible in one place.
- **`@id` / `@test_id` decorators:** `@id` shadows the builtin `id()`; a separate decorator
  duplicates `id=` on the scoring decorators.
- **Item id `None` when undeclared:** undeclared items could only be told apart by position.
- **Feedback following score visibility:** better for explaining points, but failure text
  can leak what a hidden test checks.

## Open questions

- None outstanding. Decided in review: the change lands as a new helper PR on top of the
  open stack (#1–#3), so the delta is reviewable on its own.

## Rollback

- The helper is unreleased; revert the PR (or don't merge it) and v1.0.0 ships with the v1
  scoring API.
- Platform side: the `core::results` change and its fixture are one revertable commit; the
  recipes example change is one commit on recipes PR #4.

## Implementation steps

1. **Helper (new PR stacked on #3):**
   1. `scoring.py`: additive `award`, `set`, `nil`, `check(points, reason)`, `attempt`,
      `ExtraCredit`, clamp, feedback lines, `item(id, name, points, clamp, extra_credit)`.
   2. `markers.py`: remove `weight`; `rubric_item(id, name, points, clamp, extra_credit)`;
      `rubric(cls, id)`.
   3. `plugin.py`: undecorated tests score 0/0; `node`/`id`; test-level and item feedback;
      cleaned failures; clamp and extra credit in `earned()`/max and the summary; duplicate
      declared ids fail collection; the grading trace (results field and terminal summary).
   4. Tests for each rule above (additive checks, clamp bounds and `None`s, commuting
      deductions, each extra-credit mode, attempted, feedback lines and cleaning, ids,
      undecorated tests, the trace), then the README.
   5. Run the suite as root with real users on 3.10 and 3.12; `mypy --strict`.
2. **Recipes PR #4:** convert the example to `@rubric_item` with ids, reasons and an
   `ATTEMPTED_WORK` stretch goal.
3. **pedagog:** `core::results` model (including `trace`) and checks above, a regenerated fixture from the
   example, ARCHITECTURE.md's results format, and the assignment-builds plan's helper section.
