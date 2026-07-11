# Plan: hammer — Milestone 1 (plan + Containerfile generation)

## Rationale

Core recipe types are defined in `pedagog-core`. This milestone wires them into a
runnable `hammer plan <assignment.yml>` command that resolves recipes, prints an
inspectable build plan, and generates a Containerfile or shell script — without
requiring k8s.

---

## Scope

- `hammer plan --assignment <file>` — build plan in one of three formats
- `hammer plan --os <id>` — base image plan in one of three formats
- Recipe discovery from a standard Linux path, overridable via env
- Minimal `AssignmentYaml` type added to `pedagog-core`
- `hammer build-os` and `hammer build` (Kaniko) are out of scope

---

## Recipe Discovery

`HAMMER_RECIPES` env var sets the primary recipe directory. There is no default —
if it is unset, hammer emits a `miette` warning:

```
warning: HAMMER_RECIPES is not set
         Set it to the directory containing your os/, platforms/, and toolchains/ recipes.
```

Additional directories are appended via `--recipes <dir>`, which may be given multiple
times. Directories are searched in order: `HAMMER_RECIPES` first (if set), then each
`--recipes` dir in order. If neither `HAMMER_RECIPES` nor any `--recipes` flag is
provided, hammer errors with no recipe directories configured.

The loader scans each directory recursively for `.yaml` files. It does not infer type
from directory names except at the top level (`os/`, `platforms/`, `toolchains/`) —
everything below those is scanned recursively. Every file is parsed; all errors are
collected and reported together (no early stop).

---

## New Types

### `pedagog-core`: `recipe/primitives.rs` — ID type aliases and generic `Versioned`

Add type aliases for `Id`:

```rust
pub type OsId        = Id;
pub type PlatformId  = Id;
pub type ToolchainId = Id;
```

Aliases, not newtypes — documents intent without boilerplate. Upgrade to newtypes
later if cross-kind confusion becomes a real bug source.

These aliases must be used everywhere an `Id` carries a known semantic:
- `OsDef.id: OsId`, `OsDef.upstream: String`, `OsDef.image: String`
- `PlatformRecipe.os: Vec<OsId>`
- `ToolchainRecipe.id: ToolchainId`, `ToolchainRecipe.os: Vec<OsId>`,
  `ToolchainRecipe.addons: Vec<Versioned<ToolchainId>>`
- All `RecipeStore` and plan type signatures (already shown below with aliases)

Make `Versioned` and `MaybeVersioned` generic over the ID type:

```rust
pub struct Versioned<T = Id> {
    pub id:      T,
    pub version: Version,
}

pub struct MaybeVersioned<T = Id> {
    pub id:      T,
    pub version: Option<Version>,
}
```

The default `T = Id` keeps existing bare uses valid. Callers that know the kind use
`Versioned<ToolchainId>`, `MaybeVersioned<OsId>`, etc. This is a breaking change to
`ToolchainRecipe.addons` (currently `Vec<Versioned>` → `Vec<Versioned<ToolchainId>>`)
and to `LayerSource::Toolchain` in the plan types.

### `pedagog-core`: `recipe/assignment.rs`

`ToolchainRef` is replaced by `Versioned<ToolchainId>`:

```rust
pub struct AssignmentYaml {
    pub name: String,
    pub environment: Environment,
}

pub struct Environment {
    pub platform:   PlatformKind,
    pub os:         OsId,
    pub toolchains: Vec<Versioned<ToolchainId>>,
}
```

`environment` encapsulates the full runtime environment. `os` is required — a platform
may have multiple definitions for different OS variants, so the OS must always be
explicit.

Toolchains are listed explicitly. Addons declared in a `ToolchainRecipe` are metadata
about what is available, not auto-installed. If a user wants an addon, they list it
explicitly in `toolchains:` like any other toolchain entry.

---

## Module Structure (`crates/hammer/src/`)

```
main.rs
cli.rs
loader/
  mod.rs          RecipeStore; scans directories, parses files, collects errors
resolve/
  mod.rs          AssignmentYaml + RecipeStore → Plan
  plan.rs         Plan, PlanKind, Layer, LayerSource, ResolvedStep types
render/
  mod.rs          OutputFormat enum; dispatches to submodules
  describe.rs       human-readable tree output
  script.rs       executable shell script
  containerfile.rs  Dockerfile/Containerfile
```

