# 10 — Design (Draft): `pedagog image build`, `toolchain`, `pkg` (Increment B)

> **Date:** 2026-06-22
> **Status:** **Draft** — proposed shape for redline. Implements
> [`10-prompt-build-toolchain-pkg.md`](./10-prompt-build-toolchain-pkg.md). Follows
> [`05-design-code-conventions.md`](./05-design-code-conventions.md); builds on
> [`09-design-cli-manifest-daemon.md`](./09-design-cli-manifest-daemon.md).

## 1. Shape

`pkg` is the low-level primitive (wraps `apk`); `toolchain` is a named lifecycle bundle on top; `build`
is the declarative driver that reads the manifest and calls both. All three are instructor/root-only,
idempotent, and run **in-container at image-build time** (`RUN pedagog image build`).

```
pedagog.toml [toolchains]/[packages]      registered defs: /pedagog/config/toolchain/<id>.toml
        └───────────────┬───────────────────────────────┘
                 pedagog image build  (root; idempotent)
                        │  resolve toolchain ids → defs; apk add packages; run install; verify
                        ▼
        /pedagog/config/build.toml   ← resolved state (the one ledger); `build --info` prints it
```

## 2. Toolchain definition

A registered TOML describing the toolchain's lifecycle as two phases — `[install]` and `[uninstall]`.
Command fields are **arrays of shell commands** run in order. The schema is **versioned** like the
manifest (`version` + a `v0` module, migratable via `magic_migrate`); `version` is validated against
`^0.1`.

```toml
version     = "0.1.0"
id          = "rust"
description = "Rust 1.88.0 via rustup (shared install under /opt/rust)"

[install]
# pkg: the essentials apk provides (rustup's downloader + a native linker/libc).
pkg = ["bash", "curl", "gcc", "musl-dev"]
# cmd: the shell-script install, pinned + non-interactive, into a shared dir.
cmd = [
  "RUSTUP_HOME=/opt/rust CARGO_HOME=/opt/rust curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain 1.88.0 --profile minimal",
]
# verify: assert it actually works.
verify = ["CARGO_HOME=/opt/rust /opt/rust/bin/cargo --version"]

# env: vars exposed to the student session (see §6.1). Emitted into the shared
# env.sh in id order; PATH-like values go through a prepend-if-absent guard.
[env]
CARGO_HOME = "/opt/rust"
PATH       = "/opt/rust/bin:$PATH"

[uninstall]
# cmd: tear down what the install put on disk.
cmd = ["rm -rf /opt/rust"]
```

All fields except `version` and `id` are optional; a pure-apk toolchain just sets `[install].pkg`.

- **Install:** `apk add [install].pkg` → run `[install].cmd` → run `[install].verify` → (re)generate
  `env.sh`. Any non-zero exit **fails the build** (fail-fast; the author sees it). No rollback in v1.
- **Remove:** run `[uninstall].cmd` → **dependency-gated** `apk del` of `[install].pkg` → drop the
  ledger id → regenerate `env.sh`. The packages and env to act on come from the **registered defn**
  (the ledger stores only ids, §3), which is why a clean uninstall assumes the defn is present. Flags
  adjust this (§5).

## 3. Resolved state — `/pedagog/config/build.toml`

One ledger (`root:pedagog`), the single source of truth for "what's installed". `build --info` prints
it verbatim. It records only **ids** — the directly-installed packages and the installed toolchain
ids; a toolchain's packages/env come from its **registered defn**, not a ledger snapshot.

```toml
additional_packages = ["ripgrep", "jq"]   # installed directly via `pkg install`
toolchains          = ["rust", "go"]      # installed toolchain ids
```

`pkg installed` lists `additional_packages` plus, for each installed toolchain id, that defn's
`[install].pkg`; `toolchain list --installed` reads `toolchains`. The dependency-gating that lets
`pkg remove` / toolchain purge avoid yanking a shared package is computed from the **defns** of the
installed toolchains (§5.1), not from the ledger.

## 4. CLI — `pkg` (the apk wrapper)

| Verb | Behavior |
|---|---|
| `pkg install [PKGS…]` | `apk add` each; record in `additional_packages`. Idempotent (already-present is a no-op). |
| `pkg remove [PKGS…]` | `apk del` (dependency-gated, §5.1), then drop from `additional_packages`. Removes a package **even if `pkg` didn't install it**, but **refuses** any package an installed toolchain depends on, naming it. |
| `pkg installed` | List **every** installed package — directly-installed and toolchain-owned — annotating toolchain-required ones with the toolchain(s), e.g. `curl (go, rust)`. |

## 5. CLI — `toolchain`

