# Recipe Resolve/Render Refactor

**Date:** 2026-07-21
**Status:** Planned

## Rationale

An in-progress refactor moved `RecipeStore` (loader) and a new `Render<O>` trait design
from `hammer` into `pedagog-core`, but left the tree half-connected:

- `hammer` still declares `mod loader;` / `mod render;` pointing at deleted files, and
  `resolve/mod.rs` still imports the deleted `resolve/plan.rs` — **hammer does not
  compile**.
- `core/src/recipe/plan.rs` is an orphaned duplicate of the old hammer plan types, not
  registered in `mod.rs`, still using self-referential `pedagog_core::` paths.
- `core/src/recipe/render/describe.rs` is fully commented out and unwired.
- The new `render::BuildPlan` shape (`base`/`platform`/`toolchain`/`assignment` as fixed
  `Layer` fields) can't represent a bare OS-only plan (`-o` alone). (The `-p -o`
  platform-only mode is being dropped — see hammer changes.)
- `LayerSource::BuildCleanup` (added in the last two commits) was dropped in favor of an
  unused `Assignment(AssignmentId)` variant with no backing data.
- `--registry` is a silent no-op — `Containerfile::prefixed()` exists but `render()`
  never calls it.

Alongside reconnecting these, this pass adds: per-assignment build steps, a
build-time network-restriction hook, port declarations, and a persistent-base-image
workflow (`Base` / `Assignment` / `Full` render targets).

---

## Design

### 1. Resolve moves into `pedagog-core`

`hammer/src/resolve/mod.rs` + `hammer/src/params.rs` move to `core::recipe::resolve`
(new module, with `params.rs` as a submodule, moved verbatim). They only ever touched
core types and produced core types — hammer becomes a thin CLI: load store, parse
target, call core resolve, call core render, print.

Each recipe type resolves independently into a small "artifact"; one function then
does the only cross-recipe merge that's needed (assignment param overrides onto the
platform's param declarations):

```rust
// core::recipe::resolve

struct OsArtifact {
    build:       Layer,                // os.hooks.build.init
    cleanup:     Layer,                // os.hooks.build.cleanup
    pkg_install: ArgHook<PkgArg>,      // carried forward for Step::Install expansion
    network:     NetworkHookDefs,      // carried forward for os_configure + runtime
    upstream:    String,
    image:       String,
}

struct PlatformArtifact {
    param_decls: HashMap<String, ParamDef>, // platform.hooks.build.params
    steps:       Vec<Step>,                  // platform.hooks.build.steps — unresolved
    entrypoint:  String,                     // unresolved
    ports:       Vec<u16>,
}

pub fn resolve_base(os_id: &OsId, store: &RecipeStore) -> Result<ImageSpec, String>;
pub fn resolve(assignment: &AssignmentYaml, store: &RecipeStore) -> Result<ImageSpec, String>;
```

