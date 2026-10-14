# Design Doc v2 — Recipe Pipeline Follow-ups

**Date:** 2026-10-13
**Status:** Planned
**Builds on:** [2026-07-21-recipe-resolve-render-refactor](2026-07-21-recipe-resolve-render-refactor.md)

## Rationale

The July 21 refactor landed mostly as planned (`ImageSpec` enum, shared `RenderOptions`,
resolve in core, generic `expand_arg_hook`, ports, assignment steps). A review of the
implementation against that plan and `docs/ARCHITECTURE.md` found:

- **Network enforcement contradicts ARCHITECTURE.md.** The architecture says runtime
  egress is a Kubernetes NetworkPolicy applied by the platform and changeable without a
  rebuild. The implementation bakes iptables rules into the image (`os_configure`) and
  runs `network.enable` as root at container start before dropping privileges. That
  requires `NET_ADMIN` on every student pod and a root PID 1, may not work under rootless
  podman, and freezes policy into the image.
- **`gosu` instead of `setpriv`.** The plan chose `setpriv` because `gosu` needs an extra
  install; the code emits `gosu`. Moot once the root entrypoint is removed.
- **`--registry` prefixes upstream images.** `hammer plan --os ubuntu-22 --registry
  localhost` emits `FROM localhost/ubuntu:22.04`, which only works with a mirror.
