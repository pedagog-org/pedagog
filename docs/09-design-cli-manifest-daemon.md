# 09 — Design (Draft): `pedagog` CLI, `pedagog.toml` Manifest, and Minimal Daemon

> **Date:** 2026-06-21
> **Status:** **Draft** — agreed direction; redline welcome. Implements
> [`09-prompt-cli-manifest-daemon.md`](./09-prompt-cli-manifest-daemon.md). Follows
> [`05-design-code-conventions.md`](./05-design-code-conventions.md); builds on the security model in
> [`02-design-exam-system.md`](./02-design-exam-system.md) §4.2 and the runit boot model in
> [`07-design-first-steps.md`](./07-design-first-steps.md).

## 1. Shape

Three trust domains in the container: **student** (untrusted editor), **pedagog** (trusted broker
daemon), **instructor** (owns the source tree). The `pedagog` binary exposes **admin/authoring verbs
under `pedagog image …`**, runnable **at build time or by an instructor over SSH** — timing-agnostic
and **idempotent**; they just have to complete before students log in. `pedagog.toml` is the
**declarative** source; `pedagog image build` orchestrates the **imperative** primitives from it.

All `pedagog image …` verbs are **instructor/root-only** — never executable by `student`. That is
what lets us keep the package manager in the image (for debugging) without handing it to students.

## 2. Pipeline & boot

```
instructor: pedagog.toml + seeds
        │  pedagog image build  (host or in-container; idempotent)
        ▼
per-assignment image:
  /pedagog/source/*            instructor inputs (manifest + seeds)
  /pedagog/config/*            resolved state: toolchain defs, build.toml, nftables.conf
  toolchains + pkgs installed
  pedagog runit service registered
  pedagog binaries baked in
        │  container start
        ▼
/etc/runit/boot  (PID 1, root, caps: NET_ADMIN SETPCAP SETUID SETGID)
  ├─ /etc/runit/1 → nft -f /pedagog/config/nftables.conf            # load egress rules into the netns
  └─ exec setpriv --bounding-set=-net_admin,-setpcap runsvdir …     # student session: LOCK AT BOOT — nft immutable
        │                                                            # instructor session: skip lock (exec runsvdir directly)
        ├─ code-server   (student: chpst -u student;  instructor: setpriv → uid 1003 + ambient net_admin)
        └─ pedagog       (chpst -u pedagog)        # the daemon
```

The per-session **`PEDAGOG_USER_TYPE`** (`student` default | `instructor`) selects the editor identity
and whether the firewall is locked — see §6.

The firewall load is a **privileged boot one-shot** (not the daemon, which is unprivileged). Rules
are **persisted as a file** and loaded each boot — exactly like `/etc/nftables.conf` +
`nftables.service` on a normal host.

## 3. Filesystem / config trees

| Path | Owner / mode | Contents |
|---|---|---|
| `/pedagog/source/` | `instructor:pedagog` `0750` | Instructor inputs: `pedagog.toml` + seed files (student: none) |
| `/pedagog/config/` | `root:pedagog` `0750` | Pedagog-managed resolved state (student: none) |
| `/pedagog/config/toolchain/` | `root:pedagog` | Registered toolchain definition TOMLs |
| `/pedagog/config/build.toml` | `root:pedagog` | The registered build config (`build --info` prints this) |
| `/pedagog/config/nftables.conf` | `root:pedagog` | Compiled **egress** ruleset, loaded at boot |
| `/pedagog/student/` | `student:pedagogc` `2770` | Student home (named volume at runtime); group-shared so an instructor session (uid 1003) can open it |
| `/pedagog/staging/` | `pedagog:pedagog` `0700` | Submission packaging |

## 4. `pedagog.toml` manifest

Declarative source of truth. **Versioned**: a top-level `version` is a full semver string, validated
against the caret requirement `^0.1` (`>= 0.1.0, < 0.2.0`) — minor/patch are backward-compatible
within the line; a breaking change bumps the minor and adds a new schema module. Forward-migration of
older schemas is handled by `magic_migrate` in `pedagog-core` (each version's types grouped in its own
`vN` module; the latest re-exported). Image-build config lives under **`[image]`**, kept separate from
assignment-level config (timing, archival, …) which gets its own top-level tables at their milestones.
`version` and `[image]` (with `network`) are defined now; the rest is sketched.

```toml
version = "0.1.0"

[image]
toolchains = ["rust"]               # registered toolchain ids to install (doc 10)
additional_packages = ["ripgrep"]   # extra apk packages (doc 10)

[image.network]
# EGRESS only. Targets are IP addresses or CIDRs (no hostnames in v1).
mode = "default"            # "default" | "block" | "open" | "custom"

allow = ["10.0.0.0/24", "10.0.0.5"]   # mode = "block": block all student egress EXCEPT these
block = ["192.168.0.0/16"]            # mode = "open":  allow all student egress EXCEPT these
rules = [                              # mode = "custom": ordered, first-match; terminal = drop
  { action = "allow", to = "10.0.0.5" },
  { action = "block", to = "10.0.0.0/8" },
]

# Future top-level (assignment-level) tables, sketched:
# [archive]   # submit/archive include/exclude (M3)
# [quotas]    # maps to codebox job vars (doc 08)
```

