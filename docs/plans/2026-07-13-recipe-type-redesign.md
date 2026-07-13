# Recipe Type Redesign

**Date:** 2026-07-13  
**Status:** Planned

## Rationale

Several accumulated gaps in the recipe type system:

- `AssignmentYaml` has no stable `id` field; only a human-readable `name`.
- `ParamDef` has no `info` field, so there is nowhere to attach documentation for error messages or tooling.
- OS hook params (`params: Vec<Param>`) are untyped string examples with no Rust-level constraint on which args a given hook is allowed to declare. Any hook can claim any param name.
- `Param` carries only `id` and `example: String` — no type, no default.
- Platform/toolchain hooks do not need `use_params` or `args`; `$DEV` is available as an env var and platform params are already resolved by the time steps run.

## Design

### `ParamDef` — gains `info`, keeps `Map`

```rust
pub enum ParamDef {
    Bool { default: Option<bool>,          info: Option<String> },
    Int  { default: Option<i64>,           info: Option<String> },
    Str  { default: Option<String>,        info: Option<String> },
    List { default: Option<Vec<ParamVal>>, info: Option<String> },
    Map  { properties: HashMap<String, ParamDef>, info: Option<String> },
}
```

`Map` is kept because platform params like `extensions` require nested structure. `info` is `Option` and `#[serde(default)]` so existing YAMLs without it continue to parse.

### `HammerParam` — closed enum of what hammer injects

```rust
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HammerParam {
    Packages, // str  — provided when hammer expands pkg.install / pkg.remove
    Rules,    // str  — provided when hammer expands network.transcribe
}
```

`DEV` is excluded — it is a shell env var (`$DEV`), not a hammer-interpolated param.

### `HookScope` — constrains which `HammerParam` values a hook type may declare

```rust
pub trait HookScope {
    const ALLOWED: &'static [HammerParam];
}
```

Each OS hook type has its own scope marker:

```rust
// os.rs
pub struct InitScope;
impl HookScope for InitScope {
    const ALLOWED: &'static [HammerParam] = &[];
}

pub struct PkgScope;
impl HookScope for PkgScope {
    const ALLOWED: &'static [HammerParam] = &[HammerParam::Packages];
}

pub struct TranscribeScope;
impl HookScope for TranscribeScope {
    const ALLOWED: &'static [HammerParam] = &[HammerParam::Rules];
}

pub struct SimpleNetworkScope;
impl HookScope for SimpleNetworkScope {
    const ALLOWED: &'static [HammerParam] = &[];
}
```

### `OsHook<S: HookScope>` — replaces `ParamHookDef`

```rust
// primitives.rs
pub struct OsHook<S: HookScope> {
    pub args: Vec<HammerParam>,
    pub steps: Vec<Step>,
    _scope: PhantomData<S>,
}
```

Deserialization uses `#[serde(bound = "")]` to drop the auto-generated `S: Deserialize` bound; `_scope` is `#[serde(skip)]`. After deserialization, a `validate()` method checks that every value in `args` is in `S::ALLOWED`, returning a load error otherwise.

```rust
impl<S: HookScope> OsHook<S> {
    pub fn validate(&self) -> Result<(), String> {
        for arg in &self.args {
            if !S::ALLOWED.contains(arg) {
                return Err(format!("arg {:?} is not valid for this hook", arg));
            }
        }
        Ok(())
    }
}
```

OS recipe struct:

```rust
pub struct PkgHookDefs {
    pub install: OsHook<PkgScope>,
    pub remove:  OsHook<PkgScope>,
}

pub struct NetworkHookDefs {
    pub transcribe: OsHook<TranscribeScope>,
    pub enable:     OsHook<SimpleNetworkScope>,
    pub disable:    OsHook<SimpleNetworkScope>,
}
```

### `AssignmentYaml` — gains `id`, `Environment` fields reordered

```rust
pub struct AssignmentYaml {
    pub id: Id,
    pub name: String,
    pub environment: Environment,
}

pub struct Environment {
    pub os: OsId,
    pub platform: PlatformSpec,
    #[serde(default)]
    pub toolchains: Vec<Versioned<ToolchainId>>,
}
```

### Platform and toolchain hooks — unchanged

`HookDef { steps: Vec<Step> }` stays as-is. No `args`, no `use_params`. Steps use `$DEV` directly as a shell env var. Platform params are resolved by the resolver before steps are interpolated.

### `BuildPlan` — gains `id`

```rust
pub struct BuildPlan {
    pub id: Id,
    pub name: String,
    pub base_image: String,
    pub layers: Vec<Layer>,
    pub entrypoint: String,
}
```

## YAML changes

### OS recipe (`ubuntu-22.yaml`)

`params:` block replaced with `args:` list of `HammerParam` string values:

```yaml
pkg:
  install:
    args: [packages]
    steps:
      - run:
          - apt-get update && apt-get install -y {packages}
  remove:
    args: [packages]
    steps:
      - run:
          - apt-get remove --purge -y {packages}

network:
  transcribe:
    args: [rules]
    steps:
      - run:
          - for cidr in {rules}; do iptables -A OUTPUT -d "$cidr" -j ACCEPT; done
  enable:
    steps:
      - run:
          - iptables-restore < /etc/pedagog/network.rules
  disable:
    steps:
      - run:
          - iptables -F
          - iptables -P OUTPUT ACCEPT
```

