# Sanitized errors

## Rationale

A failing test's `error` is the framework's raw report. For pytest that's the whole
test function's source, argument dumps (`score = <Score object at 0x…>`), object
addresses, absolute paths and helper/pytest internals: noisy for students, and it
shows them test code they weren't meant to read. The failure corpus
(`test-frameworks/python-pytest/corpus/REPORT.md`) shows today's output for eleven
kinds of failure.

## Decisions

- Two fields per test: `error` (raw, staff only) and `error-sanitized` (students).
  Staff see both. Students only ever get `error-sanitized`, on the `error` part's
  visibility schedule (unchanged: after the deadline by default).
- `error-sanitized` is a summary (where and what: the student's line when the
  failure is in their code, else the test's), then the call chain that led there:
  each frame that matters with its one line of code; repeated frames folded (`×2`).
  Never: the test's source listing, argument dumps, object addresses (`<LinkedList>`
  instead of `<linked_list.LinkedList object at 0xffff…>`), absolute paths (relative
  to the submission or the tests), helper/pytest/Python frames, "Use -v to get more
  diff".
- Each framework's helper produces it from the real exception. Results without it
  (older helpers) show students no error text until their assignment is rebuilt on a
  helper that fills it; staff see a note saying so.
- The page shows the summary and folds the chain ("How it got there").

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
7. A setup error: `test_linked_list.py:61: ConnectionError: the course's test server is down`
   (setup errors are the course's, not the student's; see Open questions).
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
11. A syntax error in the student's file (the whole run): see Open questions.

## Alternatives considered

- One short `message` beside the raw `error`: students would still be one click from
  the raw report and the test's source.
- `--tb=short` only: shorter, but still absolute paths, addresses and internals, and
  every framework decides its own.
- The platform sanitizing text: framework-specific parsing of reports; misses what
  the exception itself knows.
- Students opting into the raw report: one more setting; the raw report stays a leak.

## Open questions

- The run-level error (a syntax or import error in the student's file): students get
  only "the tests couldn't run with your code". Add `run.output-sanitized`
  (`linked_list.py, line 2: SyntaxError: expected ':'` and the caret lines) shown on
  the "couldn't be graded" card?
- Setup errors are the course's fault: show students "the tests couldn't set up; this
  isn't your code" instead of the exception?
- Process isolation: the corpus runs in-process; real grading runs student code in a
  separate process (RPyC). The expected outputs above are checked against a real
  cluster run before the helper's tests are written.

## Rollback

Each step is its own PR. pedagog's lands before the helper's, so nothing ever sends a
field the platform ignores. Reverting the helper (the recipe back to 2.1.0) leaves
`error-sanitized` empty, which the platform handles (students see no error text, staff
the raw one); reverting pedagog's returns to showing students the raw `error`.

## Steps

1. **Verify under isolation:** the corpus as an assignment in kind (tests and the
   buggy student file), graded for real; record its `error`s next to the in-process
   ones.
2. **pedagog PR** (`feat/sanitized-errors`, after #61): `core::results::TestResult`
   gains `error_sanitized` (`error-sanitized` in the YAML); the student view's error
   comes from it; the staff results show both; ARCHITECTURE's results format and
   visibility sections; the UI shows the summary and folds the chain; tests in core,
   api and web.
3. **python-pytest PR** (`feat/sanitized-errors`): the plugin keeps each failure's
   exception and builds `error-sanitized` (summary from `describe_failure` with the
   fixes above, the chain from the traceback's suite frames with relative paths);
   the corpus cases become `tests/test_plugin.py` cases asserting the exact text;
   version 2.2.0; tag v2.2.0 (the release workflow builds the bundle).
4. **recipes PR:** `toolchains/ubuntu-22/pedagog-pytest/2.yaml` to v2.2.0 and the
   bundle's sha256 (merge commit, not squash).
5. **pedagog chore PR:** bump both submodules; a kind e2e check that a failing test's
   student view shows the sanitized summary and none of the test's source.