Defns live in **one registry dir, `/pedagog/config/toolchain/`**. Immutability is by file mode: a
defn whose file is **read-only** (`0444`) is *immutable* — the base image bakes its curated defns that
way; instructor `register` writes `0644`. The verbs run as root, so this is a CLI-honored convention,
not an OS guarantee. One file per `id`, so there is no shadowing.

| Verb | Behavior |
|---|---|
| `toolchain register [DEFN.toml…] [--force]` | Parse+validate each as a def; copy to `/pedagog/config/toolchain/<id>.toml` (target filename is the def's `id`, not the source name), mode `0644`. **Refuses if the target is immutable** (read-only/base). Refuses to overwrite an existing *custom* defn without `--force`. |
| `toolchain unregister [PATH\|ID…]` | Remove the registered def file (by path or `id`). **Errors on an immutable (base) defn.** Warns if currently installed. |
| `toolchain list [-a/--all \| -i/--installed (default) \| -u/--uninstalled]` | `installed` = ids in `build.toml`; `uninstalled` = registered-but-not-installed; `all` = both. Flags each row's **origin** (`immutable`/base vs custom). |
| `toolchain install [IDS…]` | For each: resolve the registered def, run the install lifecycle (§2), then record the id in `toolchains` **only after `verify` passes** — a failed install isn't marked installed, so a re-run retries it (no rollback of `cmd`'s on-disk effects in v1). Already-installed = no-op (skips command re-runs). |
| `toolchain verify (IDS… \| -a/--all)` | For each: check every `[install].pkg` is installed (apk query), then run `[install].verify` commands. Reports pass/fail per toolchain; a missing package fails before the commands run. `--all` verifies every installed toolchain. Read-only. |
| `toolchain remove [IDS…] [--no-purge] [--no-cmd] [--dry-run] [--forget]` | Default: run `[uninstall].cmd` → dependency-gated `apk del` of the defn's `[install].pkg` → drop the ledger id → regenerate `env.sh`. (`uninstall` is an alias of `remove`.) Reads the **registered defn** for the cmd/packages/env; `--forget` works without it, but a default remove **errors if the defn is missing**, pointing at `--no-cmd`/`--forget`. |

**`remove` flags:**

| Flag | Effect |
|---|---|
| *(default)* | uninstall cmd → dependency-gated purge → drop ledger entry |
| `--no-purge` | keep packages; still run uninstall cmd + drop entry |
| `--no-cmd` | skip the uninstall cmd; still purge (gated) + drop entry |
| `--dry-run` | print the plan; change nothing |
| `--forget` | drop the ledger entry only — run nothing, remove nothing |

### 5.1 Dependency-tracked package removal

Both `pkg remove` and a toolchain purge gate every `apk del` on whether the package is still needed.
A package's **requirers** are computed from the **registered defns of the installed toolchains** (the
ledger holds only ids, §3):

- every **installed toolchain** whose defn lists it in `[install].pkg`, and
- the **assignment itself** — `additional_packages` in the ledger (and the manifest, §8).

So the CLI resolves the installed ids → defns, then calls the pure requirer/purgeable functions in
`pedagog-core`. A package is removed only when **no requirer remains** (excluding the toolchain being
removed); removing `rust` won't yank `bash`/`curl` if another installed toolchain or the assignment
still needs them — those are reported as *kept*.

`pkg remove X` (§4) uses the same check: it will remove `X` **even if `pkg` didn't install it**, but
**refuses** if an installed toolchain depends on `X`, naming that toolchain.

## 6. CLI — `build`

`build [CONFIG=/pedagog/source/pedagog.toml] [--info]`:

- Read the manifest's `[image].toolchains` and `[image].additional_packages` (§8).
- Install each listed toolchain (§2) and each listed package (§4), skipping anything already recorded
  in `build.toml` — so re-running converges without re-running side-effectful commands.
- `--info` prints `build.toml` and exits (no changes).

v1 is **additive** (installs what the manifest asks for). Declarative *pruning* — removing things
present in `build.toml` but absent from the manifest — is deferred (noted in §11) to avoid surprising
removals early.

### 6.1 Toolchain env — `/pedagog/config/env.sh`

A shared install (e.g. `/opt/rust`) needs `CARGO_HOME`/`PATH` set in the *student's* session. Each
defn's `[env]` (§2) declares those vars. `install`/`remove`/`build` **regenerate a single managed
file**, `/pedagog/config/env.sh`, from the `[env]` of all installed toolchains (in id order):

```sh
# generated — do not edit
_pedagog_path_prepend() { case ":$PATH:" in *":$1:"*) ;; *) PATH="$1:$PATH" ;; esac; }
export CARGO_HOME="/opt/rust"
_pedagog_path_prepend "/opt/rust/bin"
export PATH
```

- Plain vars are emitted as `export KEY="VALUE"` (double-quoted, so values expand when sourced).
- A `PATH` entry goes through the `prepend-if-absent` guard, so sourcing twice (login profile **and**
  the code-server launch) doesn't duplicate entries.
