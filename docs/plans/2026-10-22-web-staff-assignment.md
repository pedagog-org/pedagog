# PR 4: the staff assignment pages

Step 4 of `2026-10-20-web-ui-implementation.md`. Design artboards: AssignmentStaff,
Assignments (instructor, grader), Upload (form, refused, progress), Build
(validation-failed, passed, build-failed), Home (staff), Flow 2 (upload, build).
Also carries the staff overview's raw-errors alert from
`2026-10-21-sanitized-errors.md` and the staff/admin chunk from ARCHITECTURE.

## Rationale

Staff can log in but see only assignment names; everything they do still goes through
`hammer`. This PR covers what an instructor does with an assignment: upload it, watch it
build and validate, see why it failed, and put an older build back. Graders get the same
pages, read-only.

## What the api is missing

- **Types:** the staff list, the staff assignment and the build are untyped
  `json!`. `core::api` has only student types.
- **Uploads:** only a `.tgz` body works. There's no multipart (folders from the browser)
  and no `.zip`. The uploader isn't recorded.
- **Builds:** only the last 10 come with the assignment. Builds have no uploaded-by and
  no per-phase timings (only `started_at` / `finished_at` for the whole attempt).
- **Logs:** a failed build's whole pod log sits in `assignment_builds.error` (Postgres)
  and is read back as its last 4000 bytes. Successful builds and validations keep no
  log, and there's no line count.
- **Make current:** missing. A build becomes current only by passing validation.
- **Events:** nothing fires when `current_build_id` changes.
- **Staff Home:** `/api/me/courses` fills `items` for students only.

## Changes

### Store (migration 0017)

- `assignment_builds` gains:
  - `uploaded_by` (user, nullable: older rows show "—");
  - `rebuild_of` (build, nullable);
  - `built_at` (build phase ended), `validation_started_at`;
  - `build_log_lines`, `validation_log_lines` (nullable ints).
- `error` keeps only the failure's short reason ("exit status 1", "timed out"); the log
  moves to Garage. Older rows (whose `error` is the whole pod log) keep only its last
  non-empty line, and their build page says "no log kept".
- A trigger on `assignments` sends `{kind:"current", assignment, build}` when
  `current_build_id` changes. It uses the same channel as the build events.

### Jobs

- **Intake:**
  - accepts `multipart/form-data` (one part per file named by its relative path, as
    submissions are), `application/gzip` and `application/zip`;
  - every input is unpacked to a tree, a single top-level folder is stripped, and the
    tree is repacked with `archive::pack`, so the same files always give the same
    `tree_hash` whatever the format;
  - records `uploaded_by` from the api.
- **Logs:** each phase's pod log is streamed into `logs/builds/{id}/{build,validation}.log.gz`
  whether it passed or failed, with its line count recorded. The nightly cleanup deletes
  logs older than 180 days.
- **Make current** (`POST /internal/…/builds/{id}/current`, with the optional regrade
  policy):
  - **Image present:** one transaction sets `current_build_id`, queues regrades if
    asked, and the trigger announces it. Dates and visibility don't change
    (`take_dates` off).
  - **Image deleted:** a new build attempt with `rebuild_of = id`. It uses the same
    upload (the same stored archive in Garage, not a copy) and the caller as uploader.
    It builds and validates through the normal path and becomes current if usable.
  - **Eligible:** build succeeded, and validation passed or skipped. It isn't the
    current build already.
  - **A newer build in flight:** the make-current still happens. If the newer one
    passes later, it becomes current as usual.
  - **Image deleted and a build in flight:** the rebuild starts at once and cancels the
    build in flight (superseded, as a new upload would). Its regrade request goes with
    it, so the instructor uploads that version again if they still want it.

### Api (typed, in `core::api`)

- **`GET /courses/{c}/assignments`, staff:**
  - each assignment with dates and `state` (the student enum);
  - the current build, and the latest build with its statuses;
  - submitted count of active students, and the couldn't-be-graded count.
