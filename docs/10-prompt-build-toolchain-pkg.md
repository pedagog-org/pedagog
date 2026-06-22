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

## Open (to settle in design)

- Declarative pruning in `build` (additive-only for v1?).
- How a shared toolchain's env/PATH reaches the student session (likely a separate step).
- Where toolchain defs come from for the base vs per-assignment image.
