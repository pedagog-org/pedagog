# Pedagog — Architecture

## System Overview

Pedagog is a multi-tenant programming assignment and assessment platform for universities. Instructors define development environments, test suites, and submission policies in a YAML file. Students interact with assignments through a Secure Exam Browser (SEB), either via an ephemeral code-server session or by uploading files for automated testing.

Two assignment platforms (`environment.platform.kind`); the platform decides where submissions come from, one source per assignment:

- **`workspace`** — student gets an ephemeral container with code-server, accessed through SEB. Long-lived for the duration of the exam window. Submissions are snapshots of the session's `submission_dir` (a platform param, default `.` under `/home/student/workspace`).
- **`upload`** — student uploads files or an archive. Platform unpacks and runs the test suite. No session: the platform recipe has no entrypoint or ports.

---

## Roles

Everyone has one **global role**, and one **course role** in each course they're in.

| Global role | Can |
| --- | --- |
| **Admin** | Everything: sets global roles, sees every user and course, and acts as an instructor in any course (logged) |
| **Staff** | Creates courses |
| **Student** | The default; only the course roles they're given |

| Course role | Can |
| --- | --- |
| **Instructor** | Everything in the course: assignments, dates, visibility, release, results, and its members |
| **Grader** | Sees the members and every result, and submits; changes nothing (may leave the course) |
| **Student** | Submits and sees their own work |

- Course roles don't depend on global ones: a grad TA can be an instructor without being able to create courses.
- Instructors add instructors, graders and students, and may remove themselves. Removing or demoting another instructor takes an admin, and a course always keeps one active instructor.
- Only admins set global roles; the last admin can't be demoted.

---

## Courses & Enrollment

- A **course** is a per-semester entity (e.g. COP3502 Fall 2026): an id slug like `cop3502-f26` and a display name.
- Staff and admins create courses; the creator teaches it unless they name its instructors.
- **Members** (`memberships`): one row per course and person, with their role, how they joined (`created`, `manual`, `import`), who added them, and when. Adding someone again with another role changes it.
- **Drops are soft**: a dropped member loses access and keeps their history and submissions; adding them again restores them.
- **Roster import** enrolls students from a CSV of `email,name`, or Canvas's gradebook export (its SIS login becomes `<login>@<PEDAGOG_LOGIN_DOMAIN>`, default `ufl.edu`; Canvas's points row and test student are skipped). An import only adds; with `sync` it also drops students missing from the file (hammer lists them and asks first). Staff in the file keep their role. Self-registration is not supported.
- **Renaming:** instructors and admins change a course's name and slug. References follow the slug; images and archives stored under the old one keep working, and the old slug answers "renamed to …" (`course_renames`).
- **Choosing a course in `hammer`:** `-c <course>`, else the nearest `course.yml` at or above the working directory (`course:`, and optionally `api:`, which `hammer login` defaults to), else the course saved with `hammer use <course>`.
- **Finding people:** admins list users by role or search, and `show` a user's logins, sessions and courses; course staff `show` a member of their course. Both take part of an email or name; an exact email wins, several matches are listed.
- Each course gets its own Kubernetes namespace for isolation.
- Resource limits are set at the platform level as defaults; admins may raise them per course.
- Separate limits apply to workspace sessions (code-server) and queued grading runs.

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
platform: workspace        # workspace | upload

environment:
  toolchains:               # every version pinned; see "Toolchains and Addons"
    - gcc:13+gdb:12+valgrind:3   # shorthand: parent, then its addons
    - id: python                 # object form
      version: "3.12"
      addons: [pytest:8]
  editor:                  # workspace platform only
    terminal: true         # default true; false sets student shell to /usr/sbin/nologin
    extensions:
      install: [cpptools]        # pre-installed at image build time; always in allowlist
      allow: [vim]               # in allowlist only; student may install from marketplace
  network:                 # runtime egress policy; enforced by the platform, not the image
    mode: deny             # deny (default: no egress) | allow (unrestricted)
    allow:
      - host: mirror.internal
        ports: [80]
      - host: "*.github.com"   # one leading "*." wildcard label allowed
        ports: [443]
      - cidr: 10.0.0.0/8       # omitted ports = all ports
      - cidr: 2001:db8::1/128  # IPv6; IP addresses always go under cidr
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
  duration: 120m           # workspace platform: max session length
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