`resolve_base` only touches the OS recipe — no assignment file exists in that CLI mode.
`resolve` does the full walk: OS, platform (with assignment's param overrides applied),
each toolchain (self-contained, no override needed), the assignment's own steps, and —
if `environment.network` is present — an `os_configure` layer plus a runtime prefix.

### 2. `ImageSpec` — an enum over the two build kinds

There are two kinds of thing to build: a reusable base image, or an assignment image.
Modeling that as one struct with six `Option<Layer>` fields lets you construct nonsense
(e.g. `os_cleanup: Some` while `platform: None`). An enum makes those illegal states
unrepresentable and removes the `if let` scatter from every renderer:

```rust
// core::recipe::render

pub enum ImageSpec {
    /// Reusable base image: FROM upstream, only the OS build layer, tagged as `image`.
    Base {
        upstream: String,
        image:    String,
        os:       Layer,
    },
    /// Assignment image: all build phases + runtime.
    Full {
        upstream:   String,
        base_image: String,      // e.g. "pedagog/ubuntu:22" — the pre-built base tag
        plan:       BuildPlan,
        runtime:    Runtime,     // user switch + entrypoint + root-only startup (see §7)
        ports:      Vec<u16>,
    },
}

#[derive(Clone)]
pub struct BuildPlan {
    pub os:           Layer,
    pub platform:     Layer,
    pub toolchain:    Vec<Layer>,
    pub assignment:   Layer,
    pub os_configure: Option<Layer>, // stays Option — genuinely absent when
                                     // network is Allow (see §5)
    pub os_cleanup:   Layer,
}

#[derive(Clone)]
pub enum LayerSource {
    Os(OsId),
    Platform(PlatformKind),
    Toolchain(ToolchainRef),   // ToolchainRef = Versioned<ToolchainId> — see §6
    Assignment(AssignmentId),
    OsConfigure(OsId),   // network.transcribe, parameterized by the Deny allowlist
    OsCleanup(OsId),
}
```

`os_configure` stays `Option` because network restriction is a genuine on/off
(`NetworkSpec::Allow` vs `Deny`), not an artifact of the base/full split — the enum
eliminates the spurious options and keeps the one honest one. `resolve_base` produces
`ImageSpec::Base`; `resolve` produces `ImageSpec::Full`.

### 3. Renderers share one options set; `from` chooses the FROM line

The per-renderer generic `O` is dropped. `registry` and from-source apply to *every*
renderer (describe shows the `FROM` line and the registry-prefixed reference too), so
they live in one shared `RenderOptions`:

```rust
pub enum FromSource {
    Standalone,   // FROM upstream, emit the os layer inline
    PrebuiltBase, // FROM base_image, skip the os layer (already baked in)
}

pub struct RenderOptions {
    pub registry: Option<String>,
    pub from:     FromSource,
}

// `render` returns the renderer's typed output (e.g. `Containerfile`); `ToString`
// is the uniform way to pull the final document out. Each renderer impls `ToString`
// (idiomatically via `Display`, which gives `ToString` for free).
pub trait Render: ToString {
    fn render(spec: &ImageSpec, opts: &RenderOptions) -> Self;
}
```

`from` only affects the `Full` case; for `Base` it's inert (a base always renders one
way). This replaces the earlier 3-way `RenderTarget`: the two build *kinds* are the
`ImageSpec` enum, and the Assignment-vs-Full rendering of a `Full` plan is the boolean
`from`.

`Containerfile::render` matches on the `ImageSpec` variant. The comment block records
the phase order (mirrored in the instructor-facing build-order doc, step 12) so the
sequence is legible at the one place it's emitted:

```rust
impl Render for Containerfile {
    // Phase order (Full build). Each phase is one or more RUN layers, in this order:
    //   1. os           — base OS package-manager init (only when FROM upstream)
    //   2. platform     — platform build steps (e.g. install code-server)
    //   3. toolchain    — each requested toolchain, in listed order
    //   4. assignment   — the assignment's own build steps
    //   5. os_configure — network.transcribe (Deny allowlist); omitted when Allow
    //   6. os_cleanup   — OS build cleanup; runs last so every phase's mess is swept
    // Then runtime metadata: EXPOSE ports, then USER + ENTRYPOINT (see render_runtime / §7).
    fn render(spec: &ImageSpec, opts: &RenderOptions) -> Self {
        let mut to = String::new();
        match spec {
            ImageSpec::Base { upstream, os, .. } => {
                Self::render_from(&mut to, &Self::prefixed(opts.registry.as_deref(), upstream));
                Self::render_phase(&mut to, BuildPhase::Base, &[os]);
            }
            ImageSpec::Full { upstream, base_image, plan, runtime, ports } => {
                let from = match opts.from {
                    FromSource::Standalone   => upstream,
                    FromSource::PrebuiltBase => base_image,
                };
                Self::render_from(&mut to, &Self::prefixed(opts.registry.as_deref(), from));

                // os layer is inline only when building standalone; under PrebuiltBase
                // it's already in base_image.
                if matches!(opts.from, FromSource::Standalone) {
                    Self::render_phase(&mut to, BuildPhase::Base, &[&plan.os]);
                }
                Self::render_phase(&mut to, BuildPhase::Platform, &[&plan.platform]);
                if !plan.toolchain.is_empty() {
                    Self::render_phase(&mut to, BuildPhase::Toolchain, &plan.toolchain.iter().collect::<Vec<_>>());
                }
                Self::render_phase(&mut to, BuildPhase::Assignment, &[&plan.assignment]);
                if let Some(configure) = &plan.os_configure {
                    Self::render_phase(&mut to, BuildPhase::OsConfigure, &[configure]);
                }
                Self::render_phase(&mut to, BuildPhase::OsCleanup, &[&plan.os_cleanup]);

                for port in ports {
                    writeln!(to, "EXPOSE {port}").unwrap();
                }
                Self::render_runtime(&mut to, runtime); // USER + ENTRYPOINT, or root
                                                        // privilege-drop when pre_root
            }
        }
        Containerfile(to)
    }
}
```

