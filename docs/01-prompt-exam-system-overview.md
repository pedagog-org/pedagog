# 01 — Prompt: Browser-Based Coding Exam System (Overview)

> **Date:** 2026-06-20
> **Status:** Prompt captured; design under discussion (see `02-design-*` once agreed).

## Goal

A system for administering **coding exams in a web browser**. Students connect to a
restricted, ephemeral, per-student server (a Podman container) and work inside
**VS Code in the browser**. The student's browser is locked down via **Respondus
Lockdown Browser** or **Safe Exam Browser (SEB)**.

## High-level flow (v1)

1. Student is forwarded (via Lockdown/SEB) to a URL for the assignment.
2. Student authenticates with **student ID + secret phrase** (distributed before the exam).
3. The control server spins up a **Podman container** with SSH for that student.
4. Connection info is returned to the browser; student is redirected to a URL to use
   **VS Code (web)** against the new container.
5. On **first login**, the connection time is reported to the control server.
6. Both the **container** and the **control server** know when the exam ends.
7. Student either:
   - **Submits early** via a command (which may ask to also end the session), or
   - Lets the timer expire — connection is ended and submission is made automatically.
8. The exact "build a submission from artifacts" process **varies per assignment** and is
   **specified when the student container image is built**.

## Student container image

Each assignment image is built **on top of a shared, pre-built base image**.

Desired base-image properties:

- Small size.
- Low memory usage.
- **Restricted `student` user by default**: no `apt`, no `sudo`, no outbound internet,
  locked VS Code extensions, no LLM access, no access to the submission directory.
- Application-specific commands for both the **installing user** and the **student**:
  - Installer (build time):
    - `pedagog image install rust | c_gcc | python_uv | typst | latex_minimal | ...`
    - `pedagog image restrict apt | network | ...`
  - Student (run time, executed as a separate `pedagog` user):
    - `pedagog submit [--dry-run] [--no-test] [--to=dir/url] [--overwrite]`
      — packages to a dir, then submits/copies/POSTs to a dir or URL with certain headers.
    - `pedagog unsubmit [--from=url]` — request removal of submission from server.
    - `pedagog reset` — clean student dir and copy all student files back in (runs on first login).
    - `pedagog help`
    - `pedagog time`
    - `pedagog test` — essentially `pedagog prepare --to=test_dir` then run the test cases.
- A handler so that **at exam end, if no submission is registered with the server, one is made**.
- Students can **log back into their existing session** during the exam window (e.g. after a
  computer restart).
- Commands **guide students** through the exam and keep them aware of remaining time. E.g. if
  they submit to the server, they should know an automatic submission will **not** be made at the
  end unless they `unsubmit`.
- At session end, regardless of whether a submission was made, **`pedagog archive [--to=dir/URL]`**
  archives the **entire student directory** to the server. This lets instructors fix submissions
  that were not made correctly (student error or a bad submit script).
- The container and its data can be **removed from Podman immediately** after the session ends.
  All data is pushed to the server (likely **blob storage holding TGZs** of archives and submissions).

## Long-term goals to keep in mind

- Integration with **Canvas** (roster, SSO, grade passback).
- Integration with **Lockdown Browser / SEB** (genuine-client verification, config keys).
- **Test suites with points** for automated grading.

## Decisions & clarifications (follow-up, 2026-06-20)

Answers to the first round of design questions, plus additional requirements:

### Confirmed choices

- **VS Code delivery:** `openvscode-server` (browser VS Code over HTTPS). SSH is internal-only,
  not student-facing.
- **Orchestrator / control plane language:** **Rust.** The user values speed and type-safety and
  wants **idiomatic Rust with clear separation of work** (apply this across the whole codebase).
- **Assignment languages:** primarily **C / C++ / Python** (the system is Rust; the exams are not).
- **Auth (v1):** custom **student ID + secret phrase**, but built behind a pluggable **identity
  provider** interface so Canvas **LTI 1.3** slots in later with no rework. The identity provider
  should expose fields like **name(s), SID, email, alias**, etc.
- **Lockdown client:** target **SEB** first (Respondus later, via Canvas).
- **Hosting / storage:** everything hosted **at the University on encrypted drives** — likely a
  RAID network drive or **distributed per-node storage (e.g. Longhorn-style)**.

### Network model

- Restrict the **`student` user's** network access **in software, not via Podman** — because some
  assignments legitimately need access to **specific services** (e.g. a cybersecurity exam with a
  designated target server). So the container *has* a network; the `student` user's egress is
  filtered, with a **per-assignment allowlist**.
- The **`student` user** has access only to **standard system binaries, the `pedagog` binary, and
  the student's own directory** (no submission dir, etc.).

### `pedagog` daemon / token

- The local **`pedagog` daemon** is the agreed approach (privileged broker; holds secrets; only
  path to the server).
- **Session token** can be injected by Podman (e.g. an env var / secret) or obtained by the
  container POSTing to the server after it boots — exact mechanism TBD.

### Reset / first login

- On (SSH/connection) provision, check whether the session has already been reset (e.g. an
  `--on-provision` step; the **daemon tracks whether reset has happened**) and run reset if not —
  like a login script. Must **not** wipe in-progress work on reconnect.

### Submission at deadline (concern)

- The user worries a student **submits working code, then experiments, the deadline hits, and a
  broken workspace is auto-submitted**, costing points. The final-submission model must protect
  against this.

### Accommodations

- A **default assignment time limit**, plus **per-student adjustments** (a multiplier *or* an
  explicit time length).