---

## Loader (`loader/mod.rs`)

`RecipeStore` fields are private; all access is through query methods:

```rust
pub struct RecipeStore {
    os:         HashMap<OsId, OsDef>,
    platforms:  HashMap<PlatformKind, Vec<PlatformRecipe>>,  // all OS variants for a kind
    toolchains: HashMap<(ToolchainId, Version), ToolchainRecipe>, // each carries Vec<OsId>
}

impl RecipeStore {
    // Primary lookups — used by the resolver
    pub fn os(&self, id: &OsId) -> Option<&OsDef>;
    pub fn platform(&self, kind: PlatformKind, os: &OsId) -> Option<&PlatformRecipe>;
    pub fn toolchain(&self, id: &ToolchainId, version: &Version, os: &OsId) -> Option<&ToolchainRecipe>;

    // List everything — used for discovery, diagnostics, and future list commands
    pub fn list_oses(&self) -> Vec<&OsId>;
    pub fn list_platforms(&self) -> Vec<&PlatformRecipe>;
    pub fn list_toolchains(&self) -> Vec<&ToolchainRecipe>;

    // Filtered listing — used for error messages and cross-reference
    pub fn platforms_for_os(&self, os: &OsId) -> Vec<&PlatformRecipe>;
    pub fn toolchains_for_os(&self, os: &OsId) -> Vec<&ToolchainRecipe>;
    pub fn oses_for_platform(&self, kind: PlatformKind) -> Vec<&OsId>;
    pub fn oses_for_toolchain(&self, id: &ToolchainId, version: &Version) -> Vec<&OsId>;
}
```

Loading:
1. Walk each recipe directory; collect all `.yaml` paths under `os/`, `platforms/`,
   `toolchains/` subdirectories.
2. Attempt to parse every file. Accumulate all errors.
3. If any parse errors: emit all of them via `miette` and exit. Do not partially load.

---

## Resolution (`resolve/mod.rs`)

**Build plan** (`--assignment <file>`) — given `AssignmentYaml` and `RecipeStore`:

1. **OS**: look up `environment.os` directly in the store; error if not found.
2. **Platform**: find the `PlatformRecipe` matching `(platform kind, os id)`.
3. **Toolchains**: for each `Versioned<ToolchainId>` in `environment.toolchains`, look
   up `(id, version, os)`; error if not found. Each entry is resolved independently —
   no addon auto-expansion. The `addons` field on `ToolchainRecipe` is metadata only.
4. **Produce a `BuildPlan`**.

**Base image plan** (`--os <id>`) — given an `OsId` and `RecipeStore`:

1. Look up the `OsDef`; error if not found.
2. Collect the OS `init` hook steps.
3. Produce a `BasePlan`.

---

## Plan Types (`resolve/plan.rs`)

Two concrete plan types — not an enum, since they have different fields and rendering paths.

```rust
/// FROM upstream → [OS init steps] → tagged as OsDef.image (e.g. pedagog/ubuntu:22).
pub struct BasePlan {
    pub os_id:    OsId,
    pub upstream: String,    // OsDef.upstream — what we FROM (e.g. ubuntu:22.04)
    pub image:    String,    // OsDef.image — the tag we produce (e.g. pedagog/ubuntu:22)
    pub layers:   Vec<Layer>,
}

/// FROM <base_image> → [platform build steps] → [toolchain build steps] → assignment image.
/// `include_base: true` prepends the OS init layers inline for inspection only.
pub struct BuildPlan {
    pub name:         String,
    pub base_image:   String,    // e.g. "pedagog/ubuntu:22" — the FROM; no extra fields needed
    pub layers:       Vec<Layer>,
    pub entrypoint:   String,    // runtime config — rendered at the end, after all build layers
    pub include_base: bool,
}

pub struct Layer {
    pub source: LayerSource,
    pub steps:  Vec<ResolvedStep>,
}

pub enum LayerSource {
    Os(OsId),
    Platform(PlatformKind),
    Toolchain(Versioned<ToolchainId>),
}

pub struct ResolvedStep {
    pub name:     Option<String>,
    pub commands: Vec<Command>,
}

/// Resolved shell command. Thin newtype for now; can grow env vars, cwd,
/// exec-vs-shell distinction, etc. without changing call sites.
pub struct Command(pub String);
```

