# Sanitized errors

## Rationale

A failing test's `error` is the framework's raw report. For pytest that's the whole
test function's source, argument dumps (`score = <Score object at 0x…>`), object
addresses, absolute paths and helper/pytest internals: noisy for students, and it
shows them test code they weren't meant to read. The failure corpus
(`test-frameworks/python-pytest/corpus/REPORT.md`) shows today's output for eleven
kinds of failure.

## Decisions

- Two optional fields per test: `error` (the framework's raw report) and
  `error-sanitized`. Students get `error-sanitized` when it's there, else `error` (as
  today), on the `error` part's visibility schedule (unchanged: after the deadline by
  default). Staff see both.
- `error-sanitized` is a summary (where and what: the student's line when the
  failure is in their code, else the test's), then the call chain that led there:
  each frame that matters with its one line of code; repeated frames folded (`×2`).
  Never: the test's source listing, argument dumps, object addresses (`<LinkedList>`
  instead of `<linked_list.LinkedList object at 0xffff…>`), absolute paths (relative
  to the submission or the tests), helper/pytest/Python frames, "Use -v to get more
  diff".
- Each framework's helper produces it from the real exception.
- A sanitizing helper declares it once per results file (top-level
  `sanitizes-errors: true`), so it's known at validation even when the reference
  solution passes everything. Without it, staff are warned that students who can see
  errors get the raw report:
  - at upload: the build's validation carries a warning, printed by `hammer upload`
    and shown on the build page (this PR);
  - on the staff assignment overview, a standing alert while the current build's
    helper doesn't sanitize (with the staff pages, PR 4);
  - in the staff results, a mark on each test whose error reaches students raw
    (with the results pages, PR 5).
- A run that couldn't load the student's code (a syntax or import error) gets
  `run.output-sanitized` too: where and what, e.g. `linked_list.py, line 2:
  SyntaxError: expected ':'` with the caret lines; the "couldn't be graded" card shows
  it under the sentence it has today.
- A setup error the student didn't cause (it happened in the tests or their fixtures,
  not in the student's code) is sanitized as "the tests couldn't set up; this isn't
  your code" (the exception stays in `error` for staff). One that fails inside the
  student's code (a fixture building their list) reads like any other failure.
