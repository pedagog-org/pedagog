# 07 — Design: First Implementation Steps (Plan)

> **Date:** 2026-06-21
> **Status:** Agreed plan. Implements [`06-prompt-first-steps.md`](./06-prompt-first-steps.md).
> Code is written **only when explicitly approved**, milestone by milestone, following
> [`05-design-code-conventions.md`](./05-design-code-conventions.md).

---

## Principles

- **Vertical slices, de-risk early.** Get something runnable fast, and validate the one
  empirically-unverified assumption (code-server under path routing) before building on it.
- **Containerfiles on a Wolfi base**; **Podman** (rootless) for local dev (via `podman machine` on
  macOS, which also supplies the Linux kernel features for the nftables/cap work in M2).
- Each milestone has a concrete **acceptance check**.

## Phase 1 — container + code-server

### M1 — base image runs code-server
- **Build:** `images/base/Containerfile` — `FROM` a Wolfi base; install `code-server`; create the
  `student` user; run code-server as `student` on `:8080`.
- **Acceptance:** `podman build` then `podman run -p 8080:8080 …` → open `http://localhost:8080` →
  the editor loads and a terminal works.
- **Notes:** single-arch is fine here; multi-arch comes in M2.

### M1.5 — path-routing spike (do before M2)
- **Build:** a throwaway reverse-proxy config (Caddy or Traefik) serving code-server under
  `/s/<id>/` (the production routing shape from doc 02).
- **Acceptance:** under the subpath, the editor loads, **all assets resolve**, and the
  **WebSocket** connects (terminal + live features work). Record findings in this doc.
- **Decision gate:** if subpath routing is broken/ugly, revisit **subdomain routing** (doc 02 §3)
  before proceeding — cheaper to change now than after M3.
- **Result (2026-06-21): PASS — path routing confirmed.** Traefik (file provider) router
  `PathPrefix(/s/test)` + `stripPrefix` → code-server. code-server emits **relative** asset paths
  (`./_static/...`, `stable-<hash>/static/...`) and a **relative** redirect (`./?folder=...`), so the
  workbench (HTTP 200) and assets (200) load correctly under the subpath, and Traefik proxies the
  **WebSocket** upgrade (connection held open). So we keep **path-based routing**; no subdomain
  fallback needed. Spike config: `deploy/spike-path-routing/`. (Minor: `/s/test` without a trailing
  slash should redirect to `/s/test/`; the production proxy will add that.)

### M2 — flesh out the base image
- **Filesystem:** the `/pedagog/{instructor,student,staging}` layout with the ownership/modes from
  doc 02 §4.2.
- **Users/groups:** `student`, `pedagog`, `instructor`; the `pedagogc` group (socket clients).
- **Init/supervision:** **runit** as PID 1 — stage 1 (`/etc/runit/1`) one-time setup; stage 2
  (`runsvdir`) supervises `code-server` (uid `student`) with auto-restart; stage 3 shutdown.
  (The `pedagog` daemon service is added in M3.)
- **Network:** the **nftables** egress model (uid-owner): `student` egress per `network.mode`
  (`none` default / `allowlist` / `open`); `pedagog` uid egress allowed; rules applied under
  `CAP_NET_ADMIN` in stage 1, which is then dropped. Remove the package manager (`restrict apt`).
- **Multi-arch:** build for **arm64** (Pi targets) as well as the dev arch.
- **Acceptance:** container boots via runit; code-server still reachable; with `mode = none`, a
  shell as `student` has **no egress** while a process as `pedagog` does; `student` **cannot** alter
  the firewall.

## Phase 2 — the in-container contract

### M3 — CLI + daemon (against a control-plane stub)
- **Workspace:** (re)create the Cargo workspace per the conventions — `pedagog-core`,
  `pedagog-proto`, `pedagog-cli`, `pedagog-daemon` (more crates as needed).
- **Daemon:** reads `session_id`; CLI **Unix socket** server with `SO_PEERCRED` authorization;
  minimal **liveness heartbeat**; **control endpoint** (CP→daemon `EndSession`/`UpdateDeadline`,
  nftables-restricted); packaging/archive plumbing.
- **CLI:** the student/instructor verbs (`submit`, `time`, `archive`, `reset`, `status`) with
  role-based visibility; talks to the daemon over the socket.
- **Control-plane stub:** a minimal HTTP service implementing just the endpoints the daemon calls
  (`GET session`, `heartbeat`, `submit`, `archive`) so the daemon/CLI can be developed before the
  real control plane.
- **Acceptance:** CLI ↔ daemon over the socket; daemon heartbeats to the stub; control endpoint
  applies end/deadline; `cargo fmt`/`clippy -D warnings`/`nextest` all green.
- Wire the daemon into runit stage 2 (added to the M2 image).

## Phase 3+ (out of scope for this plan)

Real control plane (auth, sessions, **SQLx**, Nomad dispatch), Traefik forward-auth + routing,
MinIO + restic, the image registry, `pedagog-web`. Sequenced later.

## Open / deferred
- **CI platform** (GitHub Actions vs GitLab) + arm64 CI — decide before Phase 2 lands.
- **SEB** header verification with code-server — empirical, do alongside Traefik wiring (Phase 3).
- **apko/melange** — optional later hardening only.