`render_runtime` emits `USER {user}` + exec-form `ENTRYPOINT` when `pre_root` is empty;
otherwise it stays root and emits a shell-form `ENTRYPOINT` that runs the `pre_root`
commands then `exec setpriv --reuid {user} … {entrypoint}` (see §7).

`describe.rs` stays commented out / unwired — ported in a follow-up once this is
verified (per your call last round). It will take the same `(spec, opts) -> Self`
signature.

### 4. Ports — recipe-defined, not enum-hardcoded

Lives next to `entrypoint` on `PlatformHookDefs`, since both are runtime-image
metadata rather than a build hook:

```rust
#[derive(Debug, Deserialize)]
pub struct PlatformHookDefs {
    pub build: ParamHook,
    pub entrypoint: String,
    #[serde(default)]
    pub ports: Vec<u16>,
}
```

### 5. Assignment gains its own build steps

Resolved exactly like a toolchain — self-contained steps, no param overrides needed:

```rust
#[derive(Debug, Deserialize)]
pub struct Environment {
    pub os: OsId,
    pub platform: PlatformSpec,       // required — no default; a Full build always has one
    #[serde(default)]
    pub toolchains: Vec<ToolchainRef>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub network: NetworkSpec,         // defaults to Allow (unrestricted)
}
```

`NetworkSpec` is an enum, not a struct with a `restrict` flag: the variant *is* the
mode. `Allow` carries no fields and means "do nothing"; only `Deny` carries an
allowlist. This also sidesteps the fact that our one OS hook (`network.transcribe`)
models exactly deny-default-allowlist — there's no allow-default+denylist variant to
render because `Allow` renders nothing.

```rust
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum NetworkSpec {
    /// Unrestricted egress — no os_configure layer, no network.enable at runtime.
    #[default]
    Allow,
    /// Default-deny egress; only `allow` CIDRs are permitted.
    Deny {
        #[serde(default)]
        allow: Vec<String>,
    },
}
```

`Allow` (the default, and the common case) skips `os_configure` entirely and leaves
`Runtime.pre_root` empty. `Deny { allow }` expands `allow` against
`os.hooks.network.transcribe` for `os_configure`, and puts `os.hooks.network.enable`'s
resolved commands into `Runtime.pre_root` — the root-only startup that runs before
privileges drop to the student user at container start (see §7).

### 6. `ToolchainRef` alias

`Versioned<ToolchainId>` appears at three call sites (`Environment.toolchains`,
`LayerSource::Toolchain`, `ToolchainRecipe.addons`). Add one alias in
`recipe::primitives` and use it everywhere:

```rust
pub type ToolchainRef = Versioned<ToolchainId>;
```

The underlying `OsId`/`PlatformId`/`ToolchainId`/`AssignmentId` aliases stay as-is
(all `= Id`) — this pass only fixes the `Versioned<…>` verbosity, not the lack of
per-kind newtype safety (a separate, optional hardening left out of scope).

### 7. Runtime — student user + privileged startup

The old renderer hardcoded `USER student` + an exec-form `ENTRYPOINT`. That breaks once
`network.enable` (which runs `iptables-restore`, root-only) has to run at container
start: you can't drop to `student` before applying rules, and the rules are kernel
namespace state, so they can't be baked at build time either. So the user switch and the
entrypoint form both depend on whether there's root work to do.

Model it as one bundle rather than ad-hoc render branches:

```rust
pub struct Runtime {
    pub user:       String,        // "student" (constant today; could become a recipe field)
    pub entrypoint: Command,       // the platform's runtime command
    pub pre_root:   Vec<Command>,  // root-only startup before the privilege drop;
                                   // network.enable under Deny, empty under Allow
}
```