- `environment.network` serves as the **default** for both build time and runtime. It never changes the image: no firewall rules are baked in, and containers never need `NET_ADMIN`.
- At build time: it applies to the validation run (the reference solution against the tests); image build steps themselves keep network access for package installs.
- Omitted, it denies all egress (`mode: deny` with no rules).
- At runtime: the YAML value is the starting point, but the platform UI can override it independently. The current runtime policy is fetched from the platform and applied as a Kubernetes network policy just before handing off the container to the student. It can be updated without a rebuild.
- Schema: `mode: allow | deny`. Under `deny`, each `allow` entry is exactly one of `host` (RFC 1123 hostname, optionally with a single leading `*.`) or `cidr` (IPv4/IPv6), plus optional `ports` (TCP; omitted = all ports). `hammer` validates the policy and shows it in `describe`.
- Enforcement: `allow` and `cidr` entries map to Kubernetes NetworkPolicies (k3s and kind both enforce them). `host` entries need an egress proxy (a planned follow-up); until it exists they're accepted but **not enforced**, so those hosts stay unreachable, and `hammer upload` warns about them.

### Assignment Updates

- **Workspace assignments:** changes are allowed freely until the first student session starts. After that, the assignment is locked.
- **Upload assignments:** changes at any time prompt "Rebuild and regrade all existing submissions?"

### Result Visibility

Configured in `assignment.results.visibility`:

- `full` — student sees all test output and score immediately on submission
- `score-only` — student sees their score but not test details
- `hidden` — student sees only a confirmation of receipt; results released manually by instructor

---

## Image Build Pipeline

Building happens in two stages:

1. **OS base image** — platform-managed. Contains the OS and any platform-level configuration. Built in-cluster by the `jobs` service (Kaniko) from the recipes pinned into the installation and pushed to the cluster registry (e.g. `pedagog/ubuntu:22`). Rebuilt when the pinned recipes change (detected at `jobs` startup / via its rebuild endpoint). Locally, `hammer plan --os <os-id>` (or `just build-base`) renders the same Containerfile for podman.
2. **Instructor image** — built when the instructor uploads the assignment archive. Starts FROM the OS base image; installs toolchains, addons, platform components (code-server for `workspace`), and locked extensions declared in `assignment.yml`. The solution is run against the test suite at this step to validate the assignment. Build-time network policy is applied during this step only.

All builds run in-cluster via **Kaniko** — no Docker daemon required. `core` renders the Containerfile from resolved recipes; the `k8s` crate submits it as a Kubernetes Job; `jobs` builds both OS base images and assignment images. Recipes are **pinned to the installation and baked** into the `jobs` image (with `hammer vend` ingredients) — not git-synced in-cluster.

### Assignment Uploads

Assignments belong to a course: a numeric id keys them, and the `id` in `assignment.yml` is their slug, unique within the course, so courses can reuse an assignment. Instructors upload with `hammer upload <dir>` through the api (`POST /courses/<course>/assignments`), which passes the archive to `jobs`' internal `POST /internal/courses/<course>/assignments`; operators can go straight to `jobs` with `--jobs-url` (`just kind-upload <dir> -c <course>` port-forwards to it on kind). The archive is the assignment directory itself, `assignment.yml` at its root.

- **`hammer upload`** checks the directory locally (parses, files present, resolves when `HAMMER_RECIPES`/`--recipes` is set), then packs every regular file except VCS metadata (`.git`, `.hg`, `.svn`). Symlinks and special files are refused.
- **`jobs` is authoritative.** It opens every upload and refuses it when:
  - it isn't a gzipped tar, or `assignment.yml` isn't at its root (400);
  - an entry is absolute, contains `..`, is duplicated, or isn't a regular file or directory (400);
  - it's over 50 MiB compressed, 200 MiB unpacked, or 10 000 entries (413; `PEDAGOG_MAX_UPLOAD_BYTES`, `PEDAGOG_MAX_UNPACKED_BYTES`, `PEDAGOG_MAX_UPLOAD_FILES`);
  - `assignment.yml` doesn't parse, a configured `files:` directory is missing, `testing:` has no tests, or it doesn't resolve against `jobs`' own recipes (422).
