# Pedagog Web UI — Design Brief

A brief for designing Pedagog's first web UI. It covers who uses it, every screen and
what each shows and does, every state a screen can be in, and the real data behind
it (from the existing api). Everything here already works through the `hammer` CLI;
the web UI makes it usable without a terminal.

## 1. What Pedagog is

- A platform for programming assignments at universities. Instructors upload an
  assignment (its tests, a reference solution, the environment it builds in); the
  platform builds it into a container image and checks the reference solution passes.
  Students upload their code; it's graded in an isolated container against the tests,
  and students see their results, as much as the instructor allows.
- Multi-course, multi-university (UF first). Courses can be large: **600+ students**.
- This web UI is for the **`upload` platform** (students submit files). In-browser
  coding sessions and exam mode come later (see Out of scope); leave room for them in
  navigation, but don't design them.

## 2. Who uses it

| Role | Where | What they do |
| --- | --- | --- |
| **Student** | A course | See assignments and dates, submit files, see their results and submission history |
| **Grader** (TA) | A course | Everything a student sees, plus every student's results and submissions; submit for a student; make an earlier submission count |
| **Instructor** | A course | Everything a grader does, plus upload assignments, edit dates and what students see, release results, email students, manage the roster |
| **Admin** | Platform | Everything in every course (acts as an instructor anywhere), plus users and their global roles, every course, and error tracking |

- A person's **global role** is `admin`, `staff` (may create courses) or `student`.
  Their **course role** is `instructor`, `grader` or `student`, per course; one person
  can be a student in one course and a grader in another.
- Staff (instructors, graders, admins) can also submit their own work to test an
  assignment; it's marked as a staff submission and never shown as a student's.

## 3. Principles

- **Clarity over density, but dense where staff work.** Students see few things,
  plainly. Instructors work through hundreds of rows; their tables must scan fast.
- **Say what happens and why, in plain words.** The api already returns human
  sentences (refusals, statuses, "why a test is hidden"); show them as written, don't
  replace them with codes.
- **Never show students more than they may see.** Hidden tests aren't greyed out or
  counted; a single line says some are hidden and until when.
- **Progress is visible.** Builds, validation and grading take seconds to minutes;
  every wait shows where it is (queued with position, building, validating, grading).
- **Destructive or student-visible actions confirm** (release, take back a release,
  email students, drop members, demote an instructor, change dates on an open
  assignment).

## 4. Constraints

- **Tech:** built in Leptos (Rust compiled to WebAssembly). Prefer plain, composable
  components (tables, forms, tabs, dialogs, toasts, badges, progress) over heavy
  widgets; no third-party JS component libraries. Styling via CSS custom properties
  (design tokens).
- **Themes:** light and dark, from the same tokens; follow the system setting with a
  manual toggle.
- **Accessibility:** WCAG 2.2 AA: contrast, full keyboard use, visible focus, screen
  reader labels, status never by colour alone (pass/fail/late have text or icons).
- **Devices:** laptop first. Students must be able to check results and dates on a
  phone; staff screens may assume a laptop but must not break on a tablet.
- **Branding:** neutral and institution-agnostic (it will host several universities);
  a product name and logo slot, no UF colours baked in.
- **Times:** shown in the viewer's local time zone with the zone named on hover;
  deadlines also relative ("in 2 days", "3 hours ago"). The api gives RFC 3339.

## 5. Navigation

- **Top bar:** product, course switcher (the user's courses with their role in each;
  admins can search every course), user menu (name, email, global role, log out),
  theme toggle.
- **In a course (left nav or tabs):**
  - Students: **Assignments**.
  - Graders: **Assignments**, **Results** (per assignment), **Members** (read-only).
  - Instructors: the same, plus assignment management and **Settings** (rename).
- **Admin (separate area):** **Users**, **Courses**, **Errors** (opens GlitchTip, the
  self-hosted error tracker, in a new tab).
- Deep links for everything: course, assignment, a student's results, a submission, a
  build.

## 6. Screens

Each screen lists: what it shows, actions, and states. Field names in `code` are the
api's (see the appendix for shapes).

### 6.1 Log in

