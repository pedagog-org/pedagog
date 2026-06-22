# 09 — Prompt: `pedagog` CLI, `pedagog.toml` Manifest, and Minimal Daemon

> **Date:** 2026-06-21
> **Status:** Prompt captured; design draft in [`09-design-cli-manifest-daemon.md`](./09-design-cli-manifest-daemon.md).
> Arises from M2 increment 3 (nftables egress) in [`07-design-first-steps.md`](./07-design-first-steps.md);
> builds on the network/security model in [`02-design-exam-system.md`](./02-design-exam-system.md) §4.2.
> (`pedagog restrict` was the first framing; superseded by the `pedagog image network` verbs below.)

## What the user specified

While planning the nftables egress isolation for the base image, the user added these requirements:

1. **The egress tables must be configurable, not a single baked ruleset.** Three instructor-facing
   modes (doc 02 §4.2): `none` (default, fail-closed), `allowlist`, `open`.
2. **The configuration is specified in a `pedagog.toml` file** — the instructor's assignment
   manifest, the source of truth (also holds e.g. `[archive]`, quotas).
3. **There will be a build step that takes in the instructor file (`pedagog.toml`) and then runs the
   build commands** — i.e. a pipeline that consumes the manifest and produces the per-assignment
   image.
4. **A `pedagog restrict` command could handle the configuration** — i.e. apply the restrictions
   from the manifest.

## Implied scope

- Define the `pedagog.toml` manifest (at least its `[network]` section now; sketch the rest).
- Define the build step (host/CP-side) that consumes the manifest and builds the image.
- Define `pedagog restrict`: a command that applies the manifest's restrictions. Because nftables
  must be applied at runtime (the netns only exists then), as root holding `CAP_NET_ADMIN`, and
  before the unprivileged `pedagog` daemon starts, `restrict` is a **privileged boot-time** step,
  not the daemon.

## CLI surface (user-specified, 2026-06-21)

Admin/authoring verbs live under **`pedagog image …`** (run at build *or* by an instructor over SSH —
timing-agnostic, must just happen before students log in; operations idempotent):

- `image build [CONFIG_FILE=/pedagog/source/pedagog.toml]` — declarative; orchestrates the primitives
  below from the manifest. `--info` prints the registered build config as TOML.
- `image toolchain` — `list` (`-a/--all`, `-i/--installed` default, `-u/--uninstalled`),
  `install [TOOLCHAINS…]`, `remove [TOOLCHAINS…]`, `register [DEFN.toml…]` (copies into
  `/pedagog/config/toolchain/`). A toolchain def may install pkgs and/or run commands (TBD).
- `image pkg` (renamed from `apt`) — `installed`, `install [PKGS…]`, `remove [PKGS…]`.
- `image daemon` — `init` (registers the runit service). **No `start`/`stop`** (runit owns lifecycle).
- `image network` — `status`; `rules list` / `rules add (-a/--allow | -b/--block, --at=INDEX|END)` /
  `rules remove [INDICES…]`.

### Refinements / decisions (2026-06-21)

- **Keep the package manager** (instructor/debug); never remove it. Deny it to `student` (raw `apk`
  not student-executable; `pedagog image pkg` is instructor/root-only). Reverses doc-07's "remove apt."
- **Egress-only for v1.** Don't filter ingress in-container — external reachability is governed by
  topology; the daemon control-port concern is handled at M3 by binding + the egress drop.
- **Config trees:** `/pedagog/source/` = instructor inputs (manifest + seeds, `instructor`-owned);
  `/pedagog/config/` = pedagog-managed resolved state (toolchain defs, registered build config,
  compiled rules), root/pedagog-owned, student: none.
- **Firewall is applied at boot, not build** (nft rules are netns/kernel state, not image files):
  build generates/validates/bakes the ruleset *file*; a privileged boot step loads it.
- **The daemon runs as `pedagog` (uid 1002), `--cap-drop=ALL`, no `NET_ADMIN`** — least privilege; so
  it can't load the firewall, which is why a separate privileged boot step does.
- **Ordering:** nftables is first-match-wins; specific allows must precede the terminal default-drop;
  `--at=END` = just before that default-drop.
- **Drop `daemon start`/`stop`** — `daemon` keeps only `init`; runit owns lifecycle.
- **`apt` → `pkg`.**
- **Lock the firewall at boot** (drop `CAP_NET_ADMIN` right after loading; instructor rule edits need
  a restart to take effect).
- **Egress-only is confirmed; document that rules apply to egress only.**
- **Rules are persisted as a file and loaded by a boot step** — like `/etc/nftables.conf` +
  `nftables.service`. The boot loader is the new `pedagog image network apply` verb.
- **Sequencing = Plan A** — bootstrap the Rust workspace now; build it as static musl binaries baked
  into the image.

### Follow-ups (2026-06-22) — session user type

- **Per-session "user type" job param** — `student` (default) or `instructor`. It is the single knob
  that selects the session identity; it replaces the earlier standalone firewall-lock parameter.
- **Instructor sessions** must be able to **edit the egress firewall**, and **code-server runs as the
  instructor** (uid 1003). The instructor still opens **`/pedagog/student`** (not their own tree) so
  they experience exactly what students experience. Mechanism (spiked, PASS): the editor is launched
  with `net_admin` as an **ambient** capability (`setpriv --reuid 1003 --regid 1003 --init-groups
  --inh-caps=+net_admin --ambient-caps=+net_admin`), so the editor *and its terminals* can run `nft`;
  boot does **not** drop `net_admin` from the bounding set for an instructor session.
- **Student sessions** are unchanged: editor runs as `student` (uid 1001) opening `/pedagog/student`,
  and boot locks the firewall (drops `net_admin`/`setpcap` from the bounding set).
- **`/pedagog/student`** is therefore group `pedagogc` with the setgid bit (`student:pedagogc 2770`)
  so the instructor uid (also in `pedagogc`) can open it; in a student exam only the student uid runs
  there, so isolation is unchanged in practice (no instructor-uid process exists in that session, and
  `pedagog` is not in `pedagogc`).
- **Editor runtime state** (`/var/lib/code-server/{data,cache,share}`) is likewise shared between the
  two uids via group `pedagogc` (setgid dirs, `0660`/`2770`); the extensions dir stays root-owned
  read-only.

### Follow-ups (2026-06-22) — `network` editing surface

- **No `rules add/remove`.** The instructor edits `pedagog.toml` directly in the editor; the CLI just
  provides what hand-editing can't do.
- **`network convert`** — rewrite the manifest's `[network]` into an equivalent `custom` rule list
  (`toml_edit`, preserve the rest of the file, validate by re-parsing into the typed `Manifest`), so
  the instructor can then hand-edit ordered rules. `open` mode converts by appending catch-all
  allow-all rules (its terminal accept).
- **`network load [--compile-only]`** — default renders the manifest and applies it **live**
  (`nft -f -`, no file write; needs `net_admin`, so instructor sessions only; does not touch the baked
  boot policy). `--compile-only` instead **writes** `nftables.conf` (build-time path; replaces the old
  standalone `compile` verb — Containerfile `RUN` updates to `network load --compile-only`).
- Keep **`network status`**. Drop the `rules` subcommand group entirely.

## Still open / to refine

- **toolchain definition schema** — may install pkgs and/or run commands (user to define).
- **Where `build` runs** — host-side wrapper vs inside a build container (pairs with CP/registry).
- **Container-token injection** mechanism (orchestrator → tmpfs) — M3.
