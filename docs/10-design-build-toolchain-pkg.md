# 10 — Design (Draft): `pedagog image build`, `toolchain`, `pkg` (Increment B)

> **Date:** 2026-06-23
> **Status:** **Draft** — redlined after B3a. Implements
> [`10-prompt-build-toolchain-pkg.md`](./10-prompt-build-toolchain-pkg.md). Follows
> [`05-design-code-conventions.md`](./05-design-code-conventions.md); builds on
> [`09-design-cli-manifest-daemon.md`](./09-design-cli-manifest-daemon.md).
>
> **Implemented so far:** `pkg` (install/remove/installed, with def-based remove gating);
> `toolchain register`/`unregister`/`list`; the `Ledger` type and the toolchains-directory helper.
> **Still planned:** `toolchain install`/`verify`/`remove` (B3b), toolchain env (B3c), `build` (B4).

## 1. Shape

`pkg` is the low-level primitive (wraps `apk`); `toolchain` is a named lifecycle bundle on top; `build`
is the declarative driver that reads the manifest and calls both. All three are instructor/root-only,
idempotent, and run **in-container at image-build time** (`RUN pedagog image build`).

```
build.toml [image].toolchains/.additional_packages    defs: /pedagog/config/toolchains/<id>.toml
        └───────────────┬─────────────────────────────────────────┘
                 pedagog image build  (root; idempotent)
                        │  resolve toolchain ids → defs; apk add packages; run install; verify
                        ▼
        /pedagog/config/ledger.toml   ← resolved state; `build --info` prints it
```

The instructor's manifest defaults to `/pedagog/source/build.toml`; `build` resolves it to the
`ledger.toml` resolved state. (The canonical default paths — manifest, ledger, toolchains dir,
ruleset — live as `pub const`s in `pedagog-core` and are re-exported by the CLI.)

Two locations under `/pedagog/config/` (`root:pedagog`):

- **the toolchains directory** `/pedagog/config/toolchains/` — one `<id>.toml` definition per registered
  toolchain (the recipes);
- **the ledger** `/pedagog/config/ledger.toml` — the record of what's provisioned.

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

[uninstall]
# cmd: tear down what the install put on disk.
cmd = ["rm -rf /opt/rust"]
```

All fields except `version` and `id` are optional; a pure-apk toolchain just sets `[install].pkg`.

The `id` must be non-empty and contain only ASCII alphanumerics plus `.`, `-`, `_` (validated at parse
time). Since `id` becomes the `<id>.toml` filename, this also keeps it filename-safe — no path
separators, so a def (or a CLI id arg) cannot traverse out of the toolchains dir.

> **`[env]` is deferred to B3c** and is **not yet in the schema** (`deny_unknown_fields` would reject
> it today). When added it will carry the vars exposed to the student session (see §6.1).

- **Install (B3b):** `apk add [install].pkg` → run `[install].cmd` → run `[install].verify`. Any
  non-zero exit **fails the build** (fail-fast; the author sees it). No rollback in v1.
- **Remove (B3b):** run `[uninstall].cmd` → **dependency-gated** `apk del` of `[install].pkg` → mark
  the toolchain uninstalled in the ledger. The packages to act on come from the **def file** (the
  ledger never stores them, §3), so a clean uninstall assumes the def is present. Flags adjust this
  (§5).

## 3. Resolved state — `/pedagog/config/ledger.toml`

The ledger (`root:pedagog`) is the record of "what's provisioned". `build --info` prints it verbatim.
It is **versioned** like the manifest (`version` gated against `^0.1`, `v0` + `magic_migrate`); a fresh
ledger's `Default` stamps the current version so it round-trips. It records:

- `version` — the ledger schema version;
- `additional_packages` — packages installed directly via `pkg install`;
- `toolchains` — a table of **every registered toolchain id → whether it is installed**.

A toolchain's packages/env are **never** snapshotted here; they live in the def file (§2) and are read
back at remove/verify/list time.

```toml
version = "0.1.0"
additional_packages = ["ripgrep", "jq"]