**Modes** (only the field matching the mode is used):
- **`default`** — student egress fully blocked (fail-closed; no list). The default.
- **`block`** — blocked except `allow` (IP/CIDR).
- **`open`** — allowed except `block` (IP/CIDR).
- **`custom`** — ordered `rules` (first-match), terminal **drop** (fail-closed).

Missing file / `[network]` / parse error ⇒ **fail closed to `default`**.

## 5. CLI surface — `pedagog image …`

| Verb | Notes |
|---|---|
| `build [CONFIG=/pedagog/source/pedagog.toml]` | Declarative; orchestrates the primitives from the manifest. `--info` prints the registered `build.toml`. |
| `toolchain list [-a/--all \| -i/--installed (default) \| -u/--uninstalled]` | |
| `toolchain install [TOOLCHAINS…]` / `remove [TOOLCHAINS…]` | `uninstall` = alias of `remove` |
| `toolchain register [DEFN.toml…]` | Copies def into `/pedagog/config/toolchain/` |
| `toolchain unregister [PATH\|ID…]` | Removes the registered def file (by path or toolchain id) |
| `pkg installed` / `install [PKGS…]` / `remove [PKGS…]` | Wraps `apk`; tracks what it installed. (renamed from `apt`) |
| `daemon init` | Registers the runit service (`/etc/service/pedagog/`). No `start`/`stop` — runit owns lifecycle. |
| `network status` | Human summary of the **egress** ruleset |
| `network convert [--config C]` | Rewrite the manifest's `[network]` into an equivalent `custom` rule list |
| `network load [--config C] [--out O] [--compile-only]` | Render the manifest and **apply it live** (`nft -f -`); `--compile-only` instead **writes** `nftables.conf` |

There are **no `rules add/remove` verbs**: the instructor edits `pedagog.toml` directly in the editor
(using `convert` first if they need an ordered list), then `load`s it. The live apply is `nft -f -`;
the boot load is `nft -f /pedagog/config/nftables.conf` in `/etc/runit/1`.

### 5.1 `convert` — manifest → custom

- Rewrites only the `[network]` table of `pedagog.toml` via **`toml_edit`** (other tables, comments,
  and ordering preserved), setting `mode = "custom"` with the `lower()`'d rules. The result is
  **re-parsed into the typed `Manifest` to validate before the file is written** (atomic temp+rename),
  so we never persist a manifest that won't load. The manifest (`/pedagog/source/pedagog.toml`) is
  instructor-owned, so this works at build time *and* in an instructor session.
- Semantics are preserved: `default`→empty rules; `block`→its `allow` list as `accept` rules;
  `custom`→no-op. **`open`** is the one verbose case — its terminal *accept* is expressed in a
  terminal-drop `custom` list by appending catch-all `{ allow, 0.0.0.0/0 }` + `{ allow, ::/0 }` rules
  (noted in output).

### 5.2 `load` — make the manifest effective

- **Default (live apply):** render the manifest and pipe it to the kernel (`render | nft -f -`). **No
  file is written.** Needs `CAP_NET_ADMIN` — available in an **instructor** session (the editor's
  ambient cap, inherited by the terminal; the `pedagog` binary is `0750 root:instructor`), not in a
  locked student session. Deliberately does **not** touch the baked boot policy, so live
  experimentation stays ephemeral and never silently rewrites the exam's compiled ruleset.
- **`--compile-only` (write the file):** render the manifest and write `--out`
  (default `/pedagog/config/nftables.conf`); **no live apply**. This is the **build-time** path
  (`/pedagog/config` is `root:pedagog`, written as root during build) and what boot later loads.
  Replaces the earlier standalone `compile` verb. A missing manifest compiles to the fail-closed
  `default`; a malformed one is an error.

All verbs **idempotent** (re-running `build` reproduces the same image). **Role-gated to
instructor/root.** Student-facing verbs (`submit`/`time`/…, doc 02 §10) are a separate surface,
deferred to M3.

## 6. Firewall — egress only

- nftables `inet` table; `output` hook; **uid-owner** match. Always: accept `oif lo`; accept
  `meta skuid 1002` (pedagog broker egress). The student (`skuid 1001`) is filtered per mode.
- **One internal model for all modes:** an ordered list of `{action, cidr}` + a default verdict —
  `default`→`([], drop)`, `block`→`(allow as accept, drop)`, `open`→`(block as drop, accept)`,
  `custom`→`(rules, drop)`. **First-match wins.**
- **The translator is one small one-directional function:** emit `meta skuid 1001 ip daddr <cidr>
  <accept|drop>` per rule (in order), then the terminal `meta skuid 1001 <default>`. No nft-syntax
  parsing, no `nftables` crate (JSON-only, for live use). (Named sets are a later optimization for
  large lists.)
