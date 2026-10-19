# API and Auth

The next milestone after grading: a public `api` service with email login, courses,
rosters and roles, through which `hammer` uploads assignments, submits work and reads
results, filtered by what students may see. Assignments gain dates and per-course
visibility overrides. The web UI is the milestone after this, built on the same api.
Decisions here come from a design review on 2026-10-18.

## Rationale

- Everything so far is internal: `jobs`' endpoints have no authentication, students
  are opaque ids, assignment ids are global, and every caller sees every result
  (hidden tests included).
- The web UI, sessions and real courses all need the same things first: who someone
  is, what they may do in which course, and what of a result they may see.
- Grading already records what students should be told (regrade notices) and what
  each test lets them see (`visibility` per test); nothing delivers or enforces it.

## Design

### Shape

```text
hammer ──token──▶ api (pedagog-system, Traefik + cert-manager TLS)
                   │  who are you? may you? what may you see?
                   ├─▶ jobs (pedagog-builds, internal only: upload, submit, status)
                   ├─▶ Postgres (users, courses, sessions, …)
                   └─▶ SMTP (login codes, regrade notices)
```

- `api` is the only public entry point; a NetworkPolicy lets only it (and the
  operator's port-forward) reach `jobs`' port. `jobs` keeps doing the work and returns
  everything; `api` decides who may call what and filters what students get back.
- `hammer` gains `--api` / `PEDAGOG_API_URL` and a saved token; `--jobs-url` stays for
  operators and tests on internal endpoints.

### Auth

- **Login by email code:** `POST /auth/start {email}` mails a word phrase from the EFF
  short word list (e.g. `tiger-maple-river`); `POST /auth/finish {email, code}` returns
  a session.
  - Codes last 10 minutes and die after 5 wrong tries; a new code once a minute per
    email. Only known users get one; unknown emails get the same reply, so the
    endpoint doesn't reveal who's registered.
  - Codes are stored hashed.
- **Sessions:** a random token (`getrandom`), stored only as a hash with the user, kind
  and expiry. The web gets an HttpOnly, `Secure`, `SameSite=Lax` cookie (12 h); `hammer
  login` saves a 30-day token in `~/.config/pedagog/token` (mode 0600). `hammer logout`
  deletes the row.
- **Behind an interface** (`Authenticator`): the email code is the first implementation;
  UF SSO can be added later without touching the rest.
- **Mail:** SMTP via `lettre`, to a trusted relay without a login or with one:
  `PEDAGOG_SMTP_HOST`, `PEDAGOG_SMTP_PORT` (default by TLS mode: 25, 587, 465),
  `PEDAGOG_SMTP_TLS` (`none` | `starttls` | `tls`), optional `PEDAGOG_SMTP_USER` /
  `PEDAGOG_SMTP_PASSWORD` (from a Secret), `PEDAGOG_MAIL_FROM_NAME`,
  `PEDAGOG_MAIL_FROM_ADDRESS`, optional `PEDAGOG_MAIL_REPLY_TO`. On kind and in local dev,
  a Mailpit container catches every message and shows it in a web inbox (and kind-e2e
  reads codes from its API).
- **First admin:** `pedagog-api add-admin <email>` (run once, e.g. through `kubectl
  exec`); afterwards admins manage admins through the api.

### Roles and courses

> Superseded for roles, staff and rosters by [Step 3 revision](#step-3-revision-2026-10-19).

- **Admin:** creates courses, adds and removes their instructors, manages admins.
- **Instructor** (several per course): everything in their courses: assignments,
  dates, visibility, release, roster, results.
  - **Staff submissions:** an instructor can submit to their own assignments; graded
    like any other, accepted whether or not the assignment is open, marked `staff`,
    and left out of regrade notices and exports.
- **Student:** enrolled through the roster; submits to and sees their own work in
  their courses.
- **Courses:** an id slug (`cop3502-f26`: lowercase, used in URLs, commands and image
  names) and a display name.
- **Roster import:** a CSV of `email,name` (with a header), or Canvas's gradebook
  export (its `SIS Login ID` becomes `<login>@<PEDAGOG_LOGIN_DOMAIN>`, default
  `ufl.edu`). Re-importing adds new students and leaves existing ones; removing is
  explicit. A dropped student keeps their submissions and loses access.
- **Course-scoped assignments:** assignments, uploads, builds and submissions belong
  to a course, so the same id can exist in two courses. Images become
  `pedagog/assignments/<course>/<id>:<hash>`; `jobs`' internal routes become
  `/internal/courses/<course>/assignments/<id>/…`. A migration moves existing data
  into a course named `default`.

### Dates

Optional, in `assignment.yml` (each an RFC 3339 time with an offset):

```yaml
assignment:
  dates:
    open: "2026-09-01T14:00:00-04:00"      # submissions accepted from here
    due: "2026-09-08T23:59:00-04:00"       # after this, they're late
    late_due: "2026-09-10T23:59:00-04:00"  # after this, refused (default: due)

# or, open-ended:
assignment:
  dates:
    open: always
```

- **No dates anywhere: closed** ("linked-list isn't open yet: no dates set").
  `open: always` or `open` without `due` is open-ended.
- **The file's dates are defaults; courses edit them field by field**
  (`hammer dates set linked-list --course cop3502-f26 --due …`, `hammer dates clear`).
  Effective = the course's value for each field, else the file's.
- **Re-upload keeps the course's edits and shows the difference at once** (the upload
  reply comes before any build); `hammer upload --take-dates` adopts the file's (clears
  the course's date edits), superseding the queued build if there is one.
- **Acceptance:** from `open` until `late_due` (or `due`), decided by the dates in
  effect when a submission arrives. **Late** isn't stored: it's computed when shown,
  from the submission's time and the dates in effect for that student now, so moving a
  deadline (or, later, a student's extension) changes it. Penalties are later.
- Dates without an offset, and orders that don't make sense (`due` before `open`), are
  refused at upload and on `dates set`.

### Policy travels with the build

The image hash keeps covering the whole uploaded tree, `assignment.yml` included, so
an upload that changes only the file's dates or visibility still makes a build
attempt. Every layer comes from the cache (about 35 s on kind, including validation);
its policy (file dates, visibility defaults, submission rules) applies when that build
becomes current, like the rest of the upload.

- Course edits (`hammer dates set`, `visibility set`, `release`) never build: they're
  a database write and apply at once, so they're the way to move a deadline
  mid-semester. The upload reply says so when the file's dates differ.
- Considered and declined in review: leaving policy out of the image hash so file
  edits apply at once (rules for what counts as policy; a failed build couldn't hold
  them back), and a separate policy file (two files per assignment).

### What students see

- **Overrides**, in the file (defaults) and per course, for all tests or one test (by
  id, else node), each field (`score`, `output`) separately, with the values tests
  already use (`visible | hidden | after_end | after_publish`):

  ```yaml
  assignment:
    results:
      visibility: { output: after_end }       # every test
      tests:
        pop-front: { output: hidden }
        reverse: { score: after_publish }
  ```

  ```sh
  hammer visibility set linked-list --course cop3502-pe --score after_publish --output after_publish
  hammer visibility set linked-list --course cop3502-f26 --test pop-front --output visible
  ```

- **Resolution**, per test and field, first match wins: course for that test → course
  for all tests → file for that test → file for all tests → the test's own.
- **`after_end`** opens after the last accepted time (`late_due`, else `due`), or
  earlier on release; with no end, only on release. **`after_publish`** opens only on
  release. `hammer results release` / `--undo` (warns that students may already have
  seen them).
- **Inspecting it:** `hammer visibility show <assignment> --course <c> [--test <id>]
  [--verbose]` (and `GET …/{a}/visibility`) lists every test from the current
  build's validation results, with each field's resolved value, the layer that decided
  it, whether students see it now and why (e.g. "not yet: until the end
  (2026-09-10T23:59-04:00) or release"); `--verbose` shows every layer's setting for a
  test. Built on `core`'s `ResultsPolicy::explain` and `Visibility::shown_now`.
- **What's filtered** for students: scores, outputs, feedback and items per test as
  resolved; the summary counts only what's visible; traces, logs, problems and change
  summaries never. Staff see everything.

### Regrade notices

- Runs marked `notify` are delivered by email by `api` (a loop over unsent notices,
  woken by `LISTEN`, marking each sent): "linked-list was regraded: your score changed
  9 / 10 → 10 / 11", with a link and no test details, so visibility can't leak through
  it. Scores in the notice follow the same visibility (a hidden score isn't mailed).

### API surface

JSON over HTTPS; errors as `{"error": "…"}`.

| Who | Endpoints |
| --- | --- |
| anyone | `POST /auth/start`, `POST /auth/finish`, `POST /auth/logout`, `GET /me` |
| admin | `GET/POST/DELETE /admins`, `POST /courses`, `PUT/DELETE /courses/{c}/instructors/{email}` |
| staff | `POST /courses/{c}/roster` (CSV), `GET /courses/{c}/roster`, `DELETE /courses/{c}/students/{email}` |
| staff | `POST /courses/{c}/assignments` (`?force&take_dates&regrade=`), `GET /courses/{c}/assignments[/{a}]`, `GET …/{a}/builds/{b}` |
| staff | `PUT/DELETE …/{a}/dates`, `PUT/DELETE …/{a}/visibility`, `POST/DELETE …/{a}/release`, `GET …/{a}/submissions` |
| enrolled | `GET /courses`, `POST …/{a}/submissions`, `GET …/{a}/submissions` (own), `GET /submissions/{id}` (own, filtered) |

`hammer`: `login`, `logout`, `whoami`; `admins add|list|remove`; `courses
create|list|staff add|remove`; `roster import|list|drop`; `upload --course
[--take-dates]`; `submit --course`; `dates set|clear|show`; `visibility
set|clear|show`; `results release [--undo]`, `results list`.

### Data (`store`)

| Table | Columns (beyond ids and timestamps) |
| --- | --- |
| `users` | `email` (unique, lowercased), `name`, `is_admin` |
| `courses` | `id` (slug), `name` |
| `course_staff` | `course_id`, `user_id` |
| `enrollments` | `course_id`, `user_id`, `dropped_at` |
| `sessions` | `token_hash`, `user_id`, `kind` (`web` \| `cli`), `expires_at`, `last_used_at` |
| `login_codes` | `email`, `code_hash`, `attempts`, `expires_at` |

- `assignments` gains `course_id` (key: course and id), the file's policy defaults,
  and the course's `date_overrides`, `visibility_overrides` and `released_at`.
- `submissions.student_id` becomes a user; submissions gain `staff` (late is computed).
- `grading_runs` gains `notified_at`.

## Dependencies (approved)

- `lettre`: SMTP.
- `getrandom`: session tokens and login codes.
- `time`: RFC 3339 dates and Postgres timestamps.
- `csv`: roster files (Canvas quotes fields such as `"Gator, Albert"`).
- The EFF short word list (CC BY 3.0), embedded as a file, for login phrases.

## Alternatives considered

- **Auth only first:** nothing usable to show until the next step.
- **A minimal web UI in this milestone:** doubles it; the api comes first either way.
- **UF SSO now:** needs an application registered with UF IT; added later behind the
  same interface.
- **Passwords:** storage and resets, and the weakest option for exams.
- **JWTs:** no lookup per request, but revocation needs a deny list anyway and a leaked
  key forges anyone.
- **Admins from config (`PEDAGOG_ADMINS`):** changing admins is a redeploy.
- **Policy outside the image hash, or in its own file:** file dates would apply
  without a rebuild, but at the cost of rules for what counts as policy, or a second
  file per assignment; course edits already apply at once.
- **Intake moved into `api`, or one service:** moves freshly tested code, or puts the
  internet-facing process next to Kubernetes credentials.
- **Global assignment ids:** every course reusing an assignment would need a new id.
- **Dates only per course, or relative dates:** templated course repos couldn't carry
  them; relative dates still need overrides for holidays.
- **Course dates replacing the file's set:** simpler, but moving one deadline would
  drop the others.
- **An assignment-level full / score-only / hidden only:** fixed presets; per-test,
  per-course overrides make an assignment an exercise in one course and a practice
  exam in another.
- **Closed-by-default without `open: always`:** every always-open exercise would need
  a date.

## Open questions

- Which SMTP relay production uses (UF's, or a provider), and its credentials.
- The production hostname and its certificate (cert-manager with Let's Encrypt, or a
  UF certificate).
- Whether Canvas's gradebook export columns are stable across terms (checked with a
  real export in PR 3).

## Follow-ups (recorded, out of scope)

- Per-student date and visibility overrides (extensions, accommodations).
- Late penalties, attempt limits, which submission counts.
- UF SSO; per-course Kubernetes namespaces and quotas.
- Grade export (CSV) and artifact download.
- Rate limits per IP on `/auth/start`, beyond per email.

## Rollback

- Each PR is additive: new tables and columns (reversible migrations), a new service
  and routes, new `hammer` commands. The course-scoping migration's down step moves
  `default` data back to global ids (refused if another course exists).
- `jobs`' internal endpoints keep working for operators throughout.

## Step 3 revision (2026-10-19)

Decided after a review of #47, which had built admin-only courses, a single staff role
and import-only rosters. This replaces the roles, roster and admin parts above.

### Global roles

- `users.role`: `admin` | `staff` | `student` (default `student`), replacing `is_admin`.
- Staff and admins create courses. Only admins set global roles; `pedagog-api
  add-admin` stays for the first admin.
- Being a course instructor doesn't make someone staff, so a grad TA can teach a
  course without being able to create courses.
- Demoting a staff user keeps their course roles; they can't create new courses.
- The last admin can't be demoted.

### Course roles

- One role per person per course: `instructor`, `grader` or `student`. Adding an
  existing member with another role changes their role.

| | instructor | grader | student | admin |
| --- | --- | --- | --- | --- |
| view members | ✓ | ✓ | | ✓ |
| import, add, drop students | ✓ | | | ✓ |
| add graders and instructors | ✓ | | | ✓ |
| remove or demote another instructor | | | | ✓ |
| remove or demote yourself | ✓ (not the last) | ✓ | | |
| assignments, dates, visibility, release | ✓ | | | ✓ |
| all results | ✓ | ✓ | | ✓ |
| own results, submit | ✓ | ✓ | ✓ | ✓ |

- Admins act as an implicit instructor in every course; listings mark it `(admin)`
  and admin actions are logged with the admin's id.
- A course always has at least one active instructor.
- What graders see of hidden tests and unreleased results is decided in step 4.

### Courses

- `courses create ID NAME [--instructor EMAIL]...`: the creator becomes the instructor
  unless instructors are named (so an admin can set up a course without joining it).
- `courses list`: courses you have a role in. `--all [--search S] [--page N]`: admins
  only, every course, 50 per page.

### Members

- `hammer members list COURSE [--role R] [--dropped]`, `add COURSE EMAIL... [--role R]
  [--name N]`, `drop COURSE EMAIL...`, `import COURSE FILE [--sync] [--yes]`,
  `show COURSE QUERY`. Replaces `roster` and `courses staff`.
- Adding an unknown email creates the user.
- Imports enroll students only (the CSV and Canvas formats above). By default an
  import only adds; `--sync` also drops students missing from the file, after
  listing them and asking (or `--yes`). Staff are never touched by an import.
- Drops are soft for everyone: the member loses access, their history and
  submissions stay, listings mark them dropped, and adding them again restores them.
  Dropped instructors don't count toward the one-instructor rule.
- `members show` (instructors and graders): name, role, when and how they joined
  (`created`, `manual`, `import`), when dropped; only this course. Step 4 adds
  submissions and scores.

### Users

- Admin only: `hammer users list [--role R] [--search S]`, `users role EMAIL ROLE`
  (creates the user if unknown), `users show QUERY`, `users signout EMAIL`.
  Replaces `hammer admins`.
- `users show`: name, role, created, last login (and its kind), active sessions by
  kind, every course with its role (dropped ones dated).
- `users signout`: ends all of the user's sessions.

### Search in `show`

- `users show` and `members show` take part of an email or name (case-insensitive
  substring). An exact email match wins; one match shows the profile; several list
  up to 20 candidates (and say to narrow the search); none says so. `members show`
  searches only its course; dropped members match, marked dropped.

### API

| Who | Endpoints |
| --- | --- |
| admin | `GET /users?role&search`, `GET /users/{query}`, `PUT /users/{email}/role`, `DELETE /users/{email}/sessions`, `GET /courses?all&search&page` |
| staff, admin | `POST /courses` |
| course staff | `GET /courses/{c}`, `GET /courses/{c}/members?role&dropped`, `GET /courses/{c}/members/{query}` |
| instructor | `PUT /courses/{c}/members/{email}` (`{role, name}`), `DELETE /courses/{c}/members/{email}`, `POST /courses/{c}/members/import?sync&dry_run` (CSV) |

- `--sync` without `--yes` first calls `dry_run` to show who would be dropped.

### Data (migration `0007_roles`)

- `users`: `role` (from `is_admin`), `last_login_at`; `is_admin` dropped.
- `memberships`: `course_id`, `user_id`, `role`, `joined_at`, `added_by`, `source`
  (`created` | `manual` | `import`), `dropped_at`; one row per course and user.
  Filled from `course_staff` (instructors) and `enrollments` (students), which are
  dropped. The down step splits it back (graders become instructors).
- 0006 is not edited again (it's applied on kind and in main).

### Alternatives considered

- **A creator flag, or anyone creating courses:** a flag duplicates what `staff`
  means; open creation lets students take slugs.
- **Instructor becomes staff automatically:** grad TAs teaching a section could create
  courses.
- **A course owner:** another concept; "only admins remove other instructors" covers
  the lockout.
- **Configurable roles, or a separate TA role:** more to explain; three roles fit now.
- **Always-sync imports:** a partial file drops half the class.
- **Hard drops:** the course loses track of past submissions.
- **Separate `roster` and `staff` commands, or `admins` + `staff`:** overlapping
  commands; `staff` would mean two things.
- **Exact-email `show` with a separate search:** an extra step for the common case.

### Implementation (reworks #47, `feat/courses-roster`)

1. Migration 0007 and store: roles, memberships, last login, searches, paging.
2. api: permissions table as one `require(course, Need)` check; users, courses and
   members endpoints; sync dry runs; admin action logs.
3. hammer: `users`, `courses`, `members`; `whoami` shows the global role.
4. Docs: ARCHITECTURE (Roles, Courses & Enrollment), SETUP; kind demo.

## Step 4 addendum: results views (2026-10-19)

Decided before step 4. Each test has two visibilities, `score` and `output`, resolved
as above.

- **Graders see everything instructors see:** hidden tests, outputs, unreleased scores.
- **Staff, per test:** every test (never-shown ones too) with its result (pass / fail),
  score, and when the student sees its score and its output.
- **Staff, summary:** cumulative columns named for the visibility values; each adds
  that value's tests, so `hidden` covers every test. A marker shows the column the
  student sees now (it moves as the deadline passes and on release).

  ```text
              visible  after deadline  after release  hidden
  score       5/5      5/5             7/8            7/9
  passed      2/2      2/2             2/3            2/4
                       ^ student sees (deadline passed, not released)
  student is told: some tests are hidden until release
  ```

  `results list` has one row per student with the same columns.
- **Students:** tests whose score is visible, with result and score; output only where
  output is visible. Score-hidden tests aren't listed and their points aren't counted
  or mentioned; one line says "some tests are hidden until the deadline / until
  release" (both, if they open at different times). Pass / fail follows the score's
  visibility. Totals count only what's shown.
- **Rollout:** `api` and `jobs` share migrations and refuse a database with migrations
  they don't know, so they deploy together (same tag, one apply); a rollback across a
  migration reverts it first. Documented in SETUP.

## Step 4 split and 4a decisions (2026-10-19)

Step 4 ships as three PRs: **4a** `feat/course-assignments` (course-scoped jobs,
upload and builds through the api), **4b** `feat/api-submit` (submit, acceptance,
staff submissions, computed late), **4c** `feat/api-results` (results views, dates,
visibility, release, the upload date diff and `--take-dates`).

- **Keys:** assignments get a numeric `id`, with `(course_id, slug)` unique; uploads,
  builds, submissions and regrade batches reference the number. Migration 0008 moves
  existing assignments into course `default` (created only if there are any); its
  down step is refused once another course has assignments.
- **Names:** images `pedagog/assignments/<course>/<slug>:<hash>`; objects
  `assignments/<course>/<slug>/…` and `submissions/<course>/<slug>/…`; Job names
  include the course. Ids (assignments, recipes, OSes) tighten to start with a letter,
  end with a letter or digit, and stay within 40 characters (a trailing `-` made an
  invalid image reference).
- **jobs routes:** `POST /internal/courses/{c}/assignments`,
  `GET …/{c}/assignments/{slug}/builds/{b}`, `POST|GET …/{slug}/submissions`;
  `GET /internal/submissions/{id}` stays. The course must exist.
- **Policy on the build:** the file's `assignment:` policy (dates, visibility
  defaults) is stored on each build (`assignment_builds.policy`), so the current
  build's policy is the file's.
- **The api reads Postgres** through `store` (assignments, builds, results) and calls
  `jobs` only for work that needs Garage or Kubernetes (upload, submit), at
  `PEDAGOG_JOBS_URL`. `reqwest` (already in the workspace) becomes an api dependency.
- **api routes (4a):** `POST /courses/{c}/assignments?force&regrade` (instructors),
  `GET /courses/{c}/assignments` (members), `GET /courses/{c}/assignments/{a}` and
  `…/builds/{b}` (instructors and graders).
- **Choosing the course in hammer:** `-c/--course`, else the nearest `course.yml`
  walking up from the current directory (`course:` and an optional `api:`), else
  `hammer use <course>` (saved with the session). `members` moves to `-c` too.
- **Renaming courses:** instructors and admins change a course's name and slug
  (`PATCH /courses/{c}`, `hammer courses rename [--id NEW] [--name NAME]`). The slug
  stays the key and references follow it (`ON UPDATE CASCADE`); stored image refs and
  object keys keep the old slug and still work. `course_renames` remembers old slugs,
  so a repo still naming one gets "no course X (renamed to Y on …)".
- **Direct jobs mode** (`--jobs-url`, no login) stays for operators and kind-e2e (with
  `-c`) until step 5 moves e2e to the api.

## Implementation steps

1. **Schema and store** (`feat/accounts-store`): users, courses, staff, enrollments,
   sessions, login codes; course scoping (migration into `default`), `late`, `staff`,
   `notified_at`; `core` types for dates and visibility overrides (parsing,
   validation, resolution, field-by-field merge) with tests.
2. **The api service and auth** (`feat/api-auth`): config, the `Authenticator`
   interface and email codes, the mailer (SMTP / Mailpit), sessions, `add-admin`;
   deploy: Deployment, Service, Ingress, NetworkPolicy to `jobs`, Mailpit in the kind
   overlay; `hammer login|logout|whoami`.
3. **Courses, staff and roster** (`feat/courses-roster`): see
   [Step 3 revision](#step-3-revision-2026-10-19).
4. **Assignments through the api** (`feat/api-assignments`): course-scoped `jobs`
   (routes, image names), policy from the current build, upload with the date diff and
   `--take-dates`, submit with the acceptance window and `late`/`staff`, status
   filtered by visibility, dates / visibility / release commands, `visibility show`.
5. **Notices, e2e and docs** (`test/api-e2e`): regrade notices by email; kind-e2e
   through the api (login codes from Mailpit, roles, a closed and a late submission,
   visibility before and after release); ARCHITECTURE (Roles, Courses, Auth, Result
   Visibility), SETUP (api, mail, first admin).