`render_runtime`:

- **`pre_root` empty (Allow):** `USER student` then exec-form `ENTRYPOINT ["<entrypoint>"]`.
  Never root at runtime.
- **`pre_root` non-empty (Deny):** no `USER` line (stay root); shell-form
  `ENTRYPOINT ["/bin/sh","-c","<pre_root && …> && exec setpriv --reuid student \
  --regid student --init-groups <entrypoint>"]` — apply rules as root, then drop and
  `exec` so student is PID 1.

`setpriv` (util-linux, usually already present) is preferred over `gosu`/`su-exec`
(extra install). The exec-vs-shell entrypoint form is decided entirely by whether
`pre_root` is empty.

### 8. Unified arg-hook expansion

`expand_pkg_install` today string-replaces `{packages}` bespoke. Rather than add a
second bespoke expander for `network.transcribe`'s `{cidrs}`, route both through the
existing `interpolate`: an `ArgHook<A>`'s declared args become a params map, and each
step is interpolated with it.

```rust
// keyed by the arg enum's lowercase name (PkgArg::Packages → "packages", etc.)
fn expand_arg_hook<A: /* arg-name */>(
    hook: &ArgHook<A>,
    args: &HashMap<String, ParamVal>,
) -> Result<Vec<Command>, String>;
```

- pkg.install / pkg.remove → `{"packages": ParamVal::List(pkg names)}`
- network.transcribe → `{"cidrs": ParamVal::List(allow cidrs)}`

The `A` enum keeps validating which arg tokens are legal per hook; resolve supplies the
values. `{packages}`, `{cidrs}`, and any future arg token all flow through the same
`{token}` machinery as platform params — one code path, no per-hook special-casing.

---

## YAML changes

### Platform recipe — `ports:`

```yaml
platform: interactive
os: [ubuntu-22]
hooks:
  build:
    params: { ... }
    steps: [ ... ]
  entrypoint: "code-server --bind-addr 0.0.0.0:{port}"
  ports: [8080]
```

### Assignment — `steps:` and `network:`

```yaml
id: pointers-and-memory
name: Pointers and Memory
environment:
  os: ubuntu-22
  platform:
    kind: interactive
    params:
      terminal: true
  toolchains:
    - id: gcc
      version: "13"
  network:                # omit entirely for unrestricted (defaults to mode: allow)
    mode: deny
    allow: ["10.0.0.0/8", "192.168.0.0/16"]
  steps:
    - name: "Copy starter files"
      run: ["cp -r /opt/starter/* /home/student/"]
```

---

## hammer changes

- Remove `mod loader;`, `mod render;`, `resolve/mod.rs`, `resolve/plan.rs`, `params.rs`
  — all superseded by `pedagog_core::recipe::{store, resolve, render}`.
- `main.rs` dispatches by matching `Format` directly (no `Box<dyn Render>` — `render()`
  is a static fn, not object-safe the way the old trait was). Every renderer now has the
  identical `(spec, opts)` signature, so the match arms are symmetric:

```rust
fn make_output(spec: &ImageSpec, opts: &RenderOptions, format: &Format) -> String {
    match format {
        Format::Containerfile => Containerfile::render(spec, opts).to_string(),
        Format::Describe => unimplemented!("describe render pending — see open questions"),
    }
}
```

- The build *kind* comes from which resolve fn ran: `-o <os>` alone → `resolve_base`
  → `ImageSpec::Base`; `-a <file>` → `resolve` → `ImageSpec::Full`. The `--show-base`
  flag picks `FromSource::Standalone` (else `PrebuiltBase`) in `RenderOptions`;
  `--registry` fills `RenderOptions.registry`.
- **`-p -o` (platform-only) is removed** — the enum model has no clean home for a
  platform without an assignment, and it isn't used. Drop the `-p`/`--platform` arg, its
  `conflicts_with`/`requires` wiring, and the `(None, Some, Some)` match arm; the CLI
  target group becomes just `-a` xor `-o`.
- Cargo: hammer likely drops its direct `walkdir` dependency (now internal to core's
  store); keeps `serde_yaml` (still parses the assignment file at the CLI boundary).