- Email field → "If this email is registered, a login code is on its way." (always
  the same reply, so it doesn't reveal who has an account).
- Code field: three words like `tiger-maple-river`; case and separators don't matter.
  Errors: "wrong code; N tries left", "no valid code for this email: ask for a new
  one". A code can be re-sent after a minute.
- Session expiry anywhere → back to log in, returning to the page afterwards.

### 6.2 Home

- **Everyone:** their courses (name, id, their role), most relevant first.
- **Students:** per course, assignments that are open or due soon, with their state
  ("submitted, 9/10 shown", "not submitted, due in 2 days", "late until Fri 11:59pm").
- **Staff:** per course, assignments with something to act on (a build failing
  validation, results not released after the deadline, submissions that couldn't be
  graded).
- **Empty:** "You're not in any courses yet" (students are added by their instructor).

### 6.3 Course: assignments (everyone)

- List of assignments: name, dates status (`open`, `open, late`, `it opens at …`,
  `it's closed: the deadline has passed`, `it isn't open yet: no dates set`), due /
  late due, and for students their current state (not submitted / grading / score on
  the tests shown / couldn't be graded).
- **Staff extra columns:** current build, latest build's status (building /
  validating / passed / failed), submitted count of active students.
- **Instructors:** "Upload assignment" action.

### 6.4 Assignment (student)

- **Header:** name, dates (open, due, late due) with status and countdown.
- **Submit:** drag-and-drop files or a folder, or one `.zip` / `.tar.gz` archive.
  - Before sending: the list of files and total size.
  - Refusals, shown inline as returned: not open yet, deadline passed, missing required
    files ("missing required files: linked_list.py"), too large, unsafe names, a
    workspace assignment ("its submissions come from the session workspace").
  - **Receipt** after sending: files kept; files ignored by the assignment's rules;
    OS clutter dropped (`.DS_Store`, `__MACOSX`); a wrapping folder removed; archive
    unpacked; size; whether it's **late** ("late by 2 hours 14 minutes").
- **Live status** of the new submission: queued (position N) → grading → graded, or
  couldn't be graded (see below). Polls; no reload needed.
- **Results** (of the submission that counts):
  - "N/M on the tests shown" and a table of tests shown: name, pass/fail/error/skip,
    points.
  - Per test, expandable: **feedback** (the instructor's checks: each step with pass
    or fail, points, and the instructor's reason or message), **printed output**
    (`stdout`), **error** (what went wrong). Each appears only if the student may see
    it now.
  - "Some tests are hidden until the deadline / until release" (`hidden_until`), or
    "some tests aren't shown" (`hidden_never`). Never their names or points.
  - **Couldn't be graded:** a plain sentence (`why`): "the tests couldn't run with your
    code", "it ran past the time limit", "it ran out of memory", "it wrote more to
    disk than allowed", "the test run stopped unexpectedly", "something went wrong on
    our side"; then the next step (`next`): "check your files and submit again; if
    problems persist, contact course staff".
- **History:** every submission, newest first: when it counts from (`counted_at`),
  late or on time, status, which one **counts** (the latest, unless staff made an
  earlier one count: "counts (chosen by staff@ufl.edu)"), and "submitted by
  ta@ufl.edu on your behalf" when staff sent it. Each opens its own results and
  receipt.

### 6.5 Assignment (staff): overview

- **Builds:** the latest 10 (`build_status`, `validation_status`, current or not),
  each opening a build page (6.7).
- **Dates** (6.9), **Visibility** (6.10), **Results** (6.11) as tabs or sections.
- **Instructor actions:** upload a new version (6.6), release / take back release,
  email students (6.13).

### 6.6 Upload assignment (instructor)

- Pick a folder (the assignment directory, with `assignment.yml`) or an archive.
- Options: **Rebuild even if unchanged** (`force`); **Regrade every student's
  submission** with **tell students: never / if their results changed / always**
  (`regrade`); **Use the file's dates** (`take_dates`).
- After sending:
  - "already built and validated as build N; nothing to do" or "build N dispatched";
  - warnings (e.g. "no reference solution: validation will be skipped");
  - if the course's dates differ from the file's: a small table (field, course's,
    file's) and the note ("the course's dates stay; Use the file's dates adopts the
    file's");
  - then live progress: building → queued for validation → validating → passed /
    failed.
- **Refusals** (422 and friends): the assignment file's problems, listed as returned.

### 6.7 Build (staff)

- Status (`build_status`: running / succeeded / failed / superseded by a newer upload;
  `validation_status`: pending / queued / running / passed / failed / skipped /
  invalid), whether it's current, image reference.
- **Failed build:** the build log's tail (monospace, scrollable, copyable).
- **Validation:** the reference solution's results per test (the same test view as
  6.12, all parts shown), and **mismatches** (`validation_problems`), e.g.
  "pop-front: expected the solution to pass, but it failed". A failed validation never
  replaces the current build: say so.

### 6.8 Course members (staff; instructors manage)

- Table: name, email, role, source (`created`, `manual`, `import`), added by, joined,
  dropped. Filters: role, include dropped; search.
- **Instructors:**
  - **Add people:** emails (one or many), role, optional name. Result per email:
    added / restored / role changed (from → to) / already a member.
  - **Import roster:** upload a CSV (`email,name`) or Canvas's gradebook export;
    **Preview** first (dry run: how many listed, who'd be dropped with "also drop
    students not in the file", skipped rows with why); then import (added, restored,
    already, staff left alone, dropped, skipped).
  - **Change role**, **drop** (soft: they lose access, keep history; restorable).
    Guard rails, as returned: "the last instructor" can't be removed; only an admin
    can remove or demote another instructor.
- **Graders** see the list read-only and may drop only themselves.

### 6.9 Dates (instructor edits; everyone sees status)

- Three fields: **open** (a time, or "always"), **due** (after this, late), **late due**
  (after this, refused). Each shows its value and where it comes from (the course's
  edit, or the assignment file's), with "reset to the file's" per field.
- Validation inline as returned: "late_due needs a due date", "open is after due", "due
  is after late_due".
- Show the resulting status ("open, late until …").

### 6.10 Visibility (staff see; instructors edit)

What students see of each test, in four parts: **score** (points and pass/fail),
**feedback** (the checks), **stdout** (printed output), **error** (what went wrong).
Each is `visible`, `after_end` (after the deadline, or on release), `after_publish`
(on release only) or `hidden`.

- Table: each test (name, id), its four parts' resolved values, and whether students
  see each **now**, with the reason on hover ("shown: results are released", "not yet:
  until the end (Fri 11:59pm) or release", "never shown to students").
- **Explain** (per test): the layers that decided it, most specific first: course for
  this test, course for all tests, file for this test, file for all tests, the test's
  own default.
- **Edit:** set any part for **all tests** or **one test**; clear one test's overrides,
  the all-tests entry, or everything. Edits apply at once (no rebuild); say so.
- **Released?** banner with release / take back (6.13).
- Empty: "no validation results to list tests from (no reference solution, or not
  built yet)".

### 6.11 Results table (staff)

The busiest screen. 600+ rows; must sort, search and scan fast.

- One row per student who submitted (`submitted` of `students` active), sorted by
  name; search by name or email; toggles: include dropped students, include staff
  submissions.
- Columns: name, email, the submission that counts (and "chosen by …" if activated),
  late (duration), status (queued / grading / graded / couldn't be graded + kind), and
  **four cumulative score columns**: **visible now**, **after the deadline**, **after
  release**, **everything (incl. hidden)**, each as score/total and tests passed. A
  marker (★) on the column **students see right now** (`student_stage`), and the
  header says why ("results aren't released; the deadline has passed").
- Row → student results (6.12).
- Export (CSV) is planned; leave room for it in the toolbar.

### 6.12 A student's results (staff)

- Header: student, the submission shown (default: the one that counts), counted at,
  arrived at, submitted by, late, and a picker of their other submissions.
- **Everything**, including what students can't see:
  - score per stage (the four columns above);
  - the run's own output (`run_output`) and, if it errored, its kind, error and log
    tail;
  - per test: status, points, rubric items with steps, feedback, stdout, error, and the
    **trace** (staff-only), each part labelled with whether the student sees it now
    and why;
  - "the student is told: some tests are hidden until …".
- **Actions:** **Make this submission count** (activate; it counts until the student
  submits something newer) / **Go back to the latest**; **Submit for this student**
  (files as in 6.4, plus **counts at**: now / their last submission's time / a chosen
  time; the student sees who submitted it).
- **Submissions** list: each with time, late, status, score, counts, submitted by.

### 6.13 Release and notices (instructor)

- **Release results:** shows `after_publish` parts (and `after_end` early). Confirm
  dialog: what changes for students, and "students with a submission are emailed
  (no scores in the email)". Result: "released; emailing N students", or "already
  released".
- **Take back:** confirm with the warning "students may already have seen the released
  results".
- **Email students:** everyone with a submission that counts, or one student; each gets
  their current shown score. Result: "emailing N students".
- Regrade emails go out automatically (chosen at upload); no UI beyond 6.6.

### 6.14 Course settings (instructor)

- Rename the course's id (slug) and/or name. Warn that old links and the old id stop
  working ("renamed to … on …" is shown to anyone using the old one).

### 6.15 Create course (global staff, admins)

- id (lowercase letters, digits, dashes, e.g. `cop3502-f26`), name, optional
  instructors (emails; default: you). Errors as returned (id taken, invalid id).

### 6.16 Admin: users

- Table: email, name, global role; filter by role; search; paged (50).
- **User page:** role, created, last login (and from web or CLI), active sessions (web,
  CLI), courses (with role, joined, dropped).
- **Actions:** set global role (admin / staff / student; refused for the last admin),
  end all their sessions.

### 6.17 Admin: courses

- Every course, searchable, paged; open any (admins act as instructors everywhere).
- Create course (6.15).

## 7. States every screen needs

- **Loading** (skeletons for tables; no layout jump).
- **Empty** with the next step (no assignments yet → "Upload assignment" for
  instructors, "Nothing assigned yet" for students).
- **Waiting/progress:** queued with position, building, validating, grading; live.
- **Couldn't be graded / failed build / failed validation:** plain sentence first,
  details (staff only) expandable.
- **Refused:** the api's sentence inline next to the action, not a generic error.
- **Permission denied:** "only this course's instructors can do that" (as returned),
  with a way back.
- **Not found / renamed:** "no course X (renamed to Y on …)" with a link to Y.
- **Session expired:** log in again, then return.
- **Offline / service unreachable:** "the grading service is unreachable" and retry.
- **Long content:** logs and outputs in scrollable, copyable monospace blocks with a
  size cap and "show all".

## 8. Components to design

App shell (top bar, course switcher, nav); data table (sticky header, sort, search,
filters, paging, row actions, 600+ rows); status badges (run, build, validation,
dates, late, counts, staff, dropped); score display (s/t, per stage, star for "students
see"); file drop zone + file list; receipt; progress tracker (multi-step, live);
test result row with expandable parts (steps list, output blocks); visibility matrix
(tests × four parts, with value, now-shown, reason tooltip, layer explanation);
date-time field with "always"; confirm dialog; toast; inline field errors; empty
states; code/log block; role chips; user menu; theme toggle.

## 9. Copy and tone

- Short, direct, second person to students ("your code", "you can submit until …").
- Use the api's sentences verbatim where it gives them (refusals, `why`, `next`,
  statuses, dates status, visibility reasons). They were written for people.
- Numbers as written: "9/10 on the tests shown", "late by 2 hours 14 minutes".
- Never blame the student for platform errors ("something went wrong on our side").

## 10. What to deliver

1. Design tokens (colour, type, spacing, radius, shadow) for light and dark.
2. The component set (section 8) with states (default, hover, focus, disabled,
   loading, error).
3. Screens 6.1–6.17 at laptop width; 6.1, 6.3, 6.4 also at phone width.
4. The key flows end to end: student submits → watches grading → reads results;
   instructor uploads → watches build and validation → sets visibility → releases;
   grader finds a student → activates an earlier submission.

## 11. Assumed here, not in the api yet

The design may use these; they'll be added with the UI:

- Home summaries: a student's state per assignment across courses, and staff's "needs
  attention" list (today: one request per assignment).
- The assignments list with each student's own state and staff's submitted count.
- Serving the api under `/api` on the UI's host (today it's at the root).
- CSV export of the results table.

## 12. Out of scope (for now)

- Workspace sessions (in-browser VS Code), exam mode (Safe Exam Browser), live build
  logs streaming, grade export and download (leave toolbar room), per-student
  extensions, late penalties, SSO, the email templates (plain text, done), the CLI.

---

## Appendix A: API (what each screen reads and writes)

The api is JSON over HTTP on the same host under `/api` (planned; today at the
root). Errors are `{"error": "<sentence>"}`. The web session is an HttpOnly cookie.

| Screen | Endpoints |
| --- | --- |
| Log in | `POST /auth/start {email}`, `POST /auth/finish {email, code}`, `POST /auth/logout` |
| Shell | `GET /me` → `{user: {email, name, role}, courses: [{id, name, role}]}` |
| Home / course list | `GET /courses` (`?all&search&page` for admins) |
| Course | `GET /courses/{c}` (staff) → instructors, graders, student counts |
| Assignments | `GET /courses/{c}/assignments` → `[{slug, name}]` (+ `current_build`, `latest_build` for staff) |
| Assignment (staff) | `GET …/assignments/{slug}` → latest 10 builds |
| Upload | `POST /courses/{c}/assignments` (gzip tar; `?force&regrade&take_dates`) → upload id, build decision, warnings, date diff |
| Build | `GET …/builds/{id}` → statuses, error log tail, validation problems, results |
| Submit | `POST …/assignments/{slug}/submissions` (multipart; `?for&at`) → receipt, queue position, late |
| Submissions | `GET …/assignments/{slug}/submissions` (`?for`) → history with counts / activated by |
| A submission | `GET /courses/{c}/submissions/{id}` → receipt, runs (status, queue position; staff: scores, results, changes) |
| My results | `GET …/my-results` (`?submission`) → status, why, next, results as the student sees them |
| Results table | `GET …/results` (`?search&dropped&staff`) → rows with four stage tallies, `student_stage` |
| Student results | `GET …/results/{email-or-name}` (`?submission`) → everything, with per-part shown/why |
| Activate | `POST` / `DELETE /courses/{c}/submissions/{id}/activate` |
| Dates | `GET` / `PUT` / `DELETE …/dates` → each field's value and source, status |
| Visibility | `GET` / `PUT` / `DELETE …/visibility` → per test, four parts, value, layer, shown now, why |
| Release | `POST` / `DELETE …/release` → `told` / warning |
| Notify | `POST …/notify` (`?for`) → `queued` |
| Members | `GET /courses/{c}/members` (`?role&dropped`), `GET`/`PUT`/`DELETE …/members/{email}`, `POST …/members/import` (`?sync&dry_run`, CSV body) |
| Course settings | `PATCH /courses/{c} {id?, name?}`; create: `POST /courses {id, name, instructors?}` |
| Admin users | `GET /users` (`?role&search&page`), `GET /users/{query}`, `PUT /users/{email}/role`, `DELETE /users/{email}/sessions` |

Search endpoints (`/users/{q}`, `…/members/{q}`, `…/results/{q}`) return one object
for a single match, or `{matches: […], more}` for several (pick one).

## Appendix B: Vocabulary

- **Submission that counts:** the latest by counted time, unless staff made an earlier
  one count (until a newer one arrives).
- **Counted time vs arrival:** staff submitting for a student choose when it counts
  (now, the student's last time, or a time); lateness uses counted time.
- **Stages** (cumulative, what students would see): visible now → after the deadline →
  after release → everything. Students see one of the first three.
- **Run status:** queued, grading, graded, error. **Error kinds:** load, timeout,
  out of memory, out of disk, crash, no results, invalid, internal.
- **Build status:** running, succeeded, failed, superseded. **Validation:** pending,
  queued, running, passed, failed (solution disagrees with tests), skipped (no
  solution), invalid (results unusable).
- **Visibility:** visible, after_end, after_publish, hidden. **Parts:** score, feedback,
  stdout, error.
- **Member source:** created (made the course), manual, import.