- **Identity:** an upload is identified by its *tree hash*, a blake3 over each file's path, executable bit and content. Timestamps, owners, entry order and other mode bits don't count, so uploading identical files again is recognized however they were packed. Archives are stored in Garage at `assignments/<course>/<slug>/<tree-hash>.tgz`, one `assignment_uploads` row per distinct tree.
- **Warnings** (accepted, but reported): `host` network rules that can't be enforced yet, and a missing reference solution (validation will be skipped).
- **Builds:** each accepted upload is rendered FROM the prebuilt OS image and built by Kaniko as `pedagog/assignments/<course>/<slug>:<image-hash>`, where the image hash covers the rendered Containerfile and the tree hash.
  - The build context is the archive: an initContainer (`pedagog-jobs fetch-context`) downloads it through a short-lived presigned URL, unpacks it with the same checks as intake, and refuses files that don't match the tree hash.
  - An image already built and validated isn't rebuilt (`hammer upload --force` overrides); a newer upload supersedes an attempt still in flight, deleting its Job.
  - Builds have a deadline (`PEDAGOG_ASSIGNMENT_BUILD_DEADLINE`, 30 min by default).
  - Kaniko keeps a layer cache in the registry (`pedagog/cache`; `PEDAGOG_BUILD_CACHE=false` turns it off), so an upload that only changes its files rebuilds just the ASSIGNMENT FILES phase. `RUN` layers are cached by their script text, so bumping an ingredient must also change its step (recipes already put the version in the script).
