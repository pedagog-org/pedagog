# Pedagog — Architecture

## System Overview

Pedagog is a multi-tenant programming assignment and assessment platform for universities. Instructors define development environments, test suites, and submission policies in a YAML file. Students interact with assignments through a Secure Exam Browser (SEB), either via an ephemeral code-server session or by uploading files for automated testing.

Two assignment platforms:

- **Interactive** — student gets an ephemeral container with code-server, accessed through SEB. Long-lived for the duration of the exam window.
- **Submission-only** — student uploads files or an archive. Platform unpacks, builds, and runs the test suite. No interactive session.

---

## Roles

| Role | Scope |
| --- | --- |
| **Admin** | Full platform access: creates courses, manages resource limits, manages platform config |
| **Instructor** | Manages their own courses: uploads assignments, configures policies, views results |
| **Student** | Enrolls in courses, takes exams, submits work |
| **Grader** | _(future)_ Per-course elevated access for grading workflows |

Instructors may only manage courses they own. Admins may manage any course.

---

## Courses & Enrollment

- A **course** is a per-semester entity (e.g. COP3502 Fall 2026).
- Courses are created by admins and assigned to an instructor.
- Each course gets its own Kubernetes namespace for isolation.
- Students are enrolled via roster import (CSV). Self-registration is not supported.
- Resource limits are set at the platform level as defaults; admins may raise them per course.
- Separate limits apply to interactive containers (code-server) and queued submission runners.

---

## Assignment Model

Instructors submit an assignment as a TGZ archive containing:

- `assignment.yml` — the assignment definition
- `starter/` — code provided to the student
- `solution/` — reference implementation (used to validate the assignment at build time; never exposed to students)
- `tests/` — test suite (hidden from students)

### YAML Schema

```yaml
name: pointers-and-memory
platform: interactive      # interactive | submission

environment:
  toolchains:
    - id: gcc
      version: "13"
      options:
        std: "17"
      addons: [gdb:14, valgrind:3]
  editor:                  # interactive mode only
    terminal: true         # default true; false sets student shell to /usr/sbin/nologin
    extensions:
      install: [cpptools]        # pre-installed at image build time; always in allowlist
      allow: [vim]               # in allowlist only; student may install from marketplace
  network:                 # build-time only — runtime policy is managed in the platform
    egress: deny
    allow:
      - host: mirror.internal
        port: 80
      - cidr: 10.0.0.0/8
      - host: "2001:db8::1"    # IPv6
  sources:
    student: ./starter/
    solution: ./solution/
    tests: ./tests/

submission:
  # Produce an archive from the student workspace
  pack:
    format: tgz
    from: /home/student/
    select:
      - Cargo.toml
      - Cargo.lock
      - src/**/*.rs

  # OR: submit one or more existing files
  # path:
  #   from: /home/student/
  #   select: [submission.tar.gz]
  #   unpack: true         # unpack the selected file before testing

testing:
  # Framework shorthand — platform knows how to invoke and parse output
  framework:
    type: catch2           # catch2 | pytest | cargo
    build: cmake --build ./build --target tests   # catch2 only
    binary: ./build/tests  # catch2 only

  # pytest example:
  # framework:
  #   type: pytest
  #   path: ./tests
  #   args: ["-m", "not slow"]   # optional

  # cargo test example:
  # framework:
  #   type: cargo
  #   path: .              # project root

  # OR: full control when framework shorthand is insufficient
  # run:
  #   - cmake --build ./build
  #   - ./build/tests

  artifacts:
    - id: core_dump
      label: Core dump
      path: /pedagog/results/core.dump
      optional: true
      visibility: after_end    # visible | hidden | after_end | after_publish
    - id: valgrind_log
      label: Valgrind output
      path: /pedagog/results/valgrind.log
      optional: true
      visibility: after_end

# Policy suggestions — surfaced in the UI when publishing; instructor may override
assignment:
  dates:
    open: "2026-09-01T14:00:00-04:00"
    due: "2026-09-01T16:00:00-04:00"
    late_due: "2026-09-02T16:00:00-04:00"   # absent = late submissions not accepted
  duration: 120m           # interactive mode: max session length
  attempts: 1
  results:
    visibility: score-only  # full | score-only | hidden
  late_policy:
    grace: 15m
    penalty:
      subtract:            # or: multiply: { factor: 0.9, per: day }
        amount: 10
        unit: percent      # percent | points
        per: day           # day | hour
      apply_to: final      # max (reduces max before scoring) | final (subtracted after)
      cap: 50              # max achievable score (%), optional
      except:
        - weekends
        - dates: ["2026-12-25", "2026-01-01"]
  audit:
    commands: true         # record all shell commands executed by the student
```