- The student page shows the summary and folds the chain ("How it got there"). Raw
  reports (from helpers that don't sanitize) show as today.

## What students would see (drafts from the corpus)

1. An assert comparing values:
   ```
   test_linked_list.py:18: assert 2 == 3
    +  where 2 = len(<LinkedList>)
   ```
2. An assert with the instructor's message:
   ```
   test_linked_list.py:23: to_list() should keep insertion order
   assert [2, 1] == [1, 2]
     At index 0 diff: 2 != 1
   ```
3. An exception in the student's code:
   ```
   linked_list.py:33: AttributeError: 'NoneType' object has no attribute 'value'

   test_linked_list.py:28, in test_pop_empty
       items.pop_front()
   linked_list.py:33, in pop_front
       value = self.head.value
   ```
4. An exception in the test (a method the student didn't write):
   ```
   test_linked_list.py:33: AttributeError: 'LinkedList' object has no attribute 'reverse'
   ```
5. An expected exception that never came:
   ```
   test_linked_list.py:40: expected IndexError, but nothing was raised
   ```
6. Failed `check()` blocks: each check's summary (its own chain when it's in the
   student's code), in order.
7. A setup error in the tests: `The tests couldn't set up; this isn't your code.`
8. A deep failure in the student's code:
   ```
   linked_list.py:42: AttributeError: 'NoneType' object has no attribute 'value'

   test_linked_list.py:70, in test_find
       items.find(3)
   linked_list.py:38, in find
       return self._find(self.head, value)
   linked_list.py:44, in _find (×2)
       return self._find(node.next, value)
   linked_list.py:42, in _find
       if node.value == value:
   ```
9. Printed output that differs:
   ```
   test_linked_list.py:77: assert '3 -> 2 -> 1\n' == '1 -> 2 -> 3\n'
     - 1 -> 2 -> 3
     + 3 -> 2 -> 1
   ```
10. A timeout: `linked_list.py:58: ran past its 1 s time limit (here, in spin)`
11. A syntax error in the student's file (the whole run, `run.output-sanitized`):
    ```
    linked_list.py, line 2: SyntaxError: expected ':'
        def append(self, value)
                               ^
    ```

## Under process isolation (checked in kind)

Real grading runs student code in its own process; the corpus graded that way
(`corpus/REPORT-isolated.md`) differs from the in-process run, and the sanitizer
works from what isolation hands back:

- An exception in the student's code comes back re-raised in the test process, with
  the student's frames appended to its message as `student code traceback:` lines.
  The summary's location and the chain come from those frames (today's short line
  points at the test instead).
- A timeout under isolation is 69 lines of RPyC internals ending in "student code
  exceeded its 1s time limit or its process exited unexpectedly"; the student's
  process is gone, so where it was stuck isn't known. Sanitized: `ran past its 1 s
  time limit (or its process stopped)`.
- A syntax error arrives as `_get_exception_class.<locals>.Derived: expected ':'` with
  the run's temporary path; sanitized back to `SyntaxError` and `linked_list.py`.
- pytest's value explanations name objects as `<bound method LinkedList.pop_front of
  <linked_list.LinkedList object at 0x…>>`; sanitized to `<LinkedList>.pop_front`.
- Not about errors: a test using `capsys` sees none of the student's prints under
  isolation (they happen in the other process). Its own follow-up in the helper.

## Alternatives considered

- One short `message` beside the raw `error`: students would still be one click from
  the raw report and the test's source.
- `--tb=short` only: shorter, but still absolute paths, addresses and internals, and
  every framework decides its own.
- The platform sanitizing text: framework-specific parsing of reports; misses what
  the exception itself knows.
- Students opting into the raw report: one more setting; the raw report stays a leak.

## Open questions

- None left. Process isolation was checked (step 1, `corpus/REPORT-isolated.md`): see
  "Under process isolation" above.

## Rollback

Each step is its own PR. pedagog's lands before the helper's, so nothing ever sends a
field the platform ignores. Reverting the helper (the recipe back to 2.1.0) leaves the
new fields out: students get the raw `error` as today and staff are warned; reverting
pedagog's ignores them.

## Steps

1. **Verify under isolation:** the corpus as an assignment in kind (tests and the
   buggy student file), graded for real; record its `error`s next to the in-process
   ones.
2. **pedagog PR** (`feat/sanitized-errors`, after #61): `core::results::TestResult`
   gains an optional `error_sanitized` (`error-sanitized` in the YAML) and `Results` a
   `sanitizes_errors` flag; the student view's error is the sanitized one when present,
   else the raw one; validation warns when the flag is missing (`hammer upload`, the
   build's status); the staff results api returns both and whether students get raw; ARCHITECTURE's results format and
   visibility sections; the UI shows the summary and folds the chain; tests in core,
   api and web.
3. **python-pytest PR** (`feat/sanitized-errors`): the plugin keeps each failure's
   exception and builds `error-sanitized` (summary from `describe_failure` with the
   fixes above, the chain from the traceback's suite frames with relative paths);
   it writes `sanitizes-errors: true`; the corpus cases become `tests/test_plugin.py`
   cases asserting the exact text;
   version 2.2.0; tag v2.2.0 (the release workflow builds the bundle).
4. **recipes PR:** `toolchains/ubuntu-22/pedagog-pytest/2.yaml` to v2.2.0 and the
   bundle's sha256 (merge commit, not squash).
5. **pedagog chore PR:** bump both submodules; a kind e2e check that a failing test's
   student view shows the sanitized summary and none of the test's source.