### Platform recipe params (e.g. `interactive.yaml`)

`params:` entries gain optional `info:`:

```yaml
params:
  terminal:
    type: bool
    default: true
    info: "whether to enable terminal access for students"
  extensions:
    type: map
    info: "VS Code extension configuration"
    properties:
      install:
        type: list
        default: []
        info: "extension IDs to install at build time"
      allow:
        type: list
        default: []
        info: "additional extension IDs to permit at runtime"
```

### Assignment recipe (e.g. `pointers.yaml`)

Gains `id:` and `name:`, field order updated:

```yaml
id: pointers-and-memory
name: Pointers and Memory
environment:
  os: ubuntu-22
  platform:
    interactive:
      terminal: false
      extensions:
        install:
          - llvm-vs-code-extensions.vscode-clangd
        allow: []
  toolchains:
    - gcc:12
    - clangd:14
```

## Alternatives considered

- **`Vec<Id>` for `args`** — rejected; allows any arbitrary string, no validation.
- **Typed param struct as generic (`Hook<PkgParams>` with named fields)** — rejected; requires a custom `Deserialize` impl per hook type; `HookScope` + enum covers the constraint more cleanly.
- **`use_params` on platform/toolchain hooks** — rejected; DEV is an env var and platform params are already in scope. No declaration needed.
- **Remove `Map` from `ParamDef`** — rejected; `extensions` and similar user-defined compound params require nested structure.
- **`HammerParam::Dev`** — excluded; DEV is a Docker build arg exposed as `$DEV`, not a hammer-interpolated param.

## Open questions

- **Toolchain `params` field** (`HashMap<String, ParamVal>` today — values, not declarations): should it become `HashMap<String, ParamDef>` to support user-configurable toolchain params? Deferred — no toolchain recipes use configurable params yet.
- **`OsHook` validation timing**: validate in `Deserialize` (via a custom impl) vs. post-load in `RecipeStore::load`. Post-load is simpler and chosen here; can move to `Deserialize` later.

## Rollback plan

All changes are local to `pedagog-core`, `hammer`, and the `recipes` submodule. No database migrations, no deployed state. Reverting the Rust changes restores the old types; the YAML changes in `recipes` are in a separate commit and can be reverted independently.

## Implementation steps

1. **`primitives.rs`**
   - Add `info: Option<String>` (with `#[serde(default)]`) to all five `ParamDef` variants.
   - Add `HammerParam` enum with `serde(rename_all = "lowercase")`.
   - Add `HookScope` trait.
   - Add `OsHook<S: HookScope>` struct with `args`, `steps`, `_marker`, custom `Deserialize`, and `validate()`.
   - Remove `Param` struct.
   - Remove `ParamHookDef` struct.
   - Update `ParamType` — no change needed (Map stays).

2. **`os.rs`**
   - Add `InitScope`, `PkgScope`, `TranscribeScope`, `SimpleNetworkScope` with `HookScope` impls.
   - Replace all `ParamHookDef` usages with the appropriate `OsHook<_>` type alias.
   - Remove the `Param` import.
   - Call `validate()` on each `OsHook` inside a post-load `OsDef::validate()` method, called from `RecipeStore::load`.

3. **`assignment.rs`**
   - Add `id: Id` before `name`.
   - Reorder `Environment` fields to `os`, `platform`, `toolchains`.

4. **`params.rs` (hammer)**
   - `resolve_one` match arms: add `info` field binding with `..` or `_` — no logic change.

5. **`resolve/mod.rs` (hammer)**
   - Update `expand_pkg_install` signature from `&ParamHookDef` to `&OsHook<PkgScope>`.
   - Propagate `assignment.id.clone()` into `BuildPlan`.

6. **`resolve/plan.rs` (hammer)**
   - Add `id: Id` to `BuildPlan`.

7. **`render/describe.rs` and `render/containerfile.rs` (hammer)**
   - Use `plan.id` where appropriate (e.g. image name, describe root label).

8. **YAML — `ubuntu-22.yaml`** (recipes repo)
   - Replace `params:` blocks with `args:` on `pkg.install`, `pkg.remove`, `network.transcribe`.

9. **YAML — platform recipes** (recipes repo)
   - Add `info:` fields to `params:` entries.

10. **YAML — `pointers.yaml`** (examples)
    - Add `id: pointers-and-memory` and `name: Pointers and Memory`.
    - Reorder `environment` fields.

11. **Tests**
    - `primitives.rs`: tests for `ParamDef` deserialization with `info`; `HammerParam` round-trip; `OsHook` scope validation (valid and invalid args).
    - `os.rs`: test that `OsDef` with a bad arg (e.g. `rules` on a pkg hook) fails validation.
    - Update any existing tests referencing `Param`, `ParamHookDef`.
    - Update resolver tests if any reference `assignment.name` → also check `assignment.id`.
