# Web UI Implementation

Follows `2026-10-20-web-ui.md` (CSR with Leptos and trunk; the skeleton is merged).
The visual design is the Claude Design canvas "Pedagog Web UI"
(https://claude.ai/artifact/JmS1XRZtxER6hEfNSN6mXH): tokens and components in its
`pedagog.css`, screens per the brief (`docs/design/web-ui-brief.md`), and a few
features beyond it (below). Decided in a grilling session on 2026-10-20.

## Rationale

The platform is usable only through `hammer`. The design is done; this plan settles
how to build it well (serving, live updates, caching, security, testing, budgets),
which design-only features come with it, and the order of the work.

## Decisions

### Platform

- **The api under `/api`**, no aliases (nothing in production uses the old paths).
  hammer keeps host-only URLs (`--api https://host`) and adds `/api` itself, so saved
  sessions keep working.
- **The api serves the UI** (`tower-http` `ServeDir`, new dependency): `/api/*` is the
  REST api (an unknown `/api` path stays a JSON 404); anything else is a file from
  `PEDAGOG_WEB_DIR` or `index.html` (deep links reload). Hashed assets:
  `Cache-Control: max-age=31536000, immutable`; `index.html`: `no-cache`. Without
  `PEDAGOG_WEB_DIR` the api serves only `/api`. The api image builds the UI with trunk.
- **Sessions:** the HttpOnly SameSite=Lax cookie stays; cookie-authenticated
  POST/PUT/PATCH/DELETE must carry `X-Pedagog: web` (403 otherwise); bearer tokens
  (hammer) are unaffected. A 401 sends the UI to `/login?next=<page>`.
- **Security headers** on UI responses: a strict CSP (`default-src 'self'`;
  `script-src 'self' 'wasm-unsafe-eval' 'sha256-<theme script>'`; `style-src`,
  `font-src`, `connect-src 'self'`; `img-src 'self' data:`; `frame-ancestors 'none'`;
  `base-uri 'none'`; `form-action 'self'`), `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: same-origin`; HSTS at Traefik.
- **Live updates: SSE with a polling fallback.**
  - Store functions that change a run's or build's status `NOTIFY pedagog_events`
    with a small payload; the api `LISTEN`s once (sqlx `PgListener`) and fans out
    through a tokio broadcast channel.
  - One stream per open assignment page:
    `GET /api/courses/{c}/assignments/{a}/events`, opened with the page's own
    `require()`. Events are filtered per viewer: a student gets only their own
    submissions' runs (submission number, status, queue position); staff get every
    run (with the student and score) and the assignment's builds. No raw ids beyond
    what the page already shows.
  - Keep-alive every 15 s, which also re-checks the session (logout, an ended
    session or a role change closes the stream).
  - The browser uses `EventSource` (gloo-net `eventsource`); if the stream is down it
    polls (1 s for 30 s, then 3 s; paused while hidden).
  - Later: streaming per-test results as they come.
- **Typed views:** each endpoint's reply moves to a struct in `core::view`, used by the
  api to serialize and by the UI to deserialize; the JSON doesn't change. Done
  endpoint by endpoint with each slice.
- **Browser errors:** the UI posts unexpected errors and panics to
  `POST /api/client-errors` (signed in, rate-limited, size-capped, no bodies); the api
  logs them as warnings (`source = "web"`, path, version), so they reach GlitchTip.
- **Scripts become Rust:** `scripts/kind-e2e.sh` and `kind-lib.sh` become the
  `pedagog-e2e` test crate (each check a `#[test]`, `#[ignore]`d unless run against
  kind; a `Harness` for personas with Mailpit logins, kube, sqlx); the helpers
  (kind-login, kind-as, image loads, rebuild, add-admin) become `cargo xtask …`
  subcommands; just recipes become one-liners calling it; the pre-commit hook calls
  `cargo xtask precommit`. No new dependencies for this.
- **CI:** every push: fmt, clippy (native and wasm32), tests, sqlx check, wasm
  component tests, the web size budget. A separate `e2e.yml` runs the kind e2e
  (including browser flows) nightly on main and on PRs labelled `e2e`.

### Web UI

- **Styling:** `pedagog.css` as delivered, minus canvas-only rules (`dc-import`,
  `.is-hover`/`.is-focus` demo states, replay keyframes, unchosen wordmark variants);
  components map 1:1 to its classes. Light on `:root`, dark on `.dark`.
- **Theme:** System by default, then Light / Dark from the user menu, saved per browser
  (localStorage); an inline script in `index.html` (allowed by its CSP hash) sets
  `.dark` before first paint; System follows OS changes live.
- **Fonts:** IBM Plex self-hosted (OFL), latin subset woff2: Sans 400/500/600/700,
  Mono 400/500, `font-display: swap`, the two most used preloaded. The wordmark's
  first-load shuffle faces (Abril Fatface, Bricolage Grotesque 800, Caveat 700,
  Silkscreen, Space Mono 700, Fraunces italic 600, EB Garamond italic 600) subset to
  the letters of "pedagog" and loaded only on the login page.
- **Logo:** mark B+C 2 (a P of test bars with a cursor; SVG from the canvas's
  `Mark.dc.html`, option `u`), wordmark lowercase "pedagog" in Plex Sans 700. First
  load (log in, app start): bars build in and the font shuffle; top bar: the mark
  builds in, the word static; loading: looping bars, skeletons that grow like the
  bars. Every animation has a reduced-motion fallback.
- **Structure:** one crate, `crates/web/src/`: `api` (typed client over `core::view`,
  the header, 401 handling), `cache`, `events`, `time`, `shell/` (top bar, nav,
  theme), `components/` (one per pedagog.css component), `pages/` (one per route).
- **Routes** mirror the api without `/api`: `/login`, `/`, `/courses/{c}`,
  `/courses/{c}/assignments/{a}` with `results`, `results/{email}`, `dates`,
  `visibility`, `builds/{id}`, `submissions/{id}`, `upload`; `/courses/{c}/members`,
  `/courses/{c}/settings`, `/courses/new`; `/admin/users`, `/admin/users/{email}`,
  `/admin/courses`. Filters, search and the selected submission in the query string.
  No course-level "Results" nav item for now (a gradebook comes later with export).
- **Caching by kind:** you, your courses, a course, its assignments and members are
  cached for the session and refetched when a change you make touches them (or after
  5 minutes); results, submissions and dates show the cached copy at once and
  refetch in the background; live statuses come from events.
- **Results table:** every student who submitted in one request; sort, search and
  toggles in the browser; only visible rows rendered (windowing).
- **Time zones:** each course has a time zone (`PEDAGOG_DEFAULT_TIME_ZONE` by default,
  set at creation, editable in Settings). Dates are entered and shown in it with the
  zone named, plus the viewer's local time on hover when it differs. The UI uses the
  browser's `Intl` (js-sys/web-sys as direct dependencies; no tz data in the WASM);
  the api uses `jiff` (new dependency) with the system's tzdata for emails and
  sentences.
- **Budget:** first load (wasm, js, css, Plex) at most 400 KB gzipped, checked in CI;
  release profile for the web crate: opt-level z, LTO, wasm-opt -Oz, panic=abort.
- **Support:** the last two versions of Chrome, Edge, Firefox and Safari (desktop and
  mobile); English only, copy kept per screen; locale-aware number and date formatting.
- **Tests:** native unit tests for logic (formatters, cache rules, trackers);
  component tests in headless Chrome with `wasm-bindgen-test` (dev-dependency;
  wasm-bindgen-cli and chromedriver in CI); the three design flows as browser e2e
  tests in `pedagog-e2e` via WebDriver (`fantoccini`, dev-dependency), each page
  scanned with axe-core (vendored `axe.min.js`, injected only in tests; serious or
  critical issues fail); a short keyboard/screen-reader checklist per PR.

### Features beyond the brief (all in scope)

- **Make an old build current:** instructors make any build that passed or skipped
  validation current again, with the upload's regrade and tell-students options;
  dates and visibility don't change. If cleanup deleted its image, jobs rebuilds it
  from the stored upload, validates, then makes it current.
- **Archived courses:** `archived_at`; instructors and admins archive and unarchive in
  Settings; archived = every change refused ("cop3502-s26 is archived"), every read
  works for its members; Past courses on Home, in the switcher and the admin filter.
- **Per-student dates:** per assignment and student, own open / due / late due with
  who, when and a note; instructors (and admins) set them one at a time or by CSV
  import (with a preview), graders see them. A student's effective dates (own, else
  the course's, else the file's) drive submit refusals, lateness, the dates status and
  `after_end` visibility for that student; the staff results table's columns and ★ use
  each row's own dates (rows marked "own dates"). Release still shows everything to
  everyone.
