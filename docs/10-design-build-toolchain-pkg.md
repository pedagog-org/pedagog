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
Command fields are **arrays of shell commands** run in order.

```toml
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

[uninstall]
# cmd: tear down what the install put on disk.
cmd = ["rm -rf /opt/rust"]
```

All fields except `id` are optional; a pure-apk toolchain just sets `[install].pkg`.

- **Install:** `apk add [install].pkg` → run `[install].cmd` → run `[install].verify`. Any non-zero
  exit **fails the build** (fail-fast; the author sees it). No rollback in v1.
- **Remove:** run `[uninstall].cmd` → **dependency-gated** `apk del` of `[install].pkg` → drop the
  ledger entry. Flags adjust this (§5).

## 3. Resolved state — `/pedagog/config/build.toml`

One ledger (`root:pedagog`), the single source of truth for "what's installed". `build --info` prints
it verbatim.

```toml
# toolchains installed by `toolchain install` / `build`, with the packages each brought in
[toolchains.rust]
packages = ["rust", "cargo"]

# packages installed directly via `pkg install` (not owned by a toolchain)
[packages]
installed = ["ripgrep", "jq"]
```

`pkg installed` reads `[packages].installed`; `toolchain list --installed` reads the `[toolchains.*]`
keys. This lets `pkg remove` refuse to touch toolchain-owned or system packages, and lets `--purge`
know exactly which packages a toolchain brought in.

## 4. CLI — `pkg` (the apk wrapper)

| Verb | Behavior |
|---|---|
| `pkg install [PKGS…]` | `apk add` each; record in `[packages].installed`. Idempotent (already-present is a no-op). |
| `pkg remove [PKGS…]` | `apk del` (dependency-gated, §5.1), then drop from `[packages].installed`. Removes a package **even if `pkg` didn't install it**, but **refuses** any package an installed toolchain depends on, naming it. |
| `pkg installed` | List `[packages].installed`. |

## 5. CLI — `toolchain`

| Verb | Behavior |
|---|---|
| `toolchain register [DEFN.toml…]` | Validate it parses as a def; copy to `/pedagog/config/toolchain/<id>.toml`. |
| `toolchain unregister [PATH\|ID…]` | Remove the registered def file (by path or `id`). Warns if currently installed. |
| `toolchain list [-a/--all \| -i/--installed (default) \| -u/--uninstalled]` | `installed` = keys in `build.toml`; `uninstalled` = registered-but-not-installed; `all` = both. |
| `toolchain install [IDS…]` | For each: resolve the registered def, run the install lifecycle (§2), record `[install].pkg` under `[toolchains.<id>]`. Already-installed = no-op (skips command re-runs). |
| `toolchain verify (IDS… \| -a/--all)` | For each: check every `[install].pkg` is installed (apk query), then run `[install].verify` commands. Reports pass/fail per toolchain; a missing package fails before the commands run. `--all` verifies every installed toolchain. Read-only. |
| `toolchain remove [IDS…] [--no-purge] [--no-cmd] [--dry-run] [--forget]` | Default: run `[uninstall].cmd` → dependency-gated `apk del` of the toolchain's packages → drop the ledger entry. (`uninstall` is an alias of `remove`.) |

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
A package's **requirers** are:

- every **installed toolchain** that lists it in `[install].pkg`, and
- the **assignment itself** — the manifest's `[image].additional_packages` (§8).

A package is removed only when **no requirer remains** (excluding the toolchain being removed). So
removing `rust` won't yank `bash`/`curl` if another installed toolchain or the assignment still needs
them; those are reported as *kept*.

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

- **`pedagog-core` (pure):** the `Toolchain` def type + parse/validate; the manifest `[image]` types;
  the `build.toml` resolved-state types (serde). `BuildState` also owns the **dependency-gated removal
  logic** (`remove_package` errors if a toolchain needs it; `remove_toolchain` returns the packages now
  safe to purge) — pure, so the CLI just calls it then performs the apk side effects. No I/O.
- **`pedagog-cli`:** the verbs. Side effects (apk, command exec, filesystem) sit **behind traits** so
  the orchestration is unit-testable with fakes; a real impl shells out. `miette` at the boundary.

## 10. Sequencing (small increments within B)

- **B1** — manifest `[toolchains]`/`[packages]` types + `Toolchain` def + `build.toml` types in
  `pedagog-core` (pure, tested).
- **B2** — `pkg` (apk wrapper + ledger), with the exec/apk traits.
- **B3** — `toolchain` register/unregister/list, then install/remove (uses B2).
- **B4** — `build` orchestration + `--info`; wire `RUN pedagog image build` into a per-assignment
  image (and/or the base as a no-op).

## 11. Open / to refine

- **Declarative pruning in `build`** (§6) — additive-only for v1; revisit whether `build` should also
  remove things dropped from the manifest.
- **Toolchain env / PATH exposure** — a shared install like `/opt/rust` needs `CARGO_HOME`/`PATH` set
  in the *student's* session for `cargo` to work. The def's `cmd` puts it on disk; how the resulting
  env reaches the editor (profile.d? a manifest `[env]`?) is unspecified here — likely its own step.
- **Where defs come from for the base vs per-assignment image** — registered at base-build time, or
  shipped by the instructor alongside `pedagog.toml`.