- **`GET …/{slug}`, staff:** the summary cards' data:
  - the current build and since when;
  - submitted of active students, couldn't be graded;
  - dates;
  - released or not;
  - `students_see_raw_errors` (the current build's results lack `sanitizes-errors`);
  - the latest 10 builds (number, uploaded at and by, statuses, current, image deleted,
    `rebuild_of`, `can_make_current`).
- **`GET …/{slug}/builds`:** all builds, newest first, paginated
  (`?before={id}&limit=`).
- **`GET …/builds/{id}`:**
  - typed;
  - adds uploaded at / by, the build and validation durations, line counts, the last
    200 lines of each log, mismatches, warnings, and the reference results.
- **`GET …/builds/{id}/log?phase=build|validation`:** the whole log as `text/plain`,
  streamed and gunzipped. Staff only.
- **`POST …/builds/{id}/current`:**
  - instructors only;
  - body `{regrade: null | "never" | "changed" | "always"}`;
  - replies "now current" or "rebuilding as build 16".
- **Upload:**
  - the Content-Type is passed through (as submit does) instead of being forced to
    gzip;
  - the caller is sent as the uploader;
  - the reply is typed, including `date_diff`.
- **`/api/me/courses`, staff items:**
  - latest build failed and nothing newer in flight (instructors);
  - N submissions couldn't be graded;
  - due within 7 days with "431 of 640 submitted";
  - build N building / validating, regrading N of M.

### Web

- **Routes:**
  - `/courses/{c}/assignments/{a}` (the staff view when the viewer is staff);
  - `…/upload`, `…/builds/{id}`;
  - `/courses/{c}/upload` for a new assignment.
- **Assignments (staff):**
  - table: name, dates badge, current build, latest build status, submitted;
  - "Upload assignment" for instructors.
- **Overview:**
  - header with state and release badges, "Upload new version" (instructors);
  - only the Overview tab: Dates, Visibility and Results come with their PRs, and the
    Release and Email buttons too;
  - alerts:
    - the latest build failed, and the current one is still in use;
    - students see raw errors (the current build's helper doesn't clean them up);
  - the four summary cards;
  - the builds table with "Make current…" and "Show all builds".
- **Make current dialog:**
  - the regrade checkbox and "Tell students";
  - the in-flight note ("build 15 is still validating; if it passes it becomes current
    after this");
  - the rebuild note when the image is gone ("build 12's image was cleaned up; making
    it current rebuilds it first"), also marked in the builds table, plus "this cancels
    build 15, which is still validating" when one is in flight.
- **Upload:**
  - form:
    - a drop zone for a folder or a `.tar.gz` / `.zip`, with name, file count, size and
      "assignment.yml found";
    - regrade and "Tell students";
    - "Use the file's dates";
    - no "Rebuild even if unchanged": every upload builds;
  - refused: the problems listed;
  - progress, live from build events: Built → Validating → Passed / failed, plus the
    upload's warnings and the dates-differ table;
  - sent with the PR 3 XHR multipart sender, with progress.
- **Build:**
  - badges (build, validation, current);
  - key/values: uploaded, by, image, took ("1m 52s build · 41s validation");
  - build failed: the log's last lines, Copy, "Show all" (fetches the full log; over
    5 MB it offers a download instead);
  - validation failed: mismatches;
  - reference results: each test "as expected / not as expected", with the existing
    results component in staff mode;
  - validation warnings, including the no-sanitize one;
  - live while running;
  - "Make current…" for instructors.
- **Staff Home:** the four item kinds above, per course.
- **Code split:** the staff routes become `#[lazy_route]` and wasm-split-cli runs as a
  Trunk post-build hook. That's a new dev tool, installed in CI and checked by
  `cargo xtask doctor`.
  - **Time-box:** about half a day; if it doesn't work with Trunk, I come back to you
    before shipping without a split.
  - **Budget:** the main chunk at most 450 KB; the staff chunk's size reported.
- **Cache:** the list, overview and builds `Revalidate`. Build, current and run events
  invalidate them; upload and make current invalidate the assignment and its list.

### Tests

- **Store:** make current (transaction, regrades queued, trigger fires); the eligibility
  rules; builds pagination; uploaded-by.
- **Jobs:**
  - intake of multipart / gzip / zip gives one `tree_hash`;
  - top-level folder stripping;
  - logs written with line counts for both phases;
  - make current with the image present and deleted (rebuild row, same upload);
  - a rebuild with a build in flight cancels (supersedes) it and starts at once.
- **Api:** typed shapes; 403 for graders on upload and make current; the log endpoint
  streams; staff Home items.
- **Native web:** overview alerts from data; durations; the make-current dialog's notes;
  the upload source summary.
- **Browser components:** the overview, upload views and build views from fixed data.
- **Kind e2e:**
  - `staff_web_upload`: upload a folder in the browser, watch it validate, and open the
    build page with its log;
  - `make_current`: make the earlier build current, then the overview shows it.

## Alternatives considered

- **Code split:**
  - **no split:** simplest, but students download the staff code and the budget likely
    breaks now;
  - **cargo-leptos:** native split, but replaces the whole web build;
  - **a separate staff app:** duplicated shared code and full page loads.
- **Re-running the old build in place:** fewer rows, but history is lost and "finished
  stays finished" breaks.
- **Make current with a build in flight:**
  - refusing it blocks a revert while a fix builds;
  - superseding the in-flight build would make the instructor upload it again (chosen
    only when a rebuild is needed);
  - for the rebuild: refusing until the in-flight build ends, or queuing the rebuild
    behind it with "newer wins" (a queued state and a check in `finish_validation`).
- **Packing in the browser:** about 100–150 KB more wasm (rejected in the UI plan).
- **Logs in Postgres:** they bloat the database (rejected in the UI plan).
- **All tabs now, disabled:** the final layout early, but dead controls.

## Open questions

- **Graders:** see every staff page read-only (as `Need::See` today). Instructors alone
  upload and make current.

## Rollback

Revert the PR. Migration 0017 has a down migration (drops the new columns and the
trigger). Cutting old rows' `error` to its last line is lossy: the full text doesn't come
back on the way down. Logs already in Garage are left for cleanup. Uploads stay readable both ways,
since the stored archive format doesn't change.

## Steps (commits)

1. `feat(store)`: migration 0017 (uploaded_by, rebuild_of, phase times, line counts,
   the current trigger); make current; builds pagination.
2. `feat(jobs)`: multipart / zip intake normalised to one pack; uploader recorded.
3. `feat(jobs)`: logs to Garage per phase with line counts; their 180-day cleanup.
4. `feat(jobs)`: make current; rebuild when the image is gone, cancelling a build in
   flight.
5. `feat(api)`: typed staff list, assignment, build, builds; log endpoint; make current;
   upload passthrough; staff Home items.
6. `build(web)`: wasm-split spike, lazy staff routes, the per-chunk budget.
7. `feat(web)`: staff assignments list and overview (cards, alerts, builds table, make
   current dialog).
8. `feat(web)`: upload (form, refused, live progress).
9. `feat(web)`: build page (log, results, live).
10. `feat(web)`: staff Home items.
11. `test(e2e)`: the staff upload and make current flows in the browser.
12. `docs`: ARCHITECTURE.md (staff api shapes, logs in Garage, make current, the chunk).