[toolchains]
rust = true     # registered and installed
go   = false    # registered, not yet installed
```

`pkg installed` lists `additional_packages` plus, for each **installed** toolchain, that def's
`[install].pkg` (attributing the owner). The dependency-gating that lets `pkg remove` avoid yanking a
shared package is computed from the **defs of the installed toolchains** (§5.1), not from the ledger.

## 4. CLI — `pkg` (the apk wrapper)

| Verb | Behavior |
|---|---|
| `pkg install [PKGS…]` | `apk add` each; record in `additional_packages`. Idempotent (already-present is a no-op). |
| `pkg remove [PKGS…] [--force]` | `apk del`, then drop from `additional_packages`. Removes a package **even if `pkg` didn't install it**, but **refuses** any package an installed toolchain depends on (naming it), unless `--force` (§5.1). |
| `pkg installed` | List **every** installed package — directly-installed and toolchain-owned — annotating toolchain-owned ones with the toolchain(s), e.g. `curl (go, rust)`. |

`pkg remove` and `pkg installed` read the toolchains dir (to resolve installed defs); all three read
the ledger (§ flag convention).

## 5. CLI — `toolchain`

Defs live in **one directory, `/pedagog/config/toolchains/`** — one file per `id` (`<id>.toml`), so
there is no shadowing. There is **no mutability concept**: any registered def can be overwritten or
unregistered (subject to the install guard below).

| Verb | Status | Behavior |
|---|---|---|
| `toolchain register <FILE> [--overwrite]` | **done** | Parse+validate `FILE` as a def; copy it to `/pedagog/config/toolchains/<id>.toml` (target filename is the def's `id`, not the source name) and record the id in the ledger as not-installed. **Refuses to overwrite an already-registered id** without `--overwrite`. |
| `toolchain unregister <ID> [--force]` | **done** | Delete the def file and drop the id from the ledger. **Refuses if the toolchain is installed** unless `--force`. |
| `toolchain list` | **done** | List registered toolchains, annotating each as `installed` or `registered`. |
| `toolchain install [IDS…]` | B3b | For each: resolve the def, run the install lifecycle (§2), then set `installed = true` in the ledger **only after `verify` passes** — a failed install isn't marked installed, so a re-run retries it. Already-installed = no-op. |
| `toolchain verify (IDS… \| -a/--all)` | B3b | For each: check every `[install].pkg` is installed (apk query), then run `[install].verify`. Reports pass/fail; a missing package fails before the commands run. Read-only. |
| `toolchain remove [IDS…] [--no-purge] [--no-cmd] [--dry-run]` | B3b | Run `[uninstall].cmd` → dependency-gated `apk del` of the def's `[install].pkg` → set `installed = false`. Reads the **def file** for the cmd/packages; a default remove **errors if the def is missing**, pointing at `--no-cmd`. (`uninstall` is an alias of `remove`.) |

**`remove` flags (B3b):**

| Flag | Effect |
|---|---|
| *(default)* | uninstall cmd → dependency-gated purge → mark uninstalled |
| `--no-purge` | keep packages; still run uninstall cmd + mark uninstalled |
| `--no-cmd` | skip the uninstall cmd; still purge (gated) + mark uninstalled |
| `--dry-run` | print the plan; change nothing |

### 5.1 Dependency-tracked package removal

`pkg remove` (done) and a toolchain purge (B3b) gate every `apk del` on whether the package is still
needed. A package's **requirers** are computed from the **defs of the installed toolchains** (the
ledger holds no package lists, §3): every **installed toolchain** whose def lists it in `[install].pkg`.

So the CLI resolves the installed ids → defs from the toolchains dir, then calls the pure
`toolchains_requiring(pkg, &[Toolchain])` function in `pedagog-core`. A package is removed only when no
requirer remains; `--force` overrides the refusal. For a toolchain purge (B3b), the toolchain being
removed is excluded from the requirer set and the assignment's own `additional_packages` is added to
it, so removing `rust` won't yank `bash`/`curl` if another installed toolchain or the assignment still
needs them.

### Flag convention

Every verb that reads the **ledger** takes `--ledger` (default `/pedagog/config/ledger.toml`); every
verb that reads the **toolchains dir** takes `--toolchains` (default `/pedagog/config/toolchains`).

## 6. CLI — `build` (B4)

`build [CONFIG=/pedagog/source/build.toml] [--info]`:

- Read the manifest's `[image].toolchains` and `[image].additional_packages` (§8).
- Install each listed toolchain (§2) and each listed package (§4), skipping anything already recorded
  in the ledger — so re-running converges without re-running side-effectful commands.
- `--info` prints `ledger.toml` and exits (no changes).

v1 is **additive** (installs what the manifest asks for). Declarative *pruning* — removing things
present in the ledger but absent from the manifest — is deferred (noted in §11) to avoid surprising
removals early.

### 6.1 Toolchain env — `/pedagog/config/env.sh` (B3c)

A shared install (e.g. `/opt/rust`) needs `CARGO_HOME`/`PATH` set in the *student's* session. Each
def's `[env]` (added in B3c, §2) declares those vars. `install`/`remove`/`build` **regenerate a single
managed file**, `/pedagog/config/env.sh`, from the `[env]` of all installed toolchains (in id order):

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
leaves no stale fragments on removal.

## 7. Execution & idempotency

- Commands run as **root** via `sh -c "<cmd>"`, env inherited from the build, **fail-fast** (first
  non-zero exit aborts), with stdout/stderr streamed so authors see progress and errors.
- **Idempotency** is ledger-based: install verbs check `ledger.toml` and **skip** already-recorded
  toolchains/packages, so re-running `build` (or an instructor re-run over SSH) is a no-op rather than
  re-executing install commands. `apk add` is itself idempotent.

## 8. Manifest growth

Image-build config lives under **`[image]`** (separate from future assignment-level tables). Two
**optional list fields** there; keep `deny_unknown_fields`. Additive within `^0.1` (no minor bump).

```toml
[image]
toolchains = ["rust"]                    # ids of registered toolchain defs
additional_packages = ["ripgrep", "jq"]  # extra apk packages

