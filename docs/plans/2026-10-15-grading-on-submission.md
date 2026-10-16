# Grading on Submission

The next milestone after assignment builds: a student submits code, `jobs` grades it in the
assignment's image, and the result is stored. Decisions here come from a design review on
2026-10-15/16.

## Rationale

- Assignments now build and validate end to end (`just kind-e2e`), but nothing runs student
  code: the image, the pytest launcher, the results model and the network policies exist only
  for the reference solution.
- Grading is the platform's core job; sessions, the api and the web UI all build on it.
- Findings that shape this plan:
  - Validation already has the run shape grading needs (image, `fetch-context`,
    `run-tests --code <dir>`, presigned results PUT, pod security, egress policies).
  - Network policies apply a few seconds after a pod starts (seen with kindnet); harmless
    for the reference solution, not for student code.
  - The only platform kind is `interactive` (code-server). ARCHITECTURE.md already
    describes a submission-only mode with no session, but the schema doesn't have one.
  - Validation pods are created as soon as a build finishes. With a class of students
    submitting at once, Jobs created immediately would sit `Pending` and burn their
    deadlines waiting for a node.

## Design

### Platforms own the submission source

- **Rename `interactive` → `workspace`** (plain rename, no alias; nothing is deployed).
- **New `upload` platform kind:** no code-server, no ports, no entrypoint (`entrypoint`
  becomes optional; a platform without one has no session). Its build hook has no steps
  today.
- **One source per assignment, no mixing:**
  - `kind: workspace` takes submissions only from the student's session workspace;
    `params.submission_dir` (default `.`, relative to `/home/student/workspace`) picks the
    folder. Sessions don't exist yet, so these submissions are **refused** for now ("workspace
    submissions arrive with sessions").
  - `kind: upload` takes submissions only as uploads (files or an archive).
- So this milestone is exercised through `upload` assignments. Recipes gets
  `examples/linked-list-python-upload` (same tests and solution, `kind: upload`).

### The `submission:` block (accept rules)

Applies to every submission, whatever the source.

```yaml
environment:
  platform:
    kind: upload
submission:
  include: ["*.py"]                 # optional; absent = everything
  exclude: ["**/__pycache__/**"]    # optional
  unmatched: drop                   # drop (default, noted on the receipt) | refuse
  archives: unpack                  # unpack (default) | keep
  strip: auto                       # auto (default): drop one wrapping folder | false
  required: [linked_list.py]        # refused if missing
  max_size: 5MiB                    # optional, capped by jobs' ceiling
```

- **Normalization order** (each step is reported on the receipt):
  1. a single uploaded `.zip` or `.tar.gz` is unpacked when `archives: unpack`; with `keep`
     it's stored as a file (nested archives are never unpacked);
  2. junk is always dropped: `__MACOSX/`, `.DS_Store`, `Thumbs.db`;
  3. `strip: auto` drops the top folder when every entry sits under one folder;
  4. `include`/`exclude` (globs on the relative path); unmatched files are dropped and
     listed, or the submission is refused with the list (`unmatched: refuse`);
  5. `required` paths must exist;
  6. the total unpacked size must fit `max_size`, which can't exceed
     `PEDAGOG_MAX_SUBMISSION_BYTES` (default 50 MiB).
- Intake's existing safety checks apply first: no absolute paths, `..`, links or devices;
  the archive and file count limits.
- Uploads (`hammer upload`) reject a `submission:` block that is inconsistent (e.g. a
  `required` path excluded by `include`), and validate `submission_dir` as a relative path.

### Intake

- `POST /internal/assignments/<id>/submissions?student=<id>`, multipart:
  - several file parts, each named with its relative path, or
  - one archive part (`.zip` or `.tar.gz`).
- The student is an **opaque id** string until auth exists.
- **Refused** (4xx with the reasons) when: the assignment has no usable build; its platform
  is `workspace`; or the rules above refuse it.
- **Accepted:** the normalized tree is stored as `submissions/<assignment>/<tree_hash>.tgz`
  (deduped storage), a submission row pins the **current build**, and a grading run is
  queued. Identical resubmissions are still new submissions and are graded again.
- **Response (receipt):** `{ submission_id, run_id, build_id, files, ignored, stripped,
  queue_position }`.
- **`hammer submit <dir|files…> --assignment <id> --student <id> [--jobs-url] [--wait]`**:
  sends the files as given (a single archive is sent as an archive) and prints the receipt;
  with `--wait`, polls the status and prints the score and failing tests.

### Grading runs

