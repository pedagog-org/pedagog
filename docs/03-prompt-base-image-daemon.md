# 03 — Prompt: Base Image & `pedagog` Daemon Protocol

> **Date:** 2026-06-20
> **Status:** Prompt/scope captured; design under discussion (see `04-design-*` once agreed).
> Builds on [`02-design-exam-system.md`](./02-design-exam-system.md).

## Goal

Design the **in-container contract** — the foundational build target — covering:

1. **Base image** (Wolfi): users, the `/pedagog/` directory layout, installed binaries, the
   process/init model, how nftables egress rules are applied at start, and how `reset` seeds the
   student volume at build.
2. **`pedagog` daemon ↔ control-plane protocol**: how `session_id` is delivered, how the daemon
   fetches session info, the **heartbeat** (including how the control plane — which never connects
   *into* the container — propagates authoritative deadline/commands), and the submit/archive calls.
3. **CLI ↔ daemon protocol**: the Unix-socket contract for the student commands (`submit`, `test`,
   `time`, `archive`, `reset`, `help`), including peer authentication and policy enforcement.
4. **Build-time installer**: `pedagog image install/restrict`, the per-assignment **manifest**
   (network mode, quotas, package/test spec, time limit, toolchains, `.archiveignore`), and how the
   control plane consumes it.
5. **Teardown ownership**: who runs the final archive + auto-submit at the deadline, given the
   control plane is the authority for time but cannot reach into the container.

## Constraints carried from prior docs

- One multi-call Rust binary family; idiomatic Rust, clear separation (`pedagog-daemon`,
  `pedagog-cli`, shared `pedagog-proto`).
- `student` is untrusted; `pedagog` daemon is the trusted broker and sole egress.
- Control plane **never** connects into the container (containers only reach out).
- Control plane is the **authority for time**.
- Only a `session_id` is injected; everything else is fetched.
- Wolfi base, rootless Podman, `cap-drop=ALL`, multi-arch (arm64).

## Decisions & clarifications (follow-up, 2026-06-20)

- **Control channel (reversal of doc 02's "no inbound"):** the control plane **pushes** commands
  (`EndSession`, `UpdateDeadline`) directly to the daemon's **inbound control endpoint** on the
  private network (nftables allows it **only from the control plane**). The **heartbeat is minimal
  liveness only** (~15s, no commands in the response). *(doc 02 to be reconciled.)*
- **`pedagog init`** = container **ENTRYPOINT / PID 1** (root within the user namespace): apply
  nftables, drop `CAP_NET_ADMIN`, secure the `session_id` file, then **spawn + supervise** the
  daemon (uid `pedagog`) and `code-server` (uid `student`); reap zombies, forward signals. No
  user-facing start/stop.
- **`pedagog daemon`** = long-running broker spawned by `init` (not user start/stop). Reads
  `session_id`, fetches session info, heartbeats, serves the CLI socket, serves the CP control
  endpoint, runs periodic archive, performs teardown.
- **`session_id` delivery:** Nomad `meta.session_id` + a `template` stanza renders it into the
  task's tmpfs `secrets/` dir; `init` relocates/secures it to `0400 pedagog`.
- **Socket access:** group **`pedagogc`** (= "pedagog clients": `student` + `instructor`) may
  *connect* to the socket; the daemon **authorizes by `SO_PEERCRED` uid** (student vs instructor).
  Instructor recovery (SSH in, fix, **resubmit on behalf of student**) uses the `instructor` uid.
- **Teardown:** CP pushes `EndSession` → daemon does final archive + **auto-submit if none** +
  sends a final **"terminal"** message ("I'm done") → services exit so the **entrypoint exits**.
  Per-session jobs are **batch/dispatch** jobs, so a clean exit *completes* the alloc (no restart).
  Nomad's podman driver **auto-removes the container**; the **named volume is CP-owned** and the CP
  **destroys it only after the confirmed terminal archive**. Nomad `stop` is the backstop.
- **Supervision:** PID 1 is **runit**. Stage 1 (`/etc/runit/1`) runs one-time setup (apply
  nftables, secure `session_id`); stage 2 (`runsvdir`) supervises the longrun services — `pedagog
  daemon` (uid `pedagog`) and `code-server` (uid `student`) — with automatic restart and proper
  zombie reaping; stage 3 handles shutdown. (Non-root services inherently lack `CAP_NET_ADMIN`, so
  they cannot alter the firewall.)
- **Manifest restructured** (build-time only; post-build-customizable things like time limit/title
  live in the CP assignment record, **not** the manifest):
  - `[assignment].[assignment.setup]` — `copy`/`commands` for `reset`.
  - `[container]` → `[container.network]`, `[container.quotas]` (no `memory_reservation` or
    `restart_attempts` — those are **platform-set**), `[container.toolchains]`.
  - `[submission].[submission.packaging]` — `include`/`exclude`.
  - `[archive]` — `exclude`/`include` for the recovery archive (**replaces `.archiveignore`**).
- **No built-in test feature.** `pedagog test`, the `[submission.test]` config, and the `tester`
  uid are **removed**. Instructors who want to give students tests ship them in the code copied
  into the student dir (students run them with normal tools, e.g. `make test` / `cargo test`).
- **`reset`:** **hard** reset with a loud warning + `--confirm`; versioned archives later for restore.
- **CLI command visibility by role:** the CLI detects the caller's role (student vs `instructor`)
  and only exposes permitted subcommands; the daemon still re-authorizes by `SO_PEERCRED`.
- **Build input:** instructor uploads a **TGZ** unpacked into `/pedagog/instructor/`; the build then
  installs toolchains and runs `reset` (as `student`, last step).
- **Control-endpoint auth:** **no signing for v1** — restrict ingress to the control port to the
  control plane via nftables, *and* drop `student`-uid access to that port (same-netns caveat).
  Signing remains a later hardening if the cluster-network trust assumptions weaken.

### Resolved (this round)
- **Supervisor:** **runit** as PID 1 (stage-1 setup, stage-2 `runsvdir`, stage-3 shutdown).
- **Per-session jobs:** **parameterized batch (dispatch) jobs** — a clean exit completes the alloc
  (no restart on success), while restart/reschedule policies still cover *failures* before the
  deadline.

### Still open
- None for this step — ready to implement the daemon socket server + control endpoint, the runit
  service definitions, and the Wolfi base image (`apko`/`melange`).