- **HTML emails:** multipart (plain text + HTML) from `askama` templates (new
  dependency) kept as files; subjects with the assignment's name, greeting by
  preferred else given name, regrade "7/10 → 9/10" and what else changed, links into
  the UI (`PEDAGOG_PUBLIC_URL`).
- **Names:** users get `given_name`, `family_name` (both optional) and
  `preferred_name`. Shown as preferred-or-given then family; greeted by
  preferred-or-given; staff tables sort by family then given. Only the person sets
  their preferred name ("Your name…" in the user menu); instructors and admins edit
  given and family names; imports set given and family only (Canvas's "Last, First"
  splits on the comma) and never touch a preferred name. Existing names split on the
  last space.
- **Smaller api gaps:** dates "no deadline" overrides (the course can clear the file's
  due / late due; clearing due clears late due); builds' uploaded by / at, build and
  validation durations, total log lines and the full log (Garage,
  `logs/builds/{id}/{build,validation}.log.gz`, served to staff, deleted after 180 days
  by the nightly cleanup); `released_by`; the import preview's would-add / restore /
  already / staff counts; the assignment's required files and size limit for students;
  the shown score per submission in a student's history; admin lists (users: last
  login, course count, role totals; a user's sessions as rows; courses: instructors,
  counts, created, status); home summaries, assignment-list states and counts, and the
  overview's summary cards (brief §11).