---

## Alternatives considered

- **One flat `ImageSpec` struct with six `Option<Layer>` fields** (all `None` for the
  base case) — rejected; it permits illegal states (`os_cleanup: Some` with
  `platform: None`) and forces `if let` scatter in every renderer. The `ImageSpec::Base
  | Full` enum makes the two build kinds unrepresentable-when-wrong and keeps only
  `os_configure` optional (the one genuine on/off).
- **Two fully separate plan types (`BasePlan` + `BuildPlan`)**, as before the refactor
  — this is essentially what the enum is, but as an enum the renderers dispatch on one
  type with one `render()` signature, and `Assignment`/`Full` (which share *identical*
  resolved `Full` data) stay a single variant differing only by the `from` flag.
- **`Step::Transcribe` variant** for network config (mirroring `Step::Install`) —
  rejected in favor of a dedicated `environment.network` field; reads as configuration
  rather than an imperative step, at the cost of being a second mechanism (steps vs.
  dedicated field) for assignment data reaching an OS hook.
- **`NetworkSpec` as a struct with `restrict: bool` + `mode` + `allow`** — rejected;
  `restrict: false` and `mode: allow` both mean "unrestricted," so the struct carries a
  redundant switch. An enum (`Allow` | `Deny { allow }`) makes the variant the mode and
  gives the allowlist a home only where it's meaningful.
- **`PlatformKind::ports()` hardcoded match** — rejected; ports become a recipe field
  so adding a port doesn't require a Rust code change.
- **Plain `USER student` + `network.enable` in the entrypoint** (old shape) — rejected;
  `network.enable` needs root, so dropping to `student` first breaks it. Modeled as
  `Runtime.pre_root` + a `setpriv` privilege drop instead, so `USER`/entrypoint form
  follow from whether root work exists (§7).
- **Second bespoke expander for `network.transcribe`** (parallel to `expand_pkg_install`)
  — rejected; one generic `expand_arg_hook` routing arg values through the existing
  `interpolate` covers every `ArgHook` (§8).
- **Render-time phase filtering** (resolve always fully populates every phase; render
  decides what applies) — rejected for the base kind, since there's no assignment file
  to resolve platform/toolchain/assignment/network from in that CLI mode at all. Applied
  only to the `Full` variant's `Standalone` vs. `PrebuiltBase` (`from`) distinction,
  where the same resolved plan really does exist either way.

---

## Open questions

- **`describe` renderer** — deferred until `containerfile` is verified working
  end-to-end; needs a full rewrite against `Layer`/`LayerSource`/`ImageSpec`/
  `BuildPhase` once picked back up.
- **ARCHITECTURE.md's existing `network:` sketch** (`egress: deny`, `allow: [{host,
  port} | {cidr}]`) is richer than `Deny { allow: Vec<String> }` used here, which only
  covers what `TranscribeArg::Cidrs` actually consumes today (bare CIDRs, no host/port).
  Flagging the gap; not resolving it in this pass.
- **`network.disable` hook** — declared in `NetworkHookDefs` but intentionally left
  unwired for now (only `transcribe` and `enable` are used). Reserved for a future
  runtime toggle; not a gap in this pass.

---

## Rollback plan

All changes are local to `pedagog-core` and `hammer`; no database or deployed state
involved. `core/src/recipe/plan.rs` is deleted as dead code (never wired into
`mod.rs`, so nothing depends on it). Reverting the Rust changes restores the
pre-refactor types. YAML schema changes (`ports:`, assignment `steps:`/`network:`) are
additive and `#[serde(default)]`-guarded — existing recipe files continue to parse
without them.

---

## Implementation steps

1. **Delete** `crates/core/src/recipe/plan.rs` (orphaned, unregistered duplicate).
2. **`core::recipe::primitives`** — add `pub type ToolchainRef = Versioned<ToolchainId>;`;
   replace `Versioned<ToolchainId>` at its three call sites (`Environment.toolchains`,
   `LayerSource::Toolchain`, `ToolchainRecipe.addons`).
3. **`core::recipe::platform`** — add `ports: Vec<u16>` (`#[serde(default)]`) to
   `PlatformHookDefs`.