### The `assignment:` Block

The `assignment:` block is treated as a **suggestion**. When an instructor publishes an assignment, the UI surfaces these values and may prompt for confirmation or override. This key is intentionally separate from `environment:` to make clear it is not part of the container definition.

### Network: Build-Time and Runtime

- `environment.network` serves as the **default** for both build time and runtime.
- At build time: applied when the builder runs the solution validation step.
- At runtime: the YAML value is the starting point, but the platform UI can override it independently. The current runtime policy is fetched from the platform and applied as a Kubernetes network policy just before handing off the container to the student. It can be updated without a rebuild.

### Assignment Updates

- **Interactive assignments:** changes are allowed freely until the first student session starts. After that, the assignment is locked.
- **Submission-only assignments:** changes at any time prompt "Rebuild and regrade all existing submissions?"

### Result Visibility

Configured in `assignment.results.visibility`:

- `full` — student sees all test output and score immediately on submission
- `score-only` — student sees their score but not test details
- `hidden` — student sees only a confirmation of receipt; results released manually by instructor

---

## Image Build Pipeline

Building happens in two stages:

1. **OS base image** — platform-managed. Contains the OS and any platform-level configuration. Built in-cluster by the `jobs` service (Kaniko) from the recipes pinned into the installation and pushed to the cluster registry (e.g. `pedagog/ubuntu:22`). Rebuilt when the pinned recipes change (detected at `jobs` startup / via its rebuild endpoint).
2. **Instructor image** — built when the instructor uploads the assignment archive. Starts FROM the OS base image; installs toolchains, addons, platform components (code-server for interactive), and locked extensions declared in `assignment.yml`. The solution is run against the test suite at this step to validate the assignment. Build-time network policy is applied during this step only.

All builds run in-cluster via **Kaniko** — no Docker daemon required. `core` renders the Containerfile from resolved recipes; the `k8s` crate submits it as a Kubernetes Job (`jobs` for OS base images, `api` for instructor images later). Recipes are **pinned to the installation and baked** into the `jobs` image (with `hammer vend` ingredients) — not git-synced in-cluster.

### `hammer` — Build Tool

`hammer` is a CLI tool (`crates/hammer/`) — a thin wrapper over `core`'s recipe resolution and Containerfile rendering — for local inspection (`plan`) and ingredient vending (`vend`). Recipe resolution/rendering itself lives in `core`, so the `jobs` service renders in-process rather than shelling out. Recipes live in a separate `recipes` repo, pointed to via `HAMMER_RECIPES`.

**Recipe layout:**
```text
$HAMMER_RECIPES/
  os/
    ubuntu-22.yaml
  toolchains/
    ubuntu-22/
      gcc/12.yaml  13.yaml
      clangd/14.yaml
      ...
  platforms/
    ubuntu-22/
      interactive.yaml
      submission.yaml
      multi-submission.yaml
  lib/
    vend            ← shell function installed by OS init hook
  ingredients/
    .keep
    platform/
      interactive/
        code-server.deb   ← gitignored; populated by `hammer vend`
  examples/
    pointers.yaml
```

**Subcommands:**

