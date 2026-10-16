# Grading on Submission

The next milestone after assignment builds: a student submits code, `jobs` grades it in the
assignment's image, and the result is stored. Decisions here come from a design review on
2026-10-15/16 (and a follow-up round on its open questions).

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

- A Job from the pinned build's image, in **`pedagog-grading`**, labeled
  `pedagog.io/role=grade` and `pedagog.io/network=<mode>`:
  - **initContainer `await-egress`** (jobs image, `pedagog-jobs await-egress`): connects to
    `kube-dns.kube-system.svc:9153` (CoreDNS metrics: always up, in the pod range, and
    blocked in every mode since `validate-base` allows only port 53) until the connection
    fails; gives up (pod fails) after 30 s. Added to validation pods too. (The API server
    doesn't work as a target: it resolves to a node IP, which `mode: allow` permitted.)
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

### Namespace: `pedagog-grading`

- Validation and grading runs (the "test runs") move to a new `pedagog-grading` namespace;
  `pedagog-builds` keeps only Kaniko builds, which keep their egress.
- It gets its own ResourceQuota and LimitRange (a submission burst can't starve builds, a
  big build can't stall grading), the validate/grade NetworkPolicies, Pod Security
  `baseline`, a copy of the `garage-s3-key` Secret, and a jobs Role.
- A full quota makes pod creation fail; the admission gate sees a released run without a
  pod and pauses, which is the backpressure we want.

### Egress: "internet" means public addresses

- Closes a gap in the existing policies: `validate-internet` allowed everything except
  `PEDAGOG_CLUSTER_CIDRS` (pod and service ranges), so `mode: allow` code could reach node
  IPs (kubelet `:10250`, the API server `:6443`) and the LAN.
- `validate-internet` now always excludes `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`,
  `169.254.0.0/16`, and on IPv6 `fc00::/7` and `fe80::/10`.
- `PEDAGOG_CLUSTER_CIDRS` becomes `PEDAGOG_BLOCKED_CIDRS`, an optional extra list; kind's and
  k3s's ranges fall inside `10/8`, so both overlays drop it.
- Private hosts are reachable only through a `cidr` rule (its own policy).
- Loopback is never filtered, in any mode: tests can start student servers and connect to
  them on `127.0.0.1`/`::1` within the pod, even under `deny`.
- Lands first as its own fix PR (step 0a).

### Admission

- **Postgres FIFO queue** (option A, Kueue-ready): a `run_queue` table holds every test run
  waiting for admission, **validation and grading alike**, ordered by
  `(priority, queued_at)`; a run's position is visible in its status.
- **Priorities:** submissions and validations 0, regrades 1, so a regrade batch drains
  behind new submissions and can't starve an instructor's validation (a Kueue
  `WorkloadPriorityClass` later).
- Validation status gains `queued` (between build success and `running`).
- **Capacity gate:** a release loop (every 2 s) in each `jobs` replica:
  1. takes a transaction-level advisory lock, so only one replica releases at a time;
  2. counts test-run pods in `pedagog-grading` that are `Pending`, plus released runs whose
     pod doesn't exist yet;
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
| `run_queue` | `kind` (`validate` \| `grade`), `ref_id` (build or grading run), `priority`, `queued_at`, `released_at` |

- `assignment_builds` gains `regrade_notify` (null = no regrade requested); its
  `validation_status` gains `queued`.
- Indexes: unreleased queue entries by `(priority, queued_at)`; submissions by
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
  mixes two sources with different trust and timing; revisited with sessions.
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
- **Grading in `pedagog-builds`:** no new namespace, Secret or Role, but builds and test runs
  share one quota.
- **Validation bypassing admission:** simpler, but during a burst it sits `Pending` behind
  students and can time out.
- **Per-cluster lists of blocked ranges instead of always excluding private ranges:**
  precise, but easy to get wrong on a new cluster.
- **Egress probe targets:** the `jobs` service (a false "blocked" when `jobs` is down) or a
  sentinel Deployment (another workload to keep up).
- **Skipping identical resubmissions:** students expect every submit to produce a result.
- **Waiting for egress inside the launcher, or a fixed delay:** a new helper release (and a
  rule every future helper must follow), or a guess.

## Open questions

None left; the follow-up round resolved them (see Namespace, Egress, and the follow-ups).
Facts still to confirm while implementing:

- On kind and then on the Pis (k3s's network policy controller): `await-egress` passes
  only once policies apply, and `mode: allow` pods can't reach node IPs or the LAN.

## Follow-ups (recorded, out of scope)

- Submission policies: dates, attempts, late penalties, which submission counts.
- Visibility filtering of results for students (api).
- Workspace submissions (sessions snapshot `submission_dir`).
- Notification delivery (api).
- Kueue for multi-course fair sharing.
- Retention of submissions, results and logs; rate limits per student.
- Multiple platforms, or mixed sources (e.g. workspace with an upload fallback), per
  assignment: revisit with sessions, when instructors' needs are known; an optional param
  adds it without breaking anything.
- Networking labs across pods (a client and server on separate hosts); same-pod networking
  over loopback already works.

## Rollback

- PR 0 is a rename; reverting it restores `interactive` (recipes and pedagog revert
  together).
- Step 0a (egress) is a policy change; reverting it restores the old exclusions.
- PRs a–d are additive: new tables (down migration), new routes, new Job kind, an optional
  schema block, new `hammer` flags. The `await-egress` initContainer on validation pods can
  be reverted on its own.

## Implementation steps

0a. **Egress fix** (`fix/egress-private-ranges`, before everything else):
   - always exclude the private and link-local ranges (v4 and v6) in `validate-internet`;
   - `PEDAGOG_CLUSTER_CIDRS` → optional `PEDAGOG_BLOCKED_CIDRS`; overlays and docs;
   - tests; a `kind-e2e` check that a `mode: allow` pod reaches the internet but not a node
     IP.
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
   - the `pedagog-grading` namespace (quota, limits, policies, Pod Security, Secret, Role);
     validation moves there;
   - `pedagog-jobs await-egress`; added to validation pods (verify on kind it closes the gap
     the e2e probe works around);
   - the grading Job, results check, statuses, reconcile;
   - `run_queue` and the capacity gate behind `Admission`, for validation and grading; tests;
   - confirm the egress behavior on the Pis.
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