- **Both** the login profile and the code-server launch source `env.sh`, so the editor, its integrated
  terminal, and SSH shells all see the same env regardless of login-shell semantics.

One regenerated file (not per-toolchain `env.d/*.sh` fragments) keeps the `PATH` merge simple and
leaves no stale fragments on removal. This is its own increment (**B3c**, §10).

## 7. Execution & idempotency

- Commands run as **root** via `sh -c "<cmd>"`, env inherited from the build, **fail-fast** (first
  non-zero exit aborts), with stdout/stderr streamed so authors see progress and errors.
- **Idempotency** is ledger-based: install verbs check `build.toml` and **skip** already-recorded
  toolchains/packages, so re-running `build` (or an instructor re-run over SSH) is a no-op rather than
  re-executing install commands. `apk add` is itself idempotent.

## 8. Manifest growth

Image-build config lives under **`[image]`** (separate from future assignment-level tables). Add two
**optional list fields** there; keep `deny_unknown_fields`. Additive within `^0.1` (no minor bump).

```toml
[image]
toolchains = ["rust"]                 # ids of registered toolchain defs
additional_packages = ["ripgrep", "jq"]  # extra apk packages

[image.network]
mode = "default"
```

Both lists default to empty. `network` stays required within `[image]`.

## 9. Rust structure (per doc 05)

- **`pedagog-core` (pure):** the versioned `Toolchain` def type (`v0` + `magic_migrate`, like
  `Manifest`) with its `[env]`; the manifest `[image]` types; the `build.toml` `BuildState` (now just
  `additional_packages` + `toolchains` ids). The **dependency-gating** is pure functions fed the
  installed toolchains' **defns** (`requirers_of(pkg, &[Toolchain], …)`,
  `purgeable_on_remove(removing, remaining, …)`) — the CLI resolves ids → defns from the registry,
  calls these, then performs the apk side effects. `BuildState` itself just adds/drops ids + serializes.
  No I/O.
- **`pedagog-cli`:** the verbs. **Each command is its own directory** — `image/<cmd>/mod.rs` is the
  clap surface (the `Subcommand` + a thin `run()` that dispatches) and `image/<cmd>/ops.rs` is the
  logic; shared helpers (`apk`, `ledger`, `manifest`, `registry`, `shell`) sit at `image/` root.
  Side effects sit **behind traits** so the orchestration is unit-testable with fakes; the real impls
  shell out:
  - **`PackageManager`** (in `image::apk`, impl `Apk`) — `add`/`del` primitives + `is_installed` (apk
    query, for `verify`); owns the shared `pkg install`/`remove` logic as default methods that
    **mutate the `BuildState`** they're handed.
  - **`Shell`** (in `image::shell`, impl `Sh`) — runs `install.cmd`/`uninstall.cmd`/`verify` scripts
    via `sh -c`, fail-fast, with a `run_all` default method.

  `toolchain`'s lifecycle uses *both* traits, so its `ops` are free functions generic over them
  (not default methods on one trait). The command loads the ledger, passes it in, saves it. Registered
  defs are read/written via `image::registry` (one dir; immutability = the file's read-only mode;
  resolves an id → defn, lists with origin); ledger I/O via `image::ledger`; the `env.sh` regeneration
  (§6.1) is its own helper. `miette` at the boundary.

## 10. Sequencing (small increments within B)

- **B1** — manifest `[image]` types + `Toolchain` def + `build.toml` types in `pedagog-core` (pure,
  tested).
- **B2** — `pkg` (apk wrapper + ledger) behind the `PackageManager` trait. *(done)*
- **B3a** — the toolchain **registry** (`image/registry.rs`: load/save/list `<id>.toml`) +
  `register`/`unregister`/`list`. No command execution, so no new side-effect traits — trivially
  tested.
- **B3b** — `install`/`verify`/`remove`, the execution path. Adds the `Shell` trait (run shell
  scripts) and `PackageManager::is_installed` (apk query for verify); `toolchain` ops are free
  functions generic over both traits (the lifecycle uses package + shell side effects together).
  Gating recomputed from installed defns (§5.1).
- **B3c** — toolchain **env** (§6.1): `[env]` in the defn schema + regenerating `/pedagog/config/env.sh`
  on install/remove, and sourcing it from the login profile + code-server launch.
- **B4** — `build` orchestration + `--info`; wire `RUN pedagog image build` into a per-assignment
  image (and/or the base as a no-op).

## 11. Open / to refine

- **Declarative pruning in `build`** (§6) — additive-only for v1; revisit whether `build` should also
  remove things dropped from the manifest.
- **`env.sh` wiring details** (§6.1) — the exact profile/launch hook points (which login file, where in
  the code-server launch) get nailed down in B3c against the running image.