| Command | Description |
| --- | --- |
| `hammer plan --assignment FILE` | Resolve recipes and print the build plan (default: human-readable) |
| `hammer plan --os ID` | Print the base OS image plan |
| `hammer plan --format containerfile` | Emit a Containerfile instead of the describe view |
| `hammer vend` | Download `ingredients:` assets declared in recipes into `$HAMMER_RECIPES/ingredients/` |
| `hammer vend --platform ID` | Vend a specific platform's assets only |

Both `plan` and `vend` accept `--recipes DIR` (repeatable) to add recipe directories beyond `HAMMER_RECIPES`.

### Building and Running a Container Locally

1. **Vendor dev assets** (skip if pulling from source at build time is acceptable):
   ```sh
   HAMMER_RECIPES=/path/to/recipes hammer vend
   ```

2. **Generate the Containerfile:**
   ```sh
   HAMMER_RECIPES=/path/to/recipes hammer plan \
     --assignment /path/to/recipes/examples/pointers.yaml \
     --format containerfile \
     --show-base \
     --output Containerfile
   ```

3. **Build the image:**
   ```sh
   podman build \
     -v /path/to/recipes:/pedagog/recipes:ro,z \
     -v /path/to/recipes/ingredients:/pedagog/ingredients:ro,z \
     -f Containerfile \
     -t pedagog/pointers-and-memory:dev
   ```

   > **SELinux tip:** On SELinux-enforcing systems (Fedora, RHEL), bind mounts require
   > the `,z` option to relabel the source for container access. Without it, the build
   > step that reads from `/pedagog/recipes/lib/vend` will fail with "Permission denied".

4. **Run the container:**
   ```sh
   podman run --rm -p 8080:8080 pedagog/pointers-and-memory:dev
   ```

### Ingredient Vendoring

Platform and toolchain recipes may declare `ingredients:` — dev assets that are fetched
locally to speed up or enable offline builds. Each entry is either a GitHub release asset
or a plain URL:

```yaml
# platforms/ubuntu-22/interactive.yaml
ingredients:
  - output: code-server.deb
    github:
      repo: pedagog-org/code-server
      asset: "code-server_*_arm64.deb"
      tag: v4.127.0-pedagog.1
```

`hammer vend` downloads these into `$HAMMER_RECIPES/ingredients/<type>/<id>/`. The
`ingredients/` directory is gitignored (except for `.keep`); only `lib/` and recipe
YAMLs are tracked.

Recipe steps check for the vendored file before falling back to the source URL:
```bash
if [ -f "$(vend code-server.deb)" ]; then
  dpkg -i "$(vend code-server.deb)"
else
  curl -fsSL "https://..." -o /tmp/code-server.deb && dpkg -i /tmp/code-server.deb
fi
```

### The `vend` Shell Function

`vend <filename>` resolves a vendored asset path using two env vars that hammer emits as
`ENV` instructions before each recipe section's steps:

- `PEDAGOG_TYPE` — `os`, `platform`, or `toolchain`
- `PEDAGOG_ID` — e.g. `ubuntu-22`, `interactive`, `gcc/12`

The function lives at `$HAMMER_RECIPES/lib/vend` and is part of the OS init contract
(see below). Every OS recipe's init hook must install it.

### OS Init Contract

Every OS recipe's `init` hook must fulfill these guarantees before control passes to
platform or toolchain steps. Platform recipes rely on this contract and must not
re-implement it.

| Requirement | Details |
| --- | --- |
| **`student` user** | Created via `useradd -m -s /bin/bash student`. Home at `/home/student`. Shell may be replaced later by the platform (e.g. `/usr/sbin/nologin` when `terminal: false`). |
| **Workspace directory** | `/home/student/workspace` exists and is owned `student:student`. |
| **Home ownership** | Full `chown -R student:student /home/student` applied after directory creation. |
| **`gosu`** | Installed. Used to drop privileges in entrypoint scripts. |
| **`ca-certificates`** | Installed. Required for TLS in package downloads and API calls. |
| **`curl`** | Installed. Used in platform build steps. |
| **`vend` function** | `install -m755 /pedagog/recipes/lib/vend /usr/local/bin/vend` — makes `vend` available in all subsequent recipe steps. |