- **Exec-form `ENTRYPOINT` splits on whitespace**, breaking any quoted argument.
- **Ports are declared twice** (`ports: [8080]` and the entrypoint's `{port}` param) and
  can drift.
- **Corpus tests silently pass when the `recipes/` submodule is missing** — the
  store-backed resolve tests and the smoke test `return` early.
- **Toolchain `addons` are parsed but never resolved.** Also, the July plan's YAML example
  (`toolchains: [{id, version}]`) doesn't parse — `Environment.toolchains` only accepts the
  `gcc:13` string form.
- **Unfinished plan items:** the per-recipe artifact split (`resolve` is one long function
  duplicating the OS-layer code from `resolve_base`), the instructor build-order doc, the
  ARCHITECTURE.md update, and the `describe` renderer.
- **Loose ends:** `ArgHook.args` is declared but never checked against the `{tokens}` a
  hook uses; `pkg.remove` is required but unused.

Decision: continue from the current code (no restart). Most changes are deletions or
small schema fixes; the largest new work is addon resolution and `describe`.

---

## Design

### 1. Network: NetworkPolicy owns enforcement; hammer only validates

Egress is enforced at runtime by a K8s NetworkPolicy (a podman network for local dev),
owned by the platform. The image does nothing network-related.

**Removed:**

- `Runtime.pre_root`, the root/`gosu` entrypoint branch in `render_runtime`.
- `BuildPlan.os_configure`, `LayerSource::OsConfigure`, `BuildPhase::OsConfigure`.
- `NetworkHookDefs`, `TranscribeArg`, `NoArg` (no remaining users), and the `network:`
  block from `OsHookDefs` and every OS recipe.

**New `environment.network` schema** — policy data the platform will later turn into a
NetworkPolicy. hammer validates it and shows it in `describe`; it does not affect the
Containerfile.

```rust
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum NetworkSpec {
    #[default]
    Allow,
    Deny {
        #[serde(default)]
        allow: Vec<AllowRule>,
    },
}

pub struct AllowRule {
    pub target: EgressTarget,
    pub ports: Vec<NonZeroU16>, // empty = all ports; TCP assumed
}

pub enum EgressTarget {
    Host(HostPattern),   // RFC 1123 hostname, optionally one leading "*." label
    Cidr(ipnet::IpNet),  // v4 or v6; a single address is written as /32 or /128
}
```

- `HostPattern` is a newtype with `TryFrom<String>`: optional leading `*.`, then labels
  matching the RFC 1123 pattern (via the existing `regex`), lowercased. Other wildcards
  are rejected.
- `ipnet` is a **new dependency** (approved) — added to `pedagog-core` only, with its
  `serde` feature.
- An entry must have exactly one of `host` / `cidr`. Serde's `flatten` doesn't combine with
  `deny_unknown_fields`, so `AllowRule` gets a small hand-written `Deserialize` (via an
  intermediate struct with `Option` fields) that enforces exactly-one and rejects unknown
  keys.
- `ImageSpec::Full` gains `network: NetworkSpec` so `describe` can show it.

### 2. Registry prefix applies only to pedagog-built images

`prefixed(registry, …)` is applied only to `base_image` under `FromSource::PrebuiltBase`.
`ImageSpec::Base` and `Full` + `Standalone` emit `FROM <upstream>` unprefixed. `describe`
shows the registry-prefixed tag of the image being produced (`Base.image`) for reference.
An `--upstream-mirror` flag is a possible later addition, not in this pass.

### 3. Entrypoint as a list; ports interpolated

```rust
#[derive(Debug, Deserialize)]
pub struct PlatformHookDefs {
    pub build: ParamHook,
    pub entrypoint: Vec<String>, // hard switch: a string is a parse error
    #[serde(default)]
    pub ports: Vec<String>,      // interpolated, then parsed as NonZeroU16
}
```

New `interpolate_args(items, params) -> Result<Vec<String>, …>` in `resolve::params`:

- An item that is **exactly** one token with no format specifier (`"{flags}"`) and resolves
  to a `ParamVal::List` **splats**: one output item per list element (scalars only).
- Anything else goes through `interpolate` as today (lists inside a longer string stay
  space-joined).

Used for both `entrypoint` and `ports`. Ports then parse each item as `NonZeroU16`;
failure is a `ResolveError::InvalidPort`.

`Runtime` shrinks to:

```rust
pub struct Runtime {
    pub user: String,            // "student"
    pub entrypoint: Vec<String>, // exec-form argv
}
```

`render_runtime` always emits `USER {user}` + `ENTRYPOINT <json argv>`.

### 4. Toolchain addons

The assignment file is fully pinned — it acts as its own lockfile. No "latest" defaults
anywhere.

**Toolchain recipe:**

```rust
pub struct ToolchainRecipe {
    // …existing fields…
    #[serde(default)]
    pub addons: BTreeMap<ToolchainId, Vec<Version>>, // compatible addon versions, grouped by id
    #[serde(default)]
    pub coexist: bool, // this version may be installed alongside other versions of the same id
}
```

**Assignment selection** (replaces `Environment.toolchains: Vec<ToolchainRef>`):

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainSelection {
    pub id: ToolchainId,
    pub version: Version,                // required
    #[serde(default)]
    pub addons: Vec<ToolchainRef>,       // "gdb:14" — version required
}
```

**Resolution rules:**

1. Walk top-level selections in written order. Push each toolchain with origin
   `TopLevel`, then each of its addons (in written order) with origin `AddonOf(parent)`.
2. Each addon's version must be in `parent.addons[addon.id]`; otherwise
   `ResolveError::AddonNotDeclared` (listing the declared set).
3. Every pushed toolchain must have a recipe for the assignment's OS; otherwise
   `ToolchainNotFound` (with its origin).
4. **Dedupe** exact `(id, version)` duplicates — including a toolchain listed both
   top-level and as an addon — keeping the first position and merging origins into it.
5. **Conflicts:** if one id remains at more than one version, every involved version's
   recipe must set `coexist: true`; then all are installed in order. Otherwise
   `ResolveError::VersionConflict`.
6. One level only: an addon's own `addons` are not selectable.

Which coexisting version is the default on `PATH` is the recipe's responsibility (e.g.
install to `/opt/<id>-<ver>` and manage alternatives); documented in the instructor doc.

`LayerSource::Toolchain` carries origins for `describe` and errors:

```rust
pub enum ToolchainOrigin {
    TopLevel,
    AddonOf(ToolchainRef),
}

LayerSource::Toolchain { tc: ToolchainRef, origins: Vec<ToolchainOrigin> }
```

### 5. Typed resolve errors

`resolve` / `resolve_base` return `Result<ImageSpec, ResolveError>`. Hand-written
`Display`, no new dependency.

```rust
pub enum ResolveError {
    OsNotFound(OsId),
    PlatformNotFound { kind: PlatformKind, os: OsId },
    ToolchainNotFound { tc: ToolchainRef, os: OsId, origin: ToolchainOrigin },
    AddonNotDeclared { parent: ToolchainRef, addon: ToolchainRef, declared: Vec<Version> },
    VersionConflict { id: ToolchainId, entries: Vec<ConflictEntry> },
    InvalidPort { raw: String },
    Interpolation { context: String, message: String },
}

pub struct ConflictEntry {
    pub version: Version,
    pub origins: Vec<ToolchainOrigin>,
    pub allowed_by_parent: Vec<(ToolchainRef, Vec<Version>)>, // each addon parent's declared set
    pub coexist: bool,
}
```

`VersionConflict`'s `Display` must include: every entry with its origin chain, each
parent's declared set, the overlap of those sets, which recipes lack `coexist`, and a
suggested fix. Target shape:

```
toolchain `gdb` is selected at 2 versions, and not all of them allow coexisting:
  gdb:15  via gcc:13    (gcc:13 allows gdb 14, 15)    coexist: no
  gdb:13  via clang:17  (clang:17 allows gdb 12, 13)  coexist: yes
  versions allowed by every parent: none
help: pick one version allowed by every parent, or set `coexist: true` in every listed gdb recipe
```

When the overlap is non-empty, the help line names it (`use gdb:14 everywhere`).

### 6. Load-time validation

After `RecipeStore::load` parses every file, a cross-reference pass reports (alongside
existing parse errors):

- **Addon declarations:** every `addons[id]` version has a toolchain recipe that shares
  at least one OS with the parent.
- **ArgHook tokens:** every `{token}` in an `ArgHook`'s steps is one of its declared `args`
  (token name = text before any `:` format specifier). Token scanning is shared with
  `resolve::params`.
- `pkg.remove` becomes optional (`Option<ArgHook<PkgArg>>`), reserved and unused.

### 7. Resolve split into per-recipe artifacts

`resolve.rs` is restructured so each recipe type resolves on its own and `resolve_base`
and `resolve` share code:

```rust
struct OsArtifact<'a> {
    init: Vec<ResolvedStep>,
    cleanup: Vec<ResolvedStep>,
    pkg_install: &'a ArgHook<PkgArg>,
    upstream: String,
    image: String,
}