No existing crate fits here: `std::process::Command` is for spawning, not
representing. This stays in `resolve/plan.rs` — it is a hammer-internal resolved
type, not a core recipe primitive.

**Layer label format** (used by all output modes):
```
from: { type: Os, id: ubuntu-22 }
from: { type: Platform, id: interactive }
from: { type: Toolchain, id: gcc:13 }
from: { type: Toolchain, id: gdb:14 }
```

**Step resolution:**
- `Step::Install { packages }`: join package names, substitute `{packages}` into the
  OS `pkg.install` hook's steps → produces shell commands.
- `Step::Run { run }`: substitute `{param}` references from the recipe's `params` map.

---

## Output Formats (`render/`)

Selected via `--format <fmt>`, defaulting to `describe`. Output goes to stdout unless
`--output <file>` is given.

### `describe` (default)

Human-readable structured output for inspection. Build layers appear in order, then a
`[Runtime]` section at the end containing the entrypoint. Commands are listed bare —
no `RUN` prefix.

```
Plan: pointers-and-memory
Base: pedagog/ubuntu:22

[Build]

  [For Platform interactive]
    [Install code-server]
      if [ -n "$DEV" ]; then
        dpkg -i /pedagog/dev/code-server.deb
      else
        curl -fsSL ... && dpkg -i /tmp/code-server.deb
      fi
    [Remove built-in extensions]
      rm -rf /usr/lib/code-server/lib/vscode/extensions/tunnel-forwarding
      ...

  [For Toolchain gcc:13]
    [Install gcc 13]
      apt-get install -y gcc-13 g++-13
    [Set as default compiler]
      update-alternatives --install ...

  [For Toolchain gdb:14]
    [Install gdb]
      apt-get install -y gdb

[Runtime]
  Entrypoint: /usr/bin/code-server --bind-addr 0.0.0.0:8080
```

Base image plan (`hammer plan --os ubuntu-22`):

```
Base Image Plan: ubuntu-22
FROM ubuntu:22.04  →  pedagog/ubuntu:22

[Build]

  [For Os ubuntu-22]
    [Initialize package manager]
      apt-get update
      ...
```

### `script`

To be revisited. Naively emitting commands as a shell script works for `Run` steps
but breaks for `Install` steps (which expand through the OS pkg hook and assume a
container environment). Deferred until the build command exists and we can see what's
actually useful here.

### `containerfile`

Standard `Dockerfile`/`Containerfile`. `DEV` is always present via `ARG`/`ENV`.
Layer source is noted in a comment above each block.

**Build plan** (`--assignment <file> --format containerfile`):

```dockerfile
FROM pedagog/ubuntu:22

ARG DEV=""
ENV DEV=$DEV

# [For Platform interactive]
RUN if [ -n "$DEV" ]; then \
      dpkg -i /pedagog/dev/code-server.deb; \
    else \
      curl -fsSL ... && dpkg -i /tmp/code-server.deb; \
    fi
RUN rm -rf /usr/lib/code-server/lib/vscode/extensions/tunnel-forwarding \
           ...

# [For Toolchain gcc:13]
RUN apt-get install -y gcc-13 g++-13
RUN update-alternatives --install ...

ENTRYPOINT ["/usr/bin/code-server", "--bind-addr", "0.0.0.0:8080"]
```

**Base image plan** (`--os ubuntu-22 --format containerfile`):

```dockerfile
FROM ubuntu:22.04

# [For Os ubuntu-22]
RUN apt-get update && apt-get install -y ...
```

**Build plan with `--show-base` (`--assignment <file> --format containerfile --show-base`)**:
concatenation of both — a single standalone Containerfile that builds from the upstream
OS image all the way to the final assignment image, with no dependency on a pre-built
base image:

```dockerfile
FROM ubuntu:22.04

# [For Os ubuntu-22]  (base)
RUN apt-get update && apt-get install -y ...

ARG DEV=""
ENV DEV=$DEV

# [For Platform interactive]
RUN if [ -n "$DEV" ]; then \
      dpkg -i /pedagog/dev/code-server.deb; \
    else \
      curl -fsSL ... && dpkg -i /tmp/code-server.deb; \
    fi
RUN rm -rf ...

# [For Toolchain gcc:13]
RUN apt-get install -y gcc-13 g++-13
RUN update-alternatives --install ...

ENTRYPOINT ["/usr/bin/code-server", "--bind-addr", "0.0.0.0:8080"]
```