Platform and toolchain steps must not depend on anything outside this contract and the
package manager being functional.

The Containerfile renderer always emits `USER student` immediately before `ENTRYPOINT`,
so the entrypoint always runs as the student user.

### Base Image Layout (in repo)

```text
images/
  base/
    entrypoint.sh    # starts code-server + daemon, performs session handoff
```

The daemon binary is copied into the image from the compiled Rust artifact at build time. The Containerfile is generated by `hammer` from the OS definition and platform recipe — it is not a static file in the repo.

Built images are stored in the cluster's internal registry using immutable version tags (e.g. `:v1`, `:v2`). The platform reference-counts active sessions per image version. Old versions with no active sessions are eligible for GC and removed by a background job; the latest version is always retained. The image version is recorded on each submission row in Postgres for audit purposes. The instructor layer is rebuilt if the assignment is updated (subject to the update rules above).

---

## Container Lifecycle

### Interactive Mode

1. Student clicks start in the web UI (within the configured window). Provisioning begins immediately; the student is redirected once the container is ready.
2. The platform mounts the student's persistent NFS volume, injects a session auth token, and starts code-server and the daemon inside the container.
3. The current runtime network policy is fetched from the platform and applied as a k8s network policy before the container is handed off.
4. Traefik routes the student to their container's code-server instance.
5. The SEB Browser Exam Key is verified on every request — requests not originating from a correctly configured SEB instance are rejected.
6. On disconnect, the container remains running. The student reconnects to the same container.
7. On submission or time expiry, the daemon signals the platform via the API, the workspace is snapshotted, the test suite runs, and the container is torn down. The platform is responsible for NFS teardown.

### Interactive Session State Machine

```text
Pending → Provisioning → Ready → Active ↔ Disconnected
                                        → Submitted
                                        → Expired
                                        → Terminated
```

- `Pending` — student clicked start; session record created, provisioning not yet started
- `Provisioning` — container being created, NFS volume mounted, code-server and daemon starting; student waiting for redirect
- `Ready` — container up; redirect sent; waiting for first connection
- `Active` — student connected and working; daemon heartbeat live
- `Disconnected` — student connection dropped; container still running; waiting for reconnect
- `Submitted` — student triggered submission; test suite ran; results collected; container torn down
- `Expired` — `deadline_at` passed; container torn down by watchdog
- `Terminated` — manually killed by instructor or admin

### Submission-Only Mode

1. Student uploads files or an archive via the web UI.
2. A job row is written to the DB and a `NOTIFY` is fired. A runner picks it up via `SELECT ... FOR UPDATE SKIP LOCKED` and begins processing.
3. A container is provisioned from the assignment image, the submission is mounted (and unpacked if configured), the test suite runs, and results are collected.
4. The container is torn down immediately after the run.

### Submission Job State Machine

```text
Queued → Claimed → Provisioning → Running → Collecting → Completed
                                                        → Failed
                                                        → TimedOut
```

- `Queued` — job row created; no runner has claimed it
- `Claimed` — a runner holds a lease (`lease_expires_at`) and is beginning work; renewed periodically
- `Provisioning` — runner is creating the k8s pod for the test run
- `Running` — test suite executing; runner streaming logs
- `Collecting` — test finished; runner reading `/pedagog/results/`, gathering artifacts, writing results to DB
- `Completed` — results stored, artifacts collected, container torn down
- `Failed` — something went wrong at any stage
- `TimedOut` — job exceeded allowed duration or sat in `Queued` past a max wait threshold

Runners are long-lived services with two concurrent loops:

1. **Job loop** — `LISTEN` on Postgres for new job notifications; claim via `SELECT ... FOR UPDATE SKIP LOCKED`; process
2. **Watchdog loop** — periodic tick scanning for: expired leases (reset to `Queued`), jobs past `deadline_at` (`TimedOut`), sessions past `deadline_at` (`Expired`), provisioning stuck > N minutes (`Failed`)

If a runner crashes, its leases expire naturally and other runners pick up the uncompleted jobs.

### The In-Container Daemon

A process running inside every student container, responsible for:

- Sending periodic heartbeats to the platform API
- Listening for submission and time-expiry signals from the platform
- Triggering the submission flow (snapshot workspace, invoke test runner, report results)
- Enforcing time limits locally as a fallback if the platform signal is not received

Communication is via direct API calls to the platform, authenticated using a session token injected at container start.

### Persistence

Student workspaces are stored on shared NFS storage. Each student gets a per-assignment volume mounted into their container. The platform server sets up the NFS volume before handing off to the container, and is responsible for teardown. This allows containers to be rescheduled to any node after a disconnect or node failure without data loss. Running process state (open terminals, etc.) is not preserved across reconnects — only the filesystem.

---

## Submission & Testing

On submission (interactive or submission-only):

1. Workspace or uploaded files are collected per the `submission:` config.
2. The test suite runs inside the container against the submitted state. All test output is written to the platform-defined `/pedagog/results/` directory.
3. The platform reads results from `/pedagog/results/results.yaml` and translates framework-native output (Catch2 XML, pytest JSON, cargo test output) into the standard results format.
4. Configured artifacts are collected from declared paths.
5. Result visibility policy is applied — student sees what the assignment allows.

### Supported Test Frameworks

| Framework | Language | Platform handles invocation and output parsing |
| --- | --- | --- |
| Catch2 | C/C++ | Yes |
| pytest / unittest | Python | Yes |
| cargo test | Rust | Yes |

For unsupported frameworks or custom pipelines, use `run:` with explicit commands.

### Results YAML Format

The platform writes (and reads) a standard results file at `/pedagog/results/results.yaml`:

```yaml
submission-id: ~           # UUID, or null for test runs
run-id: ~                  # UUID, or null for test runs
summary:
  score: 85
  total: 100
tests:
  - number: 1
    id: test_null_ptr
    name: Null pointer check
    status: pass           # pass | fail | error | skip
    score: 10
    max: 10
    output: "All assertions passed"
    visibility:
      score: visible       # visible | hidden | after_end | after_publish
      output: after_end
  - number: 2
    id: test_memory_leak
    name: Memory leak check
    status: fail
    score: 0
    max: 20
    output: "4 memory leaks detected"
    visibility:
      score: visible
      output: after_end
artifacts:
  - id: core_dump
    label: Core dump
    path: core.dump
  - id: valgrind_log
    label: Valgrind output
    path: valgrind.log
```

Default per-test visibility is defined in the test suite source code. The `visibility` field is required on every test entry in the results file.

---

## SEB Integration

- The platform generates a `.seb` config file per exam. Students download it from the web UI and open it in Secure Exam Browser to begin. Instructors may also upload it directly to Canvas or another LMS.
- The `.seb` config locks SEB to the exam URL, disables extensions, prevents downloads, and embeds the Browser Exam Key.
- Every request to code-server is verified against the Browser Exam Key. Requests from a regular browser are rejected.
- Network lockdown inside the container is enforced at the k8s network policy level, independent of SEB.

---

## Auth

Students and instructors authenticate via email: the platform sends a short word-phrase to the user's email address. The auth layer is abstracted behind an interface to allow the mechanism to be swapped later (e.g. SSO, OAuth).

---

## Infrastructure

```text
[SEB / Browser]    →  [Traefik]  →  [API]
[Leptos Frontend]  →  [Traefik]  →  [API]
                                 →  [code-server (per-student container)]

[Instructor CLI]   →  [API]

[API]   →  [k8s]              →  [Cluster Registry]
        →  [Postgres NOTIFY]  →  [Jobs]  →  [k8s]  →  (submission/build containers)
        →  [Longhorn PVCs]        (student workspaces)
        →  [Postgres]             (platform state)
```