struct PlatformArtifact {
    layer: Layer,
    entrypoint: Vec<String>,
    ports: Vec<u16>,
}

fn os_artifact<'a>(os_id: &OsId, store: &'a RecipeStore) -> Result<OsArtifact<'a>, ResolveError>;
fn platform_artifact(spec: &PlatformSpec, os: &OsArtifact, store: &RecipeStore) -> Result<PlatformArtifact, ResolveError>;
fn toolchain_layers(sel: &[ToolchainSelection], os: &OsArtifact, store: &RecipeStore) -> Result<Vec<Layer>, ResolveError>;
```

New phase order (no `os_configure`):

```
os → platform → toolchains (each followed by its addons) → assignment → os_cleanup → runtime
```

### 8. `describe` renderer

Ported from the commented-out `describe.rs` against the current types, same
`Render::render(&ImageSpec, &RenderOptions)` signature, follows `--show-base`.

- **Tree:** `FROM` (registry-prefixed where applicable) → phases → layers → step names.
  Toolchain layers show origin (`gdb:14 (addon of gcc:13)`).
- **Then:** ports, user, entrypoint argv, network policy summary, toolchain resolution
  (coexisting versions called out).
- **`Base`:** FROM upstream, OS layer, produced image tag.
- **Color:** only when hammer decides output is a TTY (and not `-O file`); passed in as
  `RenderOptions.color: bool` so core does no terminal detection. How color is produced
  is an open question (see below).

---

## YAML changes

### OS recipe

```yaml
hooks:
  build: { init: [...], cleanup: [...] }
  pkg:
    install: { args: [packages], steps: [...] }
    # remove: optional now
  # network: removed