- Loaded at boot by `nft -f /pedagog/config/nftables.conf` in `/etc/runit/1` while PID 1 holds
  `CAP_NET_ADMIN`. What happens next depends on **`PEDAGOG_USER_TYPE`**:
  - **`student` (default):** PID 1 execs `runsvdir` via `setpriv --bounding-set=-net_admin,-setpcap`.
    Once `net_admin` leaves the bounding set, every later `execve` re-derives its capabilities from
    that set, so **no process — not even a root one — can alter nft for the session** (a complete
    lock, not just blocking re-grant). `setpcap` is dropped too so the bounding set can't be widened
    again; `setuid`/`setgid` stay so `chpst` can drop services to their uids.
  - **`instructor`:** PID 1 execs `runsvdir` **without** dropping from the bounding set, and the
    editor service launches code-server as uid 1003 with `net_admin` as an **ambient** capability
    (`setpriv --reuid 1003 --regid 1003 --init-groups --inh-caps=+net_admin --ambient-caps=+net_admin`).
    The ambient cap survives the uid drop and is inherited by the editor's child processes, so the
    instructor can edit the live firewall from a terminal (spiked, PASS). Use only for non-exam
    authoring/test sessions. The instructor opens `/pedagog/student` (group-shared via `pedagogc`) to
    see exactly the student environment.
- The base image bakes a **fail-closed default** `nftables.conf` (all student egress dropped), so the
  bare base always boots safely; `pedagog image network compile` overwrites it per assignment. A
  missing manifest compiles to `default`; a malformed one is a build error (so authors see it).
- The container must run with `--cap-drop=ALL --cap-add=NET_ADMIN --cap-add=SETPCAP --cap-add=SETUID
  --cap-add=SETGID` (NET_ADMIN to load nft, SETPCAP to drop it from the bounding set, SETUID/SETGID
  for `chpst`) — a flag set on the `codebox` job, wired when we touch doc 08. `setpriv` is the
  dedicated Wolfi package: busybox's applet lacks `--bounding-set`.
- **Ingress is not filtered in v1** (topology governs reachability; the daemon control-port concern
  is handled at M3 via binding + the egress drop).

## 7. Minimal daemon

- `pedagog-daemon`, runs as **`pedagog` (uid 1002)**, `--cap-drop=ALL`, **no `NET_ADMIN`** (least
  privilege; it's the network-facing broker, so we shrink the blast radius). It is **not** on the
  firewall path.
- Registered as a runit service by `pedagog image daemon init` → `/etc/service/pedagog/run` execs
  `chpst -u pedagog pedagog-daemon …`.
- Unix socket `/run/pedagog.sock`, `0660 root:pedagogc` (clients are `pedagogc` members:
  student/instructor); authorizes peers via **`SO_PEERCRED`** (map peer uid → role).
- **Container token** injected at runtime (tmpfs, e.g. `/run/pedagog/token` `0640 root:pedagog`);
  stubbed for now, real source = orchestrator at session create.
- **v1 minimal scope:** start, open the socket, answer a basic status/ping.
- **Deferred (M3):** liveness heartbeat to the CP, the CP-only token-authenticated control endpoint
  (`EndSession`/`UpdateDeadline`), submit/archive brokering.

## 8. Rust workspace

- `pedagog-core` — pure domain: versioned `Manifest` (`magic_migrate` + `semver`); `NetworkConfig`
  enum (`Default` | `Block{allow}` | `Open{block}` | `Custom{rules}`) over `IpNet`; `Rule{action,
  to}`, `Role`; the nft renderer; validation; **no I/O** (`thiserror`).
- `pedagog-cli` — the `pedagog` binary (`clap`); `image` verbs; `miette` for diagnostics at the
  boundary.
- `pedagog-daemon` — the daemon (`tokio`); socket server.
- `pedagog-proto` — socket/control message types (added with the daemon's real functions).
- Built as **static musl** binaries so they run on the Wolfi base; baked in root-owned, image verbs
  not student-executable.

## 9. Sequencing (increments)

- **A (finishes M2):** workspace skeleton + `pedagog-core` versioned manifest + `[network]` types +
  nft renderer (**done**) + `pedagog image network` (manifest → render → write
  `/pedagog/config/nftables.conf`), wired into `/etc/runit/1` + cap drop; lock raw `apk` away from
  `student`. Rootless-`nft` uid-owner egress spike **confirmed PASS**.
- **B:** `pedagog image build` orchestration + `toolchain` + `pkg`.
- **C:** minimal daemon + `daemon init` + socket (`SO_PEERCRED`).
- **M3:** daemon heartbeat/control/submit vs a CP stub; student CLI verbs.

## 10. Risks

- ~~**Rootless in-container nftables** (netns/module quirks under pasta)~~ — spike done, uid-owner
  egress loads in the codebox.
- **Static musl Rust on Wolfi** — verify the binary runs under `--cap-drop=ALL` + `NET_ADMIN`.
- **`SO_PEERCRED`** uid→role mapping correctness.

## 11. Open / to refine

- **toolchain definition schema** — may install pkgs and/or run commands (user to define).
- **Where `build` runs** — host-side wrapper vs inside a build container — pairs with CP/registry
  (doc 02 §5.4).
- **Container-token injection** mechanism (orchestrator → tmpfs) — M3.