## Alternatives considered

- **Static container behind Traefik** for the UI: independent deploys, one more
  Deployment; the api serving it keeps one service and one origin.
- **Temporary route aliases** during the `/api` move: collide with the UI's paths.
- **UI-side copies of api types:** drift caught only at runtime.
- **Fetch per page / cache until changed:** extra requests on every navigation, or
  other people's changes hidden until refresh.
- **Polling only, a per-user event stream, or `?watch=` ids:** chosen per-assignment
  streams filtered per viewer, so clients never name arbitrary runs.
- **SameSite only, or CSRF tokens:** one layer, or state for no gain over the header.
- **Packing uploads in the browser:** ~100–150 KB more WASM.
- **Google Fonts:** tells a third party who uses the platform.
- **Server-paged results table:** sorting and search wait on the server.
- **Viewer-local time zones:** a travelling instructor enters deadlines wrongly.
- **Plain-Rust or MJML emails:** askama chosen to edit templates as files.
- **Postgres for full logs:** bloats the database.
- **Keeping the shell scripts:** less testable; checks aren't reported individually.

## Open questions

- Runner time and cost of the nightly kind e2e (Kaniko builds on GitHub runners);
  revisit if it's too slow.
- `jiff` next to the `time` crate in the api: move other time handling to jiff later,
  or keep both.
- The archived course's effect on regrade runs already queued (let them finish).

## Rollback

Each PR is independent and revertible. Migrations (names, archive, per-student dates,
course time zone, released_by, build metadata) each have down migrations; the names
split is lossy only for names whose split was wrong (the original is kept in a column
until the next release).

## Implementation steps (one PR each)

1. **Platform** (`feat/platform-web`): the `/api` move and hammer; the api serving the
   UI (cache headers, CSP and security headers); the `X-Pedagog` check; SSE
   (`NOTIFY` in store functions, `PgListener` fan-out, per-assignment streams with
   per-viewer filtering); typed views scaffolding in `core::view`;
   `/api/client-errors`; xtask and `pedagog-e2e` replacing the scripts; `e2e.yml`.
2. **Web foundations:** pedagog.css, fonts, logo and animations, shell (top bar, nav,
   switcher, user menu, theme), login, the api client, cache, events, time helpers,
   the size budget in CI, wasm component tests.
3. **Student:** home, assignments, assignment (submit, receipt, live grading, results,
   history), required files and size limit, shown score per submission.
4. **Staff assignment:** overview (summary cards), upload (the api accepts multipart
   and packs it), build page (who, when, durations, full log), make current.
5. **Results:** the table and a student's results, activate, submit for a student.
6. **Members and names:** the names migration and split, edit names, "Your name…",
   import preview counts.
7. **Dates:** no-deadline overrides, per-student dates (form and CSV), effective dates
   everywhere.
8. **Visibility, release and notices:** with `released_by`.
9. **Settings, archive and create course:** rename, archive, course time zone.
10. **Admin:** users, a user, courses, with the new columns.
11. **HTML emails:** askama templates for the five emails.