```

### Platform recipe

```yaml
hooks:
  build:
    params:
      port: { type: int, default: 8080 }
    steps: [ ... ]
  entrypoint: [code-server, --bind-addr, "0.0.0.0:{port}"]
  ports: ["{port}"]
```

### Toolchain recipe

```yaml
id: gcc
version: "13"
os: [ubuntu-22]
addons:
  gdb: ["14", "15"]
  valgrind: ["3.22"]
# coexist: true   # only for recipes that install side-by-side
```

### Assignment

```yaml
environment:
  os: ubuntu-22
  platform:
    kind: interactive
  toolchains:
    - id: gcc
      version: "13"
      addons: [gdb:14, valgrind:3.22]
  network:
    mode: deny
    allow:
      - host: "*.github.com"
        ports: [443]
      - cidr: 10.0.0.0/8
```

---

## hammer changes

- Map `ResolveError` into miette reports (`Display` text; `help:` line surfaced as miette
  help where present).
- Set `RenderOptions.color` from TTY detection (and off when `-O` is used).
- Wire `-f describe` to the new renderer.
- No CLI flag changes otherwise; `--registry` semantics change as in §2.

---

## Alternatives considered

- **Keep in-image iptables enforcement** (current code) — rejected: `NET_ADMIN` on
  student pods, root PID 1, rootless podman concerns, rebuild to change policy, and it
  contradicts ARCHITECTURE.md.
- **Hybrid** (NetworkPolicy at runtime + image hook to restrict the build step) —
  rejected for now; build-step restriction belongs to the future builder.
- **Registry prefixes everything** / **two flags now** — rejected; prefixing upstream
  only works with a mirror. `--upstream-mirror` can be added later.
- **Entrypoint string in shell form, or shell-words parsing** — rejected: a shell
  wrapper, or a new dependency. A YAML list has no quoting edge cases.
- **Port param type / leave ports duplicated** — rejected in favor of interpolated
  `ports` entries.
- **Unpinned addons resolving to "latest"**, and **a separate lockfile** — rejected;
  assignments must not change when a newer recipe is added. The assignment is fully
  pinned.
- **Semver-range addon declarations** — rejected; new versions would become compatible
  untested. Explicit grouped lists codify compatibility.
- **Warn + assignment-level `allow_multiple` flag** for version conflicts — rejected;
  whether two versions can coexist is a property of the recipe, not the assignment.
- **Hard dependencies (`requires:`)** — out of scope; addons stay opt-in.
- **`thiserror` / miette in core / String errors** — rejected in favor of a typed enum
  with hand-written `Display` (no new dependency, structured data for describe/hammer).
- **Hand-rolled CIDR parsing** — rejected in favor of `ipnet`.

---

## Open questions

- **Student user creation** — which recipe creates `student` today is unverified (the
  `recipes/` submodule wasn't inspected). Confirm during the recipes-repo PR; if nothing
  creates it, the OS recipe's `build.init` should.
- **`network.transcribe` usage** — confirm nothing else in the recipes repo depends on the
  network hooks before removing them.
- **`describe` color** — the `console` crate was removed from the workspace. Options:
  re-add `console` (new dependency; needs approval), or `std::io::IsTerminal` in hammer
  plus a tiny ANSI helper in core. Decide before PR 5.
- **Toolchain selection shorthand** — whether `toolchains:` also accepts the `gcc:13`
  string form (no addons) alongside the object form. Default in this plan: object form
  only.
- **FQDN enforcement backend** — standard NetworkPolicy matches CIDRs/ports only;
  `host:` entries need an FQDN-aware plugin (e.g. Cilium) or an egress proxy. Platform-side
  decision.
- **NetworkPolicy enforcement on the cluster** — confirm the Pi cluster's network plugin
  actually enforces NetworkPolicy.
- **Build-step network restriction** — deferred until the builder (solution validation
  step) exists.
- **Corpus opt-out variable name** — proposed `PEDAGOG_SKIP_RECIPE_CORPUS=1`.

---

## Rollback plan

Each area lands as its own PR (below), so any one can be reverted independently. The
recipes-repo PR is paired with a submodule bump; reverting a pedagog PR that changed a
schema means also reverting the submodule pointer to the matching recipes commit. No
database or deployed state is involved. `ipnet` is removed by reverting PR 1.

---

## Implementation steps

Branches follow `<type>/<slug>`; small Conventional Commits within each. Tests inline in
`#[cfg(test)] mod tests` of the file under test.