- A Job from the pinned build's image, in `pedagog-builds`, labeled
  `pedagog.io/role=grade` and `pedagog.io/network=<mode>`:
  - **initContainer `await-egress`** (jobs image, `pedagog-jobs await-egress`): connects to
    `kubernetes.default.svc:443`, which no policy ever allows, until the connection fails;
    gives up (pod fails) after 30 s. Added to validation pods too.
  - **initContainer `fetch-context`:** the submission into an emptyDir at
    `/pedagog/submission` (same safety checks as intake).
  - **main:** `/pedagog/bin/run-tests --code /pedagog/submission --isolation <mode>`, results
    to a presigned PUT at `results/grading/<run_id>/results.yaml`.
  - Validation's pod security, network policies (selected by `pedagog.io/network`, so both
    roles match), limits and deadline (`PEDAGOG_GRADING_DEADLINE`, default 600 s).
- The code under test is **exactly the submission**; no starter overlay.
- **Statuses:** `queued → grading → graded | error`.
  - `graded`: results parse and pass `problems()`; score and total come from the summary;
    full results stored. Failing tests are still `graded`.
  - `error`: missing, unparseable or inconsistent results, a pod crash, or a timeout; the
    reasons and the pod's log tail are stored; the raw results file stays in Garage.
- **Isolation:** `in-process` is allowed, but uploads warn that graded student code can read
  the tests and alter its results. Each run records its isolation mode.
- **Reconcile at startup:** `grading` runs with an active Job keep waiting; Job gone →
  `error` ("interrupted"); `queued` runs stay queued.

### Admission

- **Postgres FIFO queue** (option A, Kueue-ready): runs are rows in `queued`, ordered by
  `(priority, queued_at)`; a run's position is visible in its status.
- **Submissions before regrades:** submission runs get priority 0, regrade runs 1, so a
  regrade batch drains behind new submissions (a Kueue `WorkloadPriorityClass` later).
- **Capacity gate:** a release loop (every 2 s) in each `jobs` replica:
  1. takes a transaction-level advisory lock, so only one replica releases at a time;
  2. counts grading pods cluster-wide by label that are `Pending`, plus released runs
     whose pod doesn't exist yet;
  3. if zero (and under the optional ceiling `PEDAGOG_GRADING_MAX_RUNNING`, unset by
     default), claims the next run (`FOR UPDATE SKIP LOCKED`), marks it `grading` and creates
     its Job.
- The pace follows real capacity: node loss, more nodes or other workloads change how fast
  pods schedule, and the gate adapts without configuration.
- **Jobs are created at release**, so a run's deadline starts when it's admitted, not when
  it's queued.
- Behind a small `Admission` interface (enqueue, position, release), so Kueue can replace it
  when multiple courses need fair sharing, priorities and preemption.

### Regrades

- **`hammer upload --regrade [--notify never|changed|always]`** (notify default `never`): the
  request is stored on the build.
- When that build becomes current (validation passed or skipped, in the same transaction),
  each student's **latest submission** gets a **new run** against it, in one regrade batch.
  Earlier runs stay as history; the latest run is the one shown.
- A build that fails validation or is superseded never regrades.
- Each regrade run stores a **change summary** against the submission's previous graded
  run: score before and after, tests added, removed, or with a changed outcome or score.
- **Notification intent** is stored per run (`never`: no; `changed`: if the summary isn't
  empty; `always`: yes). Delivery comes with the api; the instructor may be testing, so
  nothing is sent unless they opted in.

### Status

- `GET /internal/submissions/<id>`: submission (files, receipt, pinned build) and its runs
  (status, queue position, score/total, problems, error, log tail, change summary, results).
- `GET /internal/assignments/<id>/submissions?student=<id>`: a student's submissions,
  latest first.
- Internal callers see everything; what students see (hidden tests, feedback) is filtered by
  the api later.

### Data (`store`, migration `0004_submissions`, reversible)

| Table | Columns (beyond ids and timestamps) |
| --- | --- |
| `submissions` | `assignment_id`, `student_id`, `build_id` (pinned), `source` (`upload` \| `workspace`), `tree_hash`, `object_key`, `size_bytes`, `receipt` (JSONB) |
| `grading_runs` | `submission_id`, `build_id`, `reason` (`submission` \| `regrade`), `regrade_batch_id`, `priority`, `status`, `isolation`, `job_name`, `queued_at`, `started_at`, `finished_at`, `score`, `total`, `results` (JSONB), `problems` (TEXT[]), `error`, `log_tail`, `changes` (JSONB), `notify` |
| `regrade_batches` | `assignment_id`, `build_id`, `notify`, `runs` |

