# 10 — Prompt: `pedagog image build`, `toolchain`, `pkg` (Increment B)

> **Status:** Draft — capturing scope; design under discussion in
> [`10-design-build-toolchain-pkg.md`](./10-design-build-toolchain-pkg.md).

## Ask

Implement **Increment B** from [`09-design-cli-manifest-daemon.md`](./09-design-cli-manifest-daemon.md)
§9: the declarative **`pedagog image build`** orchestration plus the **`toolchain`** and **`pkg`**
verbs. These are the instructor/root-only authoring primitives that turn a `pedagog.toml` into a
provisioned per-assignment image.

From doc 09 §5, the intended surface:

- `build [CONFIG=/pedagog/source/pedagog.toml]` — declarative; orchestrates the primitives below from
  the manifest. `--info` prints the registered `build.toml`.
- `toolchain list [-a/--all | -i/--installed (default) | -u/--uninstalled]`,
  `install [TOOLCHAINS…]` / `remove [TOOLCHAINS…]` (`uninstall` = alias of `remove`),
  `register [DEFN.toml…]` (copies a def into `/pedagog/config/toolchain/`),
  `unregister [PATH|ID…]`.
- `pkg installed` / `install [PKGS…]` / `remove [PKGS…]` — wraps `apk`; tracks what it installed.

Constraints carried over from doc 09:

- All verbs **instructor/root-only**, **idempotent**, and timing-agnostic (run at build time or by an
  instructor; must complete before students log in).
- Resolved state lives under `/pedagog/config/` (`root:pedagog`), e.g. registered toolchain defs in
  `/pedagog/config/toolchain/` and the resolved `build.toml`.

## Resolved decisions (2026-06-22)

- **Toolchain definition schema — two phases.** `[install]` carries `pkg` (essential apk packages),
  `cmd` (the shell-script install, run after packages), and `verify` (asserts it works); `[uninstall]`
  carries `cmd` (e.g. remove the install directory). Command fields are arrays of shell commands.
- **Dependency-tracked removal.** A package is `apk del`'d only when no requirer remains. Requirers =
  every installed toolchain listing it (`[install].pkg`) + the assignment's own
  `[image].additional_packages`.
  `pkg remove` may remove a package it didn't install, but **refuses** one an installed toolchain
  depends on.
- **`toolchain remove` flags:** default = run uninstall cmd → dependency-gated purge → drop ledger
  entry; `--no-purge` (keep packages), `--no-cmd` (skip uninstall cmd), `--dry-run` (preview),
  `--forget` (drop ledger entry only, run/remove nothing).
- **`toolchain verify (IDS… | -a/--all)`** — read-only health check: confirm the `[install].pkg` are
  installed, then run the def's `[install].verify` commands; report pass/fail. Takes an explicit list
  or `--all` (every installed toolchain). (The same `verify` also runs as the last step of `install`.)
- **Manifest growth — group image-build config under `[image]`.** `network` moves to
  `[image.network]`; add `[image].toolchains` and `[image].additional_packages` as bare lists (no
  `install` sub-field). Keep `deny_unknown_fields`; assignment-level tables get their own top-level
  namespace later. An unknown section/field is an error.
- **`build` runs in-container at image-build time** (`RUN pedagog image build`, as root). A host-side
  / CP wrapper is deferred until we touch the registry.