### PR 0 — recipes repo (lands first)

1. OS recipes: remove `network:` hooks; ensure `student` user creation exists.
2. Platform recipes: `entrypoint` → list; `ports: ["{port}"]`.
3. Toolchain recipes: `addons` → grouped map; add `coexist: true` where versions install
   side-by-side.
4. Example assignments: `toolchains` → object form; new `network` allow-entry shape.
5. Bump the submodule in each pedagog PR below as it needs the new schema.

### PR 1 — `refactor/network-policy-schema`

1. Remove `NetworkHookDefs`, `TranscribeArg`, `NoArg`, `OsHookDefs.network`.
2. Remove `os_configure` (`BuildPlan`, `LayerSource`, `BuildPhase`) and `Runtime.pre_root`;
   `render_runtime` always emits `USER` + exec `ENTRYPOINT`.
3. Add `ipnet` (core, `serde` feature); add `NetworkSpec` / `AllowRule` / `EgressTarget` /
   `HostPattern` with the hand-written `AllowRule` deserializer.
4. Add `network` to `ImageSpec::Full`.
5. Tests: host validation (plain, `*.` wildcard, rejected wildcards, bad labels), CIDR v4/v6,
   exactly-one-of host/cidr, unknown keys rejected, ports non-zero, `Allow` default.

### PR 2 — `fix/registry-prefix`

1. Apply `prefixed` only to `base_image` under `PrebuiltBase`.
2. Tests: `Base` + registry → upstream unprefixed; `Full`/`Standalone` + registry →
   unprefixed; `Full`/`PrebuiltBase` + registry → prefixed.

### PR 3 — `feat/entrypoint-list-ports`

1. `PlatformHookDefs.entrypoint: Vec<String>`, `ports: Vec<String>`.
2. `interpolate_args` with whole-item list splatting.
3. `Runtime.entrypoint: Vec<String>`; drop `split_whitespace`.
4. Tests: splat vs. embedded list, quoted args preserved, port parse errors, port from
   param default/override.

### PR 4 — `feat/toolchain-addons`

1. `ResolveError` enum + `Display` (including the conflict report); switch `resolve` /
   `resolve_base` to it.
2. Artifact split (`os_artifact`, `platform_artifact`, `toolchain_layers`).
3. `ToolchainRecipe.addons` grouped map + `coexist`; `ToolchainSelection`;
   `LayerSource::Toolchain { tc, origins }`.
4. Resolution rules 1–6 (§4).
5. Store cross-reference pass: addon declarations, ArgHook tokens; `pkg.remove` optional.
6. Corpus tests fail loudly when `recipes/` is missing unless
   `PEDAGOG_SKIP_RECIPE_CORPUS=1`; document in README/Justfile that tests need
   `git submodule update --init recipes`.
7. Tests (fixture stores built in-test where possible): addon order; undeclared version;
   addon missing for OS; exact dedupe across top-level + addon; conflict without
   `coexist` (assert `Display` contains origins, declared sets, overlap, missing coexist,
   help); conflict with all `coexist` installs both; non-empty overlap suggestion;
   load-time addon and token errors.

### PR 5 — `feat/describe-renderer`

1. Resolve the color open question.
2. Port `describe.rs`; add `RenderOptions.color`; wire `-f describe` in hammer.
3. Tests: `Base`, `Full` × `Standalone`/`PrebuiltBase`, addon origins shown, network
   summary, color off produces no ANSI codes.

### PR 6 — `docs/design-doc-v2`

1. ARCHITECTURE.md: network section (NetworkPolicy, allow-entry schema, open backend
   question), toolchain/addon schema + coexist rule, registry semantics, phase order.
2. Instructor build-order doc in `docs/`: phase order, what each phase runs, why the order
   matters, addon placement, coexist/PATH responsibility.
3. Set this plan's status to Implemented.