- **Validation:** after a successful build, a Job from the image runs `/pedagog/bin/run-tests --code /pedagog/solution` (isolation from `testing:`), which uploads `results.yaml` to a presigned PUT; no solution means validation is `skipped`.
  - Test runs (validation, and grading next) live in their own namespace, `pedagog-grading`, with their own ResourceQuota and LimitRange, so a burst of runs can't starve image builds in `pedagog-builds` or the other way round. `jobs` creates them there through a Role in that namespace; the pods only ever see presigned URLs, so no credentials live there.
  - Their init steps (`await-egress`, `fetch-context`, and Kaniko's `fetch-context`) run `pedagog/jobs-init`: the same binary as `jobs`, on a minimal base (~66 MB), so a node pulls the full `pedagog/jobs` image (~340 MB, recipes and vendored downloads baked in) only for image builds.
  - Network policies take a few seconds to apply to a new pod. Every test-run pod's first initContainer (`pedagog-jobs await-egress`) retries a connection to CoreDNS's metrics port (`kube-dns.kube-system.svc:9153`), which every policy blocks, until it fails; only then does the run start (the pod fails if that takes over 30 s).
  - The pod starts as root with every capability dropped except SETUID, SETGID, CHOWN, FOWNER, DAC_OVERRIDE and KILL (what the launcher uses to drop to `grader`/`runner`), no privilege escalation, the default seccomp profile, and no service account token. This fits Pod Security "baseline", which `pedagog-grading` enforces.
  - Egress follows `environment.network`: label-selected policies that `jobs` installs at startup allow DNS and Garage for every test-run pod (selected by its `pedagog.io/network` label), plus public addresses for `mode: allow` (private, shared `100.64.0.0/10`, link-local and unique-local ranges, so the cluster, node IPs and the LAN, are always excluded, as is anything in `PEDAGOG_BLOCKED_CIDRS`); loopback inside the pod is never filtered; `cidr` rules get a per-run policy owned by the Job. No ingress.
  - `jobs` judges the results: missing, unparseable or inconsistent results are `invalid` (not stored); consistent results where the solution disagrees with the tests are `failed` (stored, with the mismatches); otherwise `passed`. A `passed` or `skipped` build becomes the assignment's current build; a failure never replaces it.
  - Deadline `PEDAGOG_VALIDATION_DEADLINE` (10 min), resources `PEDAGOG_VALIDATION_CPU`/`PEDAGOG_VALIDATION_MEMORY` (1 CPU, 1 GiB).
- **Policy:** the file's `assignment:` policy (dates, visibility defaults) is saved on each build, so the current build's policy is the file's.
- **Status:** `GET /courses/<course>/assignments/<slug>/builds/<build>` on the api (instructors and graders; `jobs` has the same under `/internal`); `hammer upload --wait` follows it and fails on a failed build or validation. `hammer assignments list|show` lists a course's assignments and their recent builds; students see only the list.
- **Restarts:** at startup `jobs` resumes attempts in flight from what the cluster shows (a vanished build Job fails as "interrupted"; a validation is judged from whatever results were uploaded) and dispatches uploads that never got an attempt.

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
      workspace.yaml
      upload.yaml
  lib/
    vend            ← shell function installed by OS init hook
  ingredients/
    .keep
    platform/
      workspace/
        code-server.deb   ← gitignored; populated by `hammer vend`
  examples/
    pointers.yaml
```

**Subcommands:**

| Command | Description |
| --- | --- |
| `hammer plan --assignment FILE` | Resolve recipes and print a Containerfile for the assignment image (`FROM` the pre-built base) |
| `hammer plan --os ID` | Print the base OS image Containerfile |
| `hammer plan --format describe` | Print a human-readable tree of phases, layers, steps and runtime instead |
| `hammer plan --show-base` | Self-contained assignment build `FROM` upstream, including the OS layer |
| `hammer plan --registry REG` | Prefix the pre-built base image (`REG/pedagog/ubuntu:22`); upstream images are never prefixed |
| `hammer vend` | Download `ingredients:` assets declared in recipes into `$HAMMER_RECIPES/ingredients/` |
| `hammer vend --platform ID` | Vend a specific platform's assets only |

Both `plan` and `vend` accept `--recipes DIR` (repeatable) to add recipe directories beyond `HAMMER_RECIPES`.

**Build phase order:** BASE (OS init) → PLATFORM → TOOLCHAIN (each followed by its addons) → ASSIGNMENT (`environment.steps`) → OS CLEANUP → runtime (`EXPOSE`, `USER student`, exec-form `ENTRYPOINT`). See [BUILD-ORDER.md](BUILD-ORDER.md) for what each phase runs and why.

Recipes are validated when loaded: OS arg hooks (`pkg.install`, optional `pkg.remove`) may only reference their declared `args`, and every declared toolchain addon must be a real recipe sharing an OS with its parent.

### Platform Runtime

The platform recipe's `entrypoint` is a YAML list rendered as an exec-form `ENTRYPOINT`; each item is interpolated with the platform params, and an item that is exactly one list-valued `{param}` expands into one argument per element. `ports` entries are interpolated the same way and become `EXPOSE` lines, so a single `port` param drives both:

```yaml
hooks:
  build:
    params:
      port: { type: int, default: 8080 }
  entrypoint: [/usr/bin/code-server, --bind-addr, "0.0.0.0:{port}"]
  ports: ["{port}"]
```

### Toolchains and Addons

Assignments pin every toolchain version; the assignment file is the lockfile. A toolchain recipe declares optional addons with the versions known to work with it:

```yaml
# toolchains/ubuntu-22/gcc/13.yaml
addons:
  gdb: ["12"]
  valgrind: ["3"]
coexist: false           # true if this version installs side by side with other versions
```

An assignment selects addons per toolchain, as `gcc:13+gdb:12` or `{id, version, allow_coexist, addons}`. Resolution:

- Each addon must be a version its parent declares; it is layered right after its parent.
- Exact duplicates (same id and version, top-level or addon) are installed once, at the first occurrence.
- One toolchain at several versions is refused unless each version sets `coexist: true` in its recipe, or is marked with a trailing `!` (`gdb:15!`, or `allow_coexist: true`) on every assignment entry for it. The error lists each version's origins, its parents' declared versions and their overlap, both coexist flags, and a suggested fix.

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
# platforms/ubuntu-22/workspace.yaml
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
- `PEDAGOG_ID` — e.g. `ubuntu-22`, `workspace`, `gcc/12`

The function lives at `$HAMMER_RECIPES/lib/vend` and is part of the OS init contract
(see below). Every OS recipe's init hook must install it.

### OS Init Contract

Every OS recipe's `init` hook must fulfill these guarantees before control passes to
platform or toolchain steps. Platform recipes rely on this contract and must not
re-implement it.

| Requirement | Details |
| --- | --- |
| **`student` user** | Created via `useradd -m -s /bin/bash student`. Home at `/home/student`. Shell may be replaced later by the platform (e.g. `/usr/sbin/nologin` when `terminal: false`). |
| **`grader` and `runner` users** | System users without a login shell, fixed uids: `grader` (900) owns the baked hidden tests and solution (`/pedagog/{tests,solution}`, mode 0700) and runs the test harness; `runner` (901) executes student code under process isolation and can't read grader files. |
| **Workspace directory** | `/home/student/workspace` exists and is owned `student:student`. |
| **Home ownership** | Full `chown -R student:student /home/student` applied after directory creation. |
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

### Workspace Sessions

1. Student clicks start in the web UI (within the configured window). Provisioning begins immediately; the student is redirected once the container is ready.
2. The platform mounts the student's persistent NFS volume, injects a session auth token, and starts code-server and the daemon inside the container.
3. The current runtime network policy is fetched from the platform and applied as a k8s network policy before the container is handed off.
4. Traefik routes the student to their container's code-server instance.
5. The SEB Browser Exam Key is verified on every request — requests not originating from a correctly configured SEB instance are rejected.
6. On disconnect, the container remains running. The student reconnects to the same container.
7. On submission or time expiry, the daemon signals the platform via the API, the workspace is snapshotted, the test suite runs, and the container is torn down. The platform is responsible for NFS teardown.

### Workspace Session State Machine

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

On submission (from a `workspace` session or an `upload`):

1. Workspace or uploaded files are collected per the `submission:` config.
2. The test suite runs inside the container against the submitted state. All test output is written to the platform-defined `/pedagog/results/` directory.
3. The platform reads results from `/pedagog/results/results.yaml` and translates framework-native output (Catch2 XML, pytest JSON, cargo test output) into the standard results format.
4. Configured artifacts are collected from declared paths.
5. Result visibility policy is applied — student sees what the assignment allows.

### Submissions and Grading (current)

Until submitting through the api arrives, submissions go to `jobs`' internal endpoint (`hammer submit -c <course>`, or `just kind-submit` on kind). Only `upload` assignments take them; `workspace` assignments will submit from sessions.

- **Intake:** `POST /internal/courses/<course>/assignments/<slug>/submissions?student=<id>` takes several files or one `.zip`/`.tar.gz`, normalizes them with the assignment's `submission:` rules (unpack, drop OS clutter, strip a wrapping folder, include/exclude, required files, size), stores the tree in Garage, pins it to the assignment's current build, and queues a grading run. Refused (409) while there's no usable build. The reply is a receipt: files kept and ignored, and the queue position.
- **Admission:** validation and grading runs wait in a Postgres queue (`run_queue`), submissions and validations ahead of regrades. One `jobs` replica at a time (under an advisory lock) releases the next run, but only while one more run fits `pedagog-grading`'s ResourceQuota and no test run is waiting for a node (a Job without its pod, or a pod not yet scheduled). It wakes when a run is queued or a test run ends (Postgres `NOTIFY` on `pedagog_run_queue`, sent in the queueing transaction, so every replica hears it on commit), 250 ms after a release, and otherwise every two seconds, so the pace follows the cluster's real capacity; `PEDAGOG_MAX_RUNNING_TESTS` adds an optional ceiling. A run's Job is created in the transaction that releases it, so its deadline starts then. Kueue can replace this gate for multi-course fair sharing.
- **Grading run:** a Job in `pedagog-grading` from the pinned build's image: `await-egress`, then `fetch-context` puts the submission at `/pedagog/submission` (read-only), then `/pedagog/bin/run-tests --code /pedagog/submission`, with validation's pod security, network policies and limits (`PEDAGOG_GRADING_DEADLINE`, `_CPU`, `_MEMORY`). The code under test is exactly the submission.
- **Outcome:** `graded` when results parse and pass the format checks (failing tests included), with score, total and full results; otherwise `error`, with the problems and the pod's log tail. A test file that couldn't be loaded (the helper reports it as `<file>::<collection>`, e.g. the submission has a syntax error) means its tests never ran, so the run is an `error` with the load error, not a 0 out of 0. `in-process` isolation is allowed, but uploads warn that student code can then read the tests and alter its results; each run records its isolation.
- **Regrades:** `hammer upload --regrade [--notify never|changed|always]` asks for one. When that build validates and becomes current, every student's latest submission gets a new run against it, queued behind new submissions (a build that fails validation or is superseded never regrades; identical content that's already built has nothing to regrade). Earlier runs stay as history; the latest is the one shown. Each regrade run stores a change summary against the submission's previous graded run (score before and after, tests added, removed or changed, matched by id, else node) and whether to tell the student: `never` (the default: the instructor may be testing), `changed` (only if the summary isn't empty), or `always`. Delivering notifications comes with the api.
- **Status:** `GET /internal/submissions/<id>` (the submission, its receipt, and its runs latest first: status, queue position, score, problems, log tail, change summary, results) and `GET /internal/courses/<course>/assignments/<slug>/submissions?student=<id>`. Internal callers see everything; what students see is filtered by the api later. `hammer submit --wait` follows a run and prints the score and the tests that didn't pass.
- **Settling runs:** one watch on `pedagog-grading`'s test-run pods (labeled with their build or run id) judges a validation or records a grading run as soon as its main container ends, from the results it uploaded (results land about a second after the tests finish). A sweep every 30 s settles what the watch can't see: a Job that failed before it had a pod, or one whose pods are gone. Settling is idempotent, so the two can overlap. Admission, the watch and the sweep are owned by the service's main task: if one stops, the service exits and Kubernetes restarts it.
- **Run settings:** a build's platform, isolation, egress and submission rules are saved on its row at dispatch, so admission and intake read one row instead of the stored archive (builds from before that are backfilled once).
- **Dispatch:** an upload's dispatch decision (already built? supersede? start) runs in one transaction under a per-assignment advisory lock, so replicas can't race and unrelated assignments don't wait for each other.
- **Restarts:** the watch starts from the pods that exist and the sweep runs at once, so runs that ended meanwhile are settled; validations waiting in the queue stay there.

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
  score: 12                # sum of the tests' scores
  total: 32                # sum of the tests' maxes
tests:
  - number: 1
    id: null-ptr           # stable id; only when the test declares one
    node: test_null_ptr.py::test_null_ptr   # the framework's own identifier
    name: Null pointer check
    status: pass           # pass | fail | error | skip
    score: 10
    max: 10
    output: ""             # the framework's failure text
    feedback: "✓ +10 handles NULL"   # what the student is told about the score
    trace: "..."           # how the score was calculated; instructors only
    visibility:
      score: visible       # visible | hidden | after_end | after_publish
      output: after_end    # also governs feedback
    clamp: [0, 10]         # single-item tests: score bounds (null = unbounded)
    extra-credit: none     # none | bonus | attempted-work
    attempted: true
  - number: 2
    node: test_leak.py::test_memory_leak
    name: Memory leak check
    status: fail
    score: 0
    max: 20
    output: "4 memory leaks detected"
    visibility:
      score: visible
      output: after_end
  - number: 3
    id: design
    node: test_design.py::test_design
    name: Design
    status: pass
    score: 2               # sum of its items' scores
    max: 2                 # sum of the maxes of its items that count (see extra-credit)
    visibility:
      score: visible
      output: after_end
    items:                 # only on tests scored by a multi-item rubric
      - id: memory_safe
        name: No leaks
        score: 2
        max: 2
        clamp: [0, 2]
        extra-credit: none
        attempted: true
        feedback: "✓ +2 no leaks"
      - id: recursion
        name: Uses recursion
        score: 1
        max: 1
        clamp: [0, 1]
        extra-credit: bonus        # adds to the score, never to the max
        attempted: true
        feedback: ""
      - id: late
        name: Late-file penalty
        score: -1
        max: 0
        clamp: [null, 0]           # deductions only
        extra-credit: none
        attempted: false
        feedback: "-1 one file late"
artifacts:
  - id: core_dump
    label: Core dump
    path: core.dump
  - id: valgrind_log
    label: Valgrind output
    path: valgrind.log
```

Default per-test visibility is defined in the test suite source code. The `visibility` field is required on every test entry in the results file.

- **Visibility levels:** `visible` (immediately), `hidden` (never to students), `after_end` (once the student can no longer submit: the late deadline if late submissions are allowed, otherwise the due date), `after_publish` (when the instructor publishes results). `feedback` follows the output visibility; `trace` is never shown to students.
- **Rubric scoring:** a test is either a single rubric item (with `clamp`, `extra-credit`, `attempted`) or lists its `items`.
  - Each item's score lies within its `clamp`.
  - A multi-item test's score is the sum of its item scores. Its max sums the items that count: `none` items, plus `attempted-work` items once attempted. `bonus` items never count in the max.
  - With unbounded clamps or extra credit, a score can fall below 0 or exceed its max; the summary is always the plain sum of the tests.
- **Checks the platform runs:** unique `node`s, declared `id`s and item ids; scores within their bounds (tests with neither items nor a clamp must lie in 0..=max); tests and the summary that add up.

---

## SEB Integration

- The platform generates a `.seb` config file per exam. Students download it from the web UI and open it in Secure Exam Browser to begin. Instructors may also upload it directly to Canvas or another LMS.
- The `.seb` config locks SEB to the exam URL, disables extensions, prevents downloads, and embeds the Browser Exam Key.
- Every request to code-server is verified against the Browser Exam Key. Requests from a regular browser are rejected.
- Network lockdown inside the container is enforced at the k8s network policy level, independent of SEB.

---

## Auth

Students and instructors authenticate via email: the platform sends a short word-phrase to the user's email address. The auth layer is abstracted behind an interface to allow the mechanism to be swapped later (e.g. SSO, OAuth).

- **The api** (`crates/api`, `pedagog-system`, behind Traefik) is the only public entry point; a NetworkPolicy lets only it reach `jobs`, which stays internal.
- **Login:** `POST /auth/start {email}` mails a three-word code (the EFF short word list; about 2.2 × 10⁹ possibilities) to a known user, and gives everyone the same reply, so it doesn't reveal who's registered. Codes last 10 minutes and die after 5 wrong tries; a new one once a minute per email. `POST /auth/finish {email, code, client}` opens a session.
- **Sessions:** a random 32-byte token; Postgres stores only its blake3 hash, with the user, kind and expiry. The web gets an `HttpOnly; Secure; SameSite=Lax` cookie (12 h); `hammer login` saves a 30-day token (mode 0600) in `~/.config/pedagog/session.json` and sends it as a bearer token. Logging out deletes the row; expired sessions and codes are swept hourly.
- **Mail** goes over SMTP to a relay, with or without a login and TLS (`PEDAGOG_SMTP_*`); on kind, Mailpit catches it.
- **The first admin** is made with `pedagog-api add-admin <email>`; admins set global roles afterwards, and can sign a user out everywhere.

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
- **Postgres** stores platform state (users, courses, memberships, assignments, submissions, results).
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
- File upload UI (`upload` platform)
- Result view (subject to visibility policy)

**Admin:**

- Platform configuration
- Course creation and instructor assignment
- Per-course resource limit management (separate limits for workspace sessions and grading runs)
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