- **One ledger** — `/pedagog/config/build.toml` records installed toolchains (+ each one's packages)
  and directly-installed packages; `build --info` prints it. Idempotency is ledger-based (install
  verbs skip what's already recorded).

## B3 decisions (agreed in discussion)

- **Side-effect traits.** Add a `Shell` trait (run `sh -c <cmd>`, fail-fast, `run_all` helper) for the
  install/uninstall/verify scripts, and extend `PackageManager` with `is_installed` (apk query) for
  `verify`. `toolchain` ops are free functions generic over **both** traits (the lifecycle needs
  package + shell side effects together) — not default methods on one trait like `pkg` was.
- **register.** Target filename is the def's `id` (`<id>.toml`), not the source filename. Refuses to
  overwrite an existing registered def without `--force`.
- **install records after verify.** The ledger entry is written only after `verify` passes, so a
  failed install isn't marked installed and a re-run retries it (no rollback of `cmd` effects in v1).
- **remove + missing def.** Purge list comes from the ledger, so `--no-cmd`/`--forget` work without
  the def; a default `remove` errors if the registered def is missing (can't run `[uninstall].cmd`).
- **Sub-increments.** B3a = registry + `register`/`unregister`/`list` (no execution). B3b =
  `install`/`verify`/`remove` (the trait-backed execution path). B3c = toolchain env.

### B3 decisions — round 2

- **Versioned defn.** The toolchain definition is versioned like the manifest (`version` field, `v0`
  module, `magic_migrate`), validated against `^0.1`.
- **Ledger stores ids only.** `BuildState` becomes `additional_packages` + `toolchains` (ids); drop the
  per-toolchain package snapshot. A toolchain's packages/env come from its **registered defn**, read at
  remove/verify time — we assume uninstall is clean and the defn is present.
- **Gating still kept, computed from defns.** Keep the shared-package purge gate; the CLI resolves
  installed ids → defns and feeds pure requirer/purgeable functions in core. `pkg remove` gating
  (shipped in B2) is updated to consult installed defns too.
- **Env config.** Toolchain defn gets `[env]`. One regenerated `/pedagog/config/env.sh` (id order),
  `export KEY="VALUE"` for plain vars, a `prepend-if-absent` guard for `PATH`; sourced by both the
  login profile and the code-server launch. One file (not `env.d/*` fragments). This is B3c.
- **Base vs custom defns.** One registry dir (`/pedagog/config/toolchain/`). Base ships curated defns
  **read-only (`0444`) = immutable**; instructor `register` writes `0644`. `register`/`unregister`
  refuse immutable defns; `list` flags each row's origin. One file per id → no shadowing.

### B3 decisions — round 3 (redline; supersedes parts of rounds 1–2)

- **Terminology flip.** "Registry" now means the **ledger** (`ledger.toml`) — the in-config record of
  what's provisioned. The *directory of definition files* is the **toolchains directory**,
  `/pedagog/config/toolchains/`. (Earlier docs called that dir "the registry"; that's gone.)
- **Ledger renamed.** `/pedagog/config/build.toml` → **`/pedagog/config/ledger.toml`**; the core type
  `BuildState` → **`Ledger`** (module `pedagog_core::image::ledger`).
- **Ledger model = packages + toolchain install-state.** `additional_packages` plus `toolchains` as a
  map of **id → installed** (tracks every *registered* toolchain and whether it is installed) — not a
  bare list of installed ids. A toolchain's packages live **only in its def file**, never in the
  ledger. (Supersedes round 2's "ledger stores ids only".)
- **No mutability.** Drop the immutable/base-vs-custom (`0444`/`0644`) concept entirely. Toolchains
  have no mutability tracked. (Supersedes round 2's "base vs custom defns".)
- **register / unregister.** `register <file> [--overwrite]` copies the def into the toolchains dir as
  `<id>.toml`; refuses an already-registered id unless `--overwrite` (was `--force`). `unregister <id>
  [--force]` deletes the def and drops the ledger entry; **refuses if the toolchain is installed unless
  `--force`**.
- **`pkg remove` gating shipped now.** `pkg remove [PKGS…] [--force]` resolves the **installed**
  toolchains' defs from the toolchains dir and refuses to remove a package any of them depends on,
  unless `--force`. Implemented in this increment (not deferred to B3b).
- **`pkg installed`** still attributes toolchain-owned packages, e.g. `curl (python, rust)`, by
  resolving installed toolchains' defs.
- **Flag convention.** Any command that reads the **ledger** takes `--ledger` (default
  `/pedagog/config/ledger.toml`); any command that reads the **toolchains dir** takes `--toolchains`
  (default `/pedagog/config/toolchains`).

### B3 decisions — round 4

- **Ledger is versioned.** Like the manifest and toolchain def, the ledger carries a `version`
  validated against `^0.1` (`v0` module + `magic_migrate`). Because it is also written and constructed
  in-process, its `Default` stamps the current version (`0.1.0`) so a fresh ledger round-trips through
  the gate.
- **Toolchain id charset.** A toolchain `id` must be non-empty and contain only ASCII alphanumerics
  plus `.`, `-`, `_`. Enforced both at parse time (on the def's `id` field) and at path-construction
  time in the CLI — the latter because `id` becomes the `<id>.toml` filename, so the restriction also
  prevents a raw id from traversing out of the toolchains dir.
- **Default paths live in `pedagog-core`.** `DEFAULT_LEDGER`, `DEFAULT_TOOLCHAINS`, `DEFAULT_MANIFEST`,
  and `DEFAULT_RULESET` are defined in core (path data, no I/O, so core stays pure) and re-exported by
  the CLI, which keeps clap defaults pointing at one canonical definition.
- **Manifest renamed to `build.toml`.** The instructor's manifest defaults to
  `/pedagog/source/build.toml` (was `pedagog.toml`). Naming story: **`build.toml`** (declarative input)
  → `pedagog image build` → **`ledger.toml`** (resolved state). (Note: `build.toml` was briefly the
  ledger's filename earlier in this increment, before the ledger became `ledger.toml`; it is now the
  manifest.)

### B3 decisions — round 5 (B3b execution path)

- **`install` returns an outcome.** The lifecycle reports `Installed` vs `AlreadyInstalled`; the ledger
  flips to installed only after `verify` passes. `install [IDS…]` stops at the first failure, persisting
  the ledger with successes so far. A path-like argument (contains `/` or ends `.toml`) is registered
  first, but only with `--register` (and `--overwrite` flows into that registration); a bare id must
  already be registered.
- **`verify` is not fail-fast across toolchains.** It checks every target, prints `id: ok` / `id:
  FAILED: <reason>`, then errors if any failed (a missing def counts as a failure). `-a/--all` selects
  every installed toolchain; ids and `--all` are mutually exclusive.
- **`remove` takes `-a/--all` too** (every installed toolchain), with the same ids-xor-`--all` rule as
  `verify`.
- **Keep `--forget`.** Re-added after round 4's flag list dropped it: `--forget` is shorthand for
  `--no-cmd --no-purge` (just mark uninstalled). It is also the only `remove` form that works when the
  def is missing, since it needs nothing from it. A default `remove` errors on a missing def, pointing
  at `--forget`.
- **`Shell` returns stdout, no `run_all`.** `Shell::run` runs one `sh -c` command, captures stdout
  (returned) and streams stderr so progress stays visible; the lifecycle loops over a def's
  `cmd`/`verify` list itself so it can print each command. `PackageManager::is_installed` (apk `info
  -e`) backs `verify` and the purge gate.

## Open (to settle in design)

- Declarative pruning in `build` (additive-only for v1?).
- How a shared toolchain's env/PATH reaches the student session (likely a separate step).
- Where toolchain defs come from for the base vs per-assignment image.