---

## CLI (`cli.rs`)

```
hammer plan  (--assignment <file> | --os <id>)
    [--recipes <dir>]                          additional recipe dir (repeatable)
    [--format describe|script|containerfile]   default: describe
    [--output <file>]                          write to file instead of stdout
    [--show-base]                              prepend OS init layers (build plan only)
```

`--assignment` and `--os` are mutually exclusive; exactly one is required.

Error reporting via `miette`. Core validation errors (`String`) are wrapped into
miette diagnostics at the CLI boundary — core itself has no miette dependency.

---

## Dependencies

New in `hammer`:
- `miette` + `miette-derive` — diagnostic error reporting
- `walkdir` — recursive directory traversal

Both need to be added to workspace `Cargo.toml` and `crates/hammer/Cargo.toml`.

---

## Alternatives Considered

- **Infer type from full directory path**: fragile; a misplaced file silently fails.
  Instead, only the top-level dir name (`os/`, `platforms/`, `toolchains/`) is used
  to route parsing; everything below is scanned recursively.
- **Stop on first load error**: makes it harder to find all broken files at once.
  Collect-all is more useful for recipe authors.
- **Single `--format containerfile` flag**: less ergonomic than a format selector;
  adding a third format later would require a new flag.
- **`os` as optional with inference**: dropped — a platform may have multiple OS
  definitions, so inference would be ambiguous in the common case. Require explicit `os`.
- **Embed recipes in binary now**: correct for production (arch doc) but slows dev
  iteration. Deferred — use filesystem discovery for this milestone.
- **Newtype wrappers for `OsId`, `ToolchainId`, etc.**: would prevent accidentally
  passing an `OsId` where a `ToolchainId` is expected at compile time. Deferred —
  type aliases give readability today; upgrade if cross-kind confusion becomes a real
  source of bugs.
- **Two subcommands (`plan` / `plan-base`)**: `--assignment` and `--os` as mutually
  exclusive flags on a single `plan` subcommand is cleaner — same flags apply to both
  modes and there's less surface area in the CLI.
- **Auto-expanding addons during resolution**: addons on a `ToolchainRecipe` are
  metadata (what's available), not a directive to install. Users list addons explicitly
  in `environment.toolchains` just like any other toolchain entry.
- **`BuildPlan.base` as a struct with upstream/image fields**: the resolver for a build
  plan only needs the base image tag string to emit `FROM`. The `upstream` field is
  only needed by the base image plan. Keeping them separate avoids carrying unused data.

---

## Open Questions

None outstanding.

---

## Rollback Plan

`hammer` is a standalone CLI with no persistent state in this milestone. Rolling back
means reverting the crate. The `AssignmentYaml` addition to core is a new module with
no existing callers — fully backward-compatible.

---

## Implementation Steps

1. Update `recipe/primitives.rs` in `pedagog-core`:
   - Add `OsId`, `PlatformId`, `ToolchainId` type aliases
   - Make `Versioned<T>` and `MaybeVersioned<T>` generic (`T = Id` default)
   - Update `ToolchainRecipe.addons` to `Vec<Versioned<ToolchainId>>`
2. Add `recipe/assignment.rs` to `pedagog-core`; update `recipe/mod.rs`
3. Add `miette`, `walkdir` to workspace; add to `crates/hammer/Cargo.toml`
4. Implement `cli.rs` — `plan` subcommand with `--assignment`/`--os` (mutually exclusive)
   and all shared flags
5. Implement `loader/mod.rs` — `RecipeStore` (private fields), recursive scan,
   collect-all parse errors; warn if `HAMMER_RECIPES` unset; error if no dirs at all
6. Implement `resolve/plan.rs` — `BasePlan`, `BuildPlan`, `Layer`, `LayerSource`,
   `ResolvedStep`, `Command` types
7. Implement `resolve/mod.rs` — `resolve_build` and `resolve_base` functions
8. Implement `render/describe.rs` — `[Build]` / `[Runtime]` sections, `[For ...]`
   headers, bare commands, entrypoint at end
9. Implement `render/containerfile.rs` — `ARG DEV=""` / `ENV DEV=$DEV` at top,
   `ENTRYPOINT` at bottom, layer source comments
10. Wire together in `main.rs`
11. Add integration test: fixture assignment YAML + fixture recipes → snapshot plan output