4. **`core::recipe::assignment`** — add `steps: Vec<Step>` and `network: NetworkSpec`
   (`#[serde(default)]`) to `Environment`; add the `NetworkSpec` enum
   (`Allow` | `Deny { allow }`, internally tagged on `mode`, `#[default] Allow`).
5. **`core::recipe::render::mod.rs`** — restructure `LayerSource` (add `OsConfigure`,
   `OsCleanup`); add the matching `BuildPhase::{OsConfigure, OsCleanup}` variants (with
   `#[strum(serialize = "…")]` for readable phase comments); convert `ImageSpec` to the
   `Base | Full` enum; make `BuildPlan` fields required except `os_configure: Option<Layer>`;
   add the `Runtime` struct (§7); replace `RenderStartpoint` with `FromSource` +
   `RenderOptions`; trait stays `Render: ToString` with `render(&ImageSpec, &RenderOptions) -> Self`.
6. **`core::recipe::render::containerfile`** — rewrite `render()` to match on the
   `ImageSpec` variant (with the phase-order comment block); consult `opts.from` for the
   `Full` `FROM` line + os-layer inclusion; call `prefixed(opts.registry, ...)`; add
   `EXPOSE` emission; replace `render_entrypoint` with `render_runtime` (§7: `USER` +
   exec `ENTRYPOINT`, or root + shell `ENTRYPOINT` with `setpriv` drop when `pre_root`);
   keep `impl ToString for Containerfile`.
7. **New `core::recipe::resolve` module** (+ `resolve::params` moved from
   `hammer/src/params.rs`, tests included) — `resolve_base`, `resolve`, the per-type
   artifact helpers, and a single generic `expand_arg_hook<A>(hook, args_map)` (§8) that
   replaces `expand_pkg_install` and serves `network.transcribe` too. `resolve` builds
   `Runtime` from the platform entrypoint + `network.enable` into `pre_root`; the
   `Deny { allow }` path also builds the `os_configure` layer. `Allow` leaves `pre_root`
   empty and `os_configure` `None`.
8. **`core::recipe::mod.rs`** — add `pub mod resolve;`.
9. **`hammer`** — delete `mod loader`, `mod render`, `resolve/mod.rs`, `resolve/plan.rs`,
   `params.rs`; update `main.rs` to call `pedagog_core::recipe::{store, resolve, render}`
   directly, matching on `Format` to pick the renderer (`.to_string()` on the result) and
   building `RenderOptions` from `--registry`/`--show-base`; update `cli.rs` — remove
   `-p`/`--platform` and reduce the target group to `-a` xor `-o`.
10. **Cargo.toml** — drop `walkdir` from `crates/hammer/Cargo.toml` if nothing else
    there uses it directly (check `vend/mod.rs`).
11. **Tests** — all inline as `#[cfg(test)] mod tests` in the file under test (Rust
    convention; no `tests/` dir, no new snapshot dependency — assert against expected
    strings written in the test):
    - `resolve::params` — the existing suite, moved over verbatim.
    - `resolve` — per-artifact unit tests; a generic `expand_arg_hook` test covering
      both `{packages}` and `{cidrs}`; a `Deny { allow }` test asserting the
      `os_configure` layer + non-empty `Runtime.pre_root` appear, and are absent under
      `Allow` (empty `pre_root`, no `os_configure`).
    - `render::containerfile` — expected-Containerfile assertions for `ImageSpec::Base`,
      and `Full` under both `FromSource::Standalone` and `PrebuiltBase`, built from
      small in-test fixture recipes.
    - **Resolves-everything smoke test** — walk the real `recipes/` submodule (located
      relative to `CARGO_MANIFEST_DIR`): assert every `os/` resolves to `ImageSpec::Base`
      and every example assignment resolves to `Full` with no error. No expected output —
      just that the production corpus still resolves. Skips gracefully if the submodule
      isn't checked out.
12. **(After implementation) Instructor build-order doc** — write `docs/` explaining the
    full firing order (os → platform → toolchain → assignment → os_configure → os_cleanup
    → runtime), what each phase runs and why the order matters, so instructors can reason
    about and debug their recipes. Mirrors the comment block in step 6.
