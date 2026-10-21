# PR 3: the student pages

Step 3 of `2026-10-20-web-ui-implementation.md`. Design artboards: Home (student),
Assignments (student), AssignmentStudent (graded, selected, refused, grading, error),
StudentResults, the phone variants, Flow 1.

## Rationale

Students can log in but see only course names. This PR is everything a student does:
see what's open, submit, watch it grade, read results and history. It's the first
slice a tryout user would touch.

## What the api is missing (students get 403 or less than the design shows)

- **Assignment list:** `[{slug, name}]` only. No dates, state or own status.
- **One assignment:** `GET …/{slug}` needs staff. Required files, size limit and archive
  rules never reach students; they meet them only as a refused submit.
- **Dates status:** a sentence ("it opens at …"), not a value the UI can badge.
- **History:** no shown score per submission (one `my-results` call each would do it).
- **Home summaries:** nothing beyond `{id, name, role}`.

## Changes

### Api (typed, in `core::api`)

- `GET /courses/{c}/assignments`, students: each assignment with its effective dates,
  a `state` (`open`, `open_late`, `upcoming {opens}`, `closed`, `no_dates`) beside
  the sentence, and the student's own summary: latest submission's status, the shown
  score of the one that counts, whether it was late. Staff keep their shape for now
  (PR 4 types it).
- `GET /courses/{c}/assignments/{slug}`, members: students get name, dates, state, and
  the submission rules they need (required files, size limit, archives unpacked,
  include patterns). Staff get today's response.
- `GET …/{slug}/submissions`, students: each row gains `shown {score, total}` (what
  they'd see in `my-results` for it now), `files` and `size_bytes` from the receipt.
- `my-results`, submission show, submit reply: moved to `core::api` types, same fields.
- `GET /api/me/courses` (new): your courses, each with what needs you, for Home in
  one request. Students get open, late-open and soon-opening items with their own
  status; staff get the same shape with needs-attention items in PR 4. `/me` stays
  small for the shell.

### Web

- **Home:** per course, "Open or due soon": open, late-open and upcoming-this-week
  assignments, each with its due line and your state badge; "Nothing needs you right
  now" otherwise.
- **Assignments:** the student list (date badge, when line, your state).
- **Assignment:** header with dates (course time zone later; viewer's for now) and
  relative times; results column (graded with tests, feedback, printed output, error,
  the hidden-tests note; grading steps live; couldn't be graded); history (shown score,
  ★ counts, late, "submitted by … on your behalf"); submit column (drop zone, choose
  files, choose folder, file list with sizes and total of the limit, required files,
  refusals inline, an upload progress bar, receipt with kept / ignored / clutter /
  unpacked).
- **Pre-check:** missing required files or a total over the limit show in the file
  list and disable Submit; the server's rules still decide what's kept and can refuse.
- **Live grading:** `events::watch` on the assignment; Received, Queued (#n, keeping the
  highest seen: "was #14"), Grading, Graded; results fetched when the run finishes.
- **Upload:** `FormData` multipart from the browser, one part per file named by its
  relative path (as hammer sends); folders from `webkitdirectory` and drag-drop
  (`webkitGetAsEntry`). Sent with `XMLHttpRequest` for its upload progress (fetch has
  none), with the same `X-Pedagog` header and 401 handling as the rest of the client.
  New `web-sys` features only, no new crates.
- **Caching:** the list and an assignment `Revalidate` (shown at once, refetched);
  submitting invalidates the assignment, its list and the history.
- **Components:** `badge`, `dates`, `tests` (results), `drop` (file picker), `steps`
  (grading progress), each matching pedagog.css classes.

### Tests

- Native: state from dates, home's "due soon" choice, the queue "was #n" tracker, file
  list totals and required-file matching, relative paths from folder entries.
- Browser components: each assignment view (graded, grading, error, refused, receipt)
  from fixed data; the hidden-tests note; keyboard reach of the drop zone.
- Api: the new student fields and the 403 → member change for one assignment.
- Kind e2e (`student_web`): a student logs in, opens the assignment, submits a file,
  sees grading finish live and the shown score, then the history row.

## Alternatives considered

- **Home from each course's list:** no extra endpoint and a shared cache, but one
  request per course; chosen one request (`/api/me/courses`) for speed.
- **N calls from the UI** (dates and submissions per assignment, `my-results` per
  submission): no api work, many requests per page.
- **Client-side required-file check only:** instant, but the server's rules (include,
  exclude, unpack, strip) decide what's kept; the UI pre-checks only required names
  and the size, the server stays the authority.
- **A busy button instead of progress:** keeps one request path (fetch), but large
  uploads on slow links look stuck.
- **Frozen history scores** (the score shown when each run was graded): rows never
  change, but can disagree with that submission's results today; needs a column.

## Open questions

- Home's "due soon" window: due within 7 days, plus anything open late (design shows
  this); upcoming that opens within 7 days.
- History scores show what the student would see *now* (as `my-results` does), so after
  release older rows can rise.
- Bundle: ~308 KB of the 400 KB budget today; these pages are the biggest yet. If they
  push past it, split the staff pages into a lazily loaded chunk later (PR 4).

## Rollback

Revert the PR. The api changes add fields and open one GET to members; no migration.

## Steps (commits)

1. `feat(api)`: typed student assignment list and show, state enum, rules for students;
   `GET /api/me/courses`.
2. `feat(api)`: shown score and receipt summary in a student's submissions.
3. `refactor(api)`: my-results, submission show and the submit reply in `core::api`.
4. `feat(web)`: components (badge, dates, steps, tests, drop).
5. `feat(web)`: home and the assignment list.
6. `feat(web)`: the assignment page: results, history, live grading.
7. `feat(web)`: submitting: files, folders, pre-check, progress, refusals, receipt.
8. `test(e2e)`: the student flow in the browser.
9. `docs`: ARCHITECTURE.md (student api shapes, upload from the browser).