### Scale / hardware

- Want **multiple hosts from the start** (e.g. a cluster of **4× Raspberry Pi 5**). Need guidance
  on **k8s / Swarm / Nomad / custom** orchestration — explicitly wants more info to decide.

### SEB header validation

- Idea: middleware on requests under a certain path validates SEB **headers**. At minimum, the
  **login** path requires the header. Possibly **drop the requirement on the VS Code path** (needs
  investigation re: how it interacts with `openvscode-server`'s many requests / WebSockets).

### `pedagog test` (v1 scope)

- For now, `pedagog test` should **package a submission and run a test script** (e.g. `cargo test`,
  or the assignment's test command) so students can confirm their submission will work. (Points /
  hidden tests / grade passback are later.)

### Quotas & resilience (instructor-configurable at image build)

- Quotas should be **set at image build time** and **configurable by the instructor**. The user
  wants suggestions on **what to make configurable** and **how to prevent container failure** —
  **losing student data during an exam is unacceptable**.

### Archiving cadence

- Open question: **archive every X minutes** so students can resume if something fails? The user
  wants **options** here.

## Decisions & clarifications (follow-up #2, 2026-06-20)

- **Base distro:** **Wolfi** confirmed. Memory-efficient LSPs to be considered later, when defining
  install recipes for C/C++/Python/etc.
- **Editor:** **`code-server`** (not openvscode-server) — chosen because it supports **path-based
  reverse-proxy routing** cleanly, which `openvscode-server` does not (absolute asset paths break
  under a subpath).
- **Routing:** **path-based** (`/s/<session>/...`), not subdomain. **Traefik** confirmed as the
  reverse proxy. WebSocket traffic rides the same path route.
- **Network restrictions:** keep them **simple/easy to understand**; instructors must be able to
  **opt out easily** (e.g. a simple `none | allowlist | open` knob). DNS port 53 is only relevant
  insofar as we shouldn't open it to a recursive resolver (DNS-tunnel exfiltration); no special
  machinery by default. (`CAP_NET_ADMIN` = the Linux capability that lets a process change firewall
  rules; init sets rules then drops it so the student can't undo them.)
- **Session token:** the control plane passes **only a `session_id`** into the container; the daemon
  fetches everything else (deadline, identity, etc.) from the server afterward. (`session_id` must be
  a high-entropy capability token held only by the daemon, never readable by the `student` user.)
- **Submissions vs. archives:**
  - **Submissions** = versioned, immutable, for grading. **`unsubmit` is removed.** Auto-submit at
    deadline fires only if the student never submitted.
  - **Archives** = crash-recovery + forensics; **incremental** (restic-style), **keep latest only**.
- **`.archiveignore`:** the archive ignore list lives at the **root of the student directory** as
  `.archiveignore`, **read-only to the student** so they can see what is / isn't saved (e.g.
  `target/`, `.venv`).
- **Volume lifecycle:** at provision, create a **named volume** for the student directory that
  **persists across container restarts and new containers** for the whole session, and is
  **destroyed at teardown** (at the deadline, after final archiving). Cross-node recovery (node/volume
  loss) falls back to restoring the **latest archive** onto a new volume.
- **Quotas:** per-assignment configurable; bake dependencies into the image so the student volume
  only holds source + build artifacts. Defaults toolchain-aware (Rust `target/` needs much more).
- **Orchestration:** **Nomad** (placement/scheduling/health/reschedule); our Rust control plane owns
  all exam domain logic, student↔allocation mapping/reconnect, volume/archive recovery, deadline
  lifecycle, and Traefik routing config.
- **SEB validation:** enforce on **(a) the entry/login path** and **(b) the route serving the
  code-server page**; the **WebSocket handshake** is authed by the **session cookie** (checked once,
  at handshake — not per frame).

## Decisions & clarifications (follow-up #3, 2026-06-20)

- **No inbound to containers:** the control plane must **never reach *into* containers**. Remove the
  daemon health endpoint; the **daemon pushes heartbeats** outbound and the CP infers liveness /
  drives recovery from stale heartbeats (plus Nomad alloc status).
- **One web app:** drop `pedagog-admin`; everything is **`pedagog-web`** (student login/portal/
  dashboard + instructor management & live view), separating public student routes from privileged
  instructor routes within the one app.
- **Pre-warm pool:** **future**, not v1.
- **Single active session:** at most **one active session per student across *all* assignments**.
  To start another assignment they must **End session** on their dashboard. (No per-assignment
  concurrency.)
- **Instructor live view:** **in scope** — real-time monitor (who's connected, time remaining),
  live time grants, broadcast, and recover/re-submit on a student's behalf.
- **Image registry:** needed — a private OCI registry (**Zot**, or Harbor later), **cosign-signed**,
  multi-arch (arm64 for Pis); nodes verify signatures at pull.
- **Live website viewing** (student runs a server, views it in-browser): **out of scope for v1**,
  a desired future feature.
- **Time authority:** the **control plane is the authority for time**; the daemon only reads the
  deadline from the CP and never decides when the exam ends.
- **Nomad access:** do **not** FFI-bind the Go/Python Nomad packages (FFI cost, arm64 cross-compile
  pain, runtime bloat — and they're just wrappers over the same HTTP API). Keep **`pedagog-nomad`**
  as a **thin `reqwest`/`serde` wrapper** over the few endpoints we use (optionally using Nomad's
  `POST /v1/jobs/parse` to turn a templated HCL jobspec into JSON).