- `assignment_builds` gains `regrade_notify` (null = no regrade requested).
- Indexes: queued runs by `(priority, queued_at)`; submissions by
  `(assignment_id, student_id, created_at)`.

## Dependencies

All approved in review:

- `zip`: unpack `.zip` submissions in `jobs`.
- `globset`: `include`/`exclude` matching in `core` (rules are validated at upload) and
  `jobs`.
- `axum` feature `multipart` (pulls in `multer`): the intake endpoint.
- `reqwest` feature `multipart`: `hammer submit`.

## Alternatives considered

- **`sources: [upload, workspace]` in the `submission:` block:** the source belongs to the
  platform, which decides whether a session exists at all.
- **Workspace assignments that also accept uploads** (`params.uploads`): convenient, but
  mixes two sources with different trust and timing; deferred (open question).
- **A list of platforms per assignment:** a lean grading image and a code-server image, but
  two builds with little shared cache, and tests running in a different image than students
  used.
- **Overlaying the starter under the submission:** forgiving, but hides what the student
  actually submitted; required paths cover the common mistake.
- **Grading against the current build at run time instead of pinning:** a mid-queue upload
  would change a student's grade silently; regrades make that explicit.
- **Admission:**
  - creating Jobs immediately and letting the scheduler queue them: deadlines expire while
    `Pending`, and nothing shows a position;
  - a fixed concurrency limit: wrong after a node goes down or one is added;
  - Kueue now: fair sharing, priorities and preemption, but another controller on the Pis
    before there's more than one course.
- **Plain FIFO for regrades:** simpler, but a large batch delays every new submission
  behind it.
- **Skipping identical resubmissions:** students expect every submit to produce a result.
- **Waiting for egress inside the launcher, or a fixed delay:** a new helper release (and a
  rule every future helper must follow), or a guess.

## Open questions

- Multiple platforms, or mixed sources, per assignment.
- Does `await-egress`' probe (connection to the API server fails) hold on k3s's network
  policy controller as on kindnet? Checked on kind first, then on the Pis.
- Should grading pods get their own namespace (separate ResourceQuota from builds)?

## Follow-ups (recorded, out of scope)

- Submission policies: dates, attempts, late penalties, which submission counts.
- Visibility filtering of results for students (api).
- Workspace submissions (sessions snapshot `submission_dir`).
- Notification delivery (api).
- Kueue for multi-course fair sharing.
- Retention of submissions, results and logs; rate limits per student.

## Rollback

- PR 0 is a rename; reverting it restores `interactive` (recipes and pedagog revert
  together).
- PRs a–d are additive: new tables (down migration), new routes, new Job kind, an optional
  schema block, new `hammer` flags. The `await-egress` initContainer on validation pods can
  be reverted on its own.

## Implementation steps

0. **Platforms** (pedagog + recipes):
   - rename `interactive` → `workspace` in the schema, recipes, examples, tests and docs;
   - `upload` platform kind; optional `entrypoint`; `platforms/ubuntu-22/upload.yaml`;
   - `submission_dir` param on `workspace` (declared, unused until sessions);
   - `examples/linked-list-python-upload`; tests.
1. **(a) Schema and store:**
   - the `submission:` block with defaults and consistency checks (`core`);
   - migration `0004_submissions`; query functions in `store::db` (enqueue, claim, finish,
     history, regrade batch); tests against Postgres.
2. **(b) Intake and `hammer submit`:**
   - normalization (archives, junk, strip, rules, size) with the receipt; `.zip` support;
   - `POST /internal/assignments/<id>/submissions`; refusal reasons;
   - `hammer submit`, `just kind-submit`; tests.
3. **(c) Grading runs and admission:**
   - `pedagog-jobs await-egress`; added to validation pods (verify on kind it closes the gap
     the e2e probe works around);
   - the grading Job, results check, statuses, reconcile;
   - the queue and capacity gate behind `Admission`; tests.
4. **(d) Regrades and status:**
   - `hammer upload --regrade --notify`; batch creation when the build becomes current;
   - change summaries and notification intent;
   - status endpoints and `hammer submit --wait`; tests.
5. **End to end and docs:**
   - `kind-e2e` scenarios: a correct and a buggy submission graded; a refused submission
     (missing required file); a burst queued and drained; a regrade with a change summary;
     the netprobe without its `sleep`;
   - ARCHITECTURE.md (platforms, submissions, grading, admission, regrades), SETUP.md,
     BUILD-ORDER.md.
