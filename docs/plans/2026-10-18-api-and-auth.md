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
  `PEDAGOG_SMTP_PASSWORD` (from a Secret), `PEDAGOG_MAIL_FROM`. On kind and in local dev,
  a Mailpit container catches every message and shows it in a web inbox (and kind-e2e
  reads codes from its API).
- **First admin:** `pedagog-api add-admin <email>` (run once, e.g. through `kubectl
  exec`); afterwards admins manage admins through the api.

### Roles and courses

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
- **Acceptance:** from `open` until `late_due` (or `due`); submissions after `due` are
  flagged `late`. Penalties are later.
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
- `submissions.student_id` becomes a user; submissions gain `late` and `staff`.
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
- UF SSO; the Grader/TA role; per-course Kubernetes namespaces and quotas.
- Grade export (CSV) and artifact download.
- Rate limits per IP on `/auth/start`, beyond per email.

## Rollback

- Each PR is additive: new tables and columns (reversible migrations), a new service
  and routes, new `hammer` commands. The course-scoping migration's down step moves
  `default` data back to global ids (refused if another course exists).
- `jobs`' internal endpoints keep working for operators throughout.

## Implementation steps

1. **Schema and store** (`feat/accounts-store`): users, courses, staff, enrollments,
   sessions, login codes; course scoping (migration into `default`), `late`, `staff`,
   `notified_at`; `core` types for dates and visibility overrides (parsing,
   validation, resolution, field-by-field merge) with tests.
2. **The api service and auth** (`feat/api-auth`): config, the `Authenticator`
   interface and email codes, the mailer (SMTP / Mailpit), sessions, `add-admin`;
   deploy: Deployment, Service, Ingress, NetworkPolicy to `jobs`, Mailpit in the kind
   overlay; `hammer login|logout|whoami`.
3. **Courses, staff and roster** (`feat/courses-roster`): admin and staff endpoints, CSV
   and Canvas import; `hammer admins|courses|roster`.
4. **Assignments through the api** (`feat/api-assignments`): course-scoped `jobs`
   (routes, image names), policy from the current build, upload with the date diff and
   `--take-dates`, submit with the acceptance window and `late`/`staff`, status
   filtered by visibility, dates / visibility / release commands.
5. **Notices, e2e and docs** (`test/api-e2e`): regrade notices by email; kind-e2e
   through the api (login codes from Mailpit, roles, a closed and a late submission,
   visibility before and after release); ARCHITECTURE (Roles, Courses, Auth, Result
   Visibility), SETUP (api, mail, first admin).