[image.network]
mode = "default"
```

Both lists default to empty. `network` stays required within `[image]`.

## 9. Rust structure (per doc 05)

- **`pedagog-core` (pure):** the versioned `Toolchain` def type (`v0` + `magic_migrate`, like
  `Manifest`) with `valid_id` + parse-time id validation; the manifest `[image]` types; the **versioned**
  `Ledger` type in `image::ledger` (`version` + `additional_packages` + `toolchains: BTreeMap<String,
  bool>`, with add/remove-package, register/unregister-toolchain, `is_installed`, and TOML
  (de)serialization). Dependency-gating is the pure `package_dependencies(direct, &[Toolchain]) ->
  BTreeMap<&str, BTreeSet<&str>>` (each package → its owning toolchains); `toolchains_requiring` is a
  thin wrapper over it. The canonical default paths (`DEFAULT_MANIFEST`, `DEFAULT_LEDGER`,
  `DEFAULT_TOOLCHAINS`, `DEFAULT_RULESET`) are `pub const`s here too — path data, no I/O.
- **`pedagog-cli`:** the verbs. **Each command is its own directory** — `image/<cmd>/mod.rs` is the
  clap surface (the `Subcommand` + a thin `run()` that dispatches) and `image/<cmd>/ops.rs` is the
  logic; shared helpers (`apk`, `ledger`, `manifest`, `toolchains`, and `shell` in B3b) sit at
  `image/` root and re-export the core default-path consts for clap. Side effects sit **behind traits**
  where an external binary needs faking; the real impls shell out:
  - **`PackageManager`** (in `image::apk`, impl `Apk`) — `add`/`del` primitives (plus `is_installed`
    in B3b for `verify`); owns the shared `pkg install`/`remove` logic as default methods that
    **mutate the `Ledger`** they're handed.
  - **`Shell`** (in `image::shell`, impl `Sh`, B3b) — runs `install.cmd`/`uninstall.cmd`/`verify`
    scripts via `sh -c`, fail-fast.

  `image::toolchains` mirrors that primitive/accounting split as plain functions (no external binary to
  fake): `add`/`delete` are the filesystem primitives, `register`/`unregister` wrap them with the ledger
  accounting (and `resolve`/`list` round it out). `toolchain`'s lifecycle (B3b) will use *both* the
  package and shell traits. Ledger I/O via `image::ledger`. `miette` at the boundary.

## 10. Sequencing (small increments within B)

- **B1** — manifest `[image]` types + `Toolchain` def + ledger types in `pedagog-core` (pure, tested).
  *(done)*
- **B2** — `pkg` (apk wrapper + ledger) behind the `PackageManager` trait. *(done)*
- **B3a** — the toolchains-directory helper (`image/toolchains.rs`: `register`/`unregister`/`list`/
  `resolve` of `<id>.toml`) + `toolchain register`/`unregister`/`list`; the ledger remodel + rename
  (`Ledger`, `ledger.toml`, id→installed map); `pkg remove` gating recomputed from installed defs.
  *(done)*
- **B3b** — `toolchain install`/`verify`/`remove`, the execution path. Adds the `Shell` trait and
  `PackageManager::is_installed`; `toolchain` ops are free functions generic over both traits.
- **B3c** — toolchain **env** (§6.1): `[env]` in the def schema + regenerating `/pedagog/config/env.sh`
  on install/remove, and sourcing it from the login profile + code-server launch.
- **B4** — `build` orchestration + `--info`; wire `RUN pedagog image build` into a per-assignment
  image (and/or the base as a no-op).

## 11. Open / to refine

- **Declarative pruning in `build`** (§6) — additive-only for v1; revisit whether `build` should also
  remove things dropped from the manifest.
- **`env.sh` wiring details** (§6.1) — the exact profile/launch hook points (which login file, where in
  the code-server launch) get nailed down in B3c against the running image.
- **Ledger/dir drift** — the toolchains dir and the ledger's `toolchains` table are kept in sync by the
  verbs; nothing yet reconciles a hand-edited divergence (e.g. a def deleted out-of-band). Revisit if
  it bites.