- **k3s** is used for both local development and production — same runtime in both environments.
- **Podman Compose** brings up infra services (Postgres, cluster registry, Traefik) for local dev.
- **Traefik** handles ingress and per-student routing to code-server instances.
- **Longhorn** provides distributed block storage for student workspace PVCs, replicated across nodes.
- **Postgres** stores platform state (courses, enrollments, assignments, submissions, results).
- **Cluster registry** stores built assignment images.
- One **Kubernetes namespace per course**.
- Designed to run on multiple Raspberry Pi nodes initially, scaling to full Kubernetes clusters.

### Subcomponents

| Name | Role |
| --- | --- |
| **Jobs** | Long-lived service: manages image builds and submission test runs; two internal loops (job loop + watchdog) |
| **Daemon** | In-container process; handles heartbeat, submission trigger, time enforcement |

### Crate Structure

```text
crates/
  core/     # shared domain types + recipe resolve/render + env; WASM-shareable (no kube/sqlx)
  store/    # build-state persistence (sqlx/Postgres) behind a feature-gated `db` module
  api/      # REST API server (axum)
  jobs/     # long-lived Jobs service (OS base builds; dispatch-and-return executor)
  daemon/   # in-container daemon
  hammer/   # thin CLI over core: `plan` (inspect) + `vend` (ingredients); build-time only
  web/      # Leptos frontend
  k8s/      # the only crate touching kube/k8s-openapi; `build` (Kaniko) + `run` (later)
```

The `k8s` crate is the sole holder of `kube`/`k8s-openapi`: its `build` submodule owns Kaniko image builds (used by `jobs` for OS images, and `api` for instructor images later), and `run` (later) will own session/submission pods. Persistent build state lives in `store` (feature-gated sqlx), keeping `core` WASM-shareable.

---

## Frontend

The web UI is built with **Leptos** (Rust → WASM) and lives in the `web` crate of the workspace. It calls the platform REST API directly. Shared Rust types (request/response structs) come from the `core` crate, giving the frontend full type safety when deserializing API responses without duplicating definitions.

The frontend and API are independently deployable. The API is callable by any HTTP client — the Leptos frontend is one consumer, not a dependency.

---

## Web UI

**Instructor:**

- Course management (students, assignments, results)
- Assignment publishing with policy review and override
- Live build log streaming
- Submission and result dashboard (filterable, exportable)
- Artifact and submission download (TGZ)
- Grade export (CSV)

**Student:**

- Exam list and status
- Exam start (triggers container provisioning and SEB config download)
- File upload UI (submission-only mode)
- Result view (subject to visibility policy)

**Admin:**

- Platform configuration
- Course creation and instructor assignment
- Per-course resource limit management (separate limits for interactive and submission-only containers)
- User and roster management

---

## Grading

- Test suites produce results in the standard results YAML format, which feeds the instructor dashboard.
- Scores and results are exportable as CSV.
- Artifacts and raw submissions are exportable as TGZ.

**Future — hardware grading workflow:** assignments will be able to specify a grader script that runs on the grader's machine: download artifact → run command → wait → prompt for grade entry → advance to next student. The platform will coordinate artifact delivery and grade collection. Current architecture should not preclude this.

---

## Observability

All services emit structured logs via the `tracing` crate, serialized as JSON.

**Local development:** `stern` streams logs from multiple k8s pods simultaneously with per-pod color coding and label-based filtering. Usage is documented in the dev setup guide.

**Production:** Loki collects and indexes logs from all pods; Grafana provides a unified UI for querying and streaming across services. Setup and usage are documented in the operations guide.

---

## Open Questions

- CLI binary name (`pedagog` for now; alias to be decided later)
