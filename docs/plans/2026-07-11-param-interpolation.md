# Plan: Recipe Param Interpolation

**Date:** 2026-07-11
**Status:** Draft

---

## Rationale

Platform recipes already declare a `params` block (e.g. `terminal: true`, `extensions.install: []`), but those values are never threaded into recipe steps. The only interpolation that exists today is the hard-coded `{packages}` substitution in `expand_pkg_install`.

We need a general-purpose interpolation pass so that:
- Platform steps can use `{terminal}`, `{extensions.install}`, etc.
- Recipes can write files like `policy.json` using `{extensions.install:json}` for JSON-serialized values
- The same pass applies uniformly to all resolved steps (OS, platform, toolchain)

---

## Design

### Interpolation syntax

Two forms, both using dot-notation for nested access:

| Form | Example | Output |
|---|---|---|
| `{key.path}` | `{extensions.install}` | Default: space-joined for lists, stringified scalars |
| `{key.path:json}` | `{extensions.install:json}` | serde_json serialization of the value |

Default rendering rules:
- `Bool` → `"true"` / `"false"`
- `Int` → decimal string
- `Str` → as-is
- `List` → space-joined (elements rendered as scalars)
- `Map` → error; use `:json` instead
- Missing key → hard error at plan resolution time

### Typed params with defaults

Platform recipes declare params with an explicit type and an optional default:

```yaml
# interactive.yaml
params:
  terminal:
    type: bool
    default: true
  ai:
    type: bool
    default: false
  extensions:
    type: map
    default:
      install: []
      allow: []
```

Types map 1:1 onto `ParamVal` variants: `bool`, `int`, `str`, `list`, `map`.

A param with no `default` is required — hammer errors at plan time if the assignment omits it.

### Assignment platform spec

The platform kind becomes a key, with params nested under it:

```yaml
# pointers.yaml
environment:
  os: ubuntu-22
  platform:
    interactive:
      terminal: true
      extensions:
        install: [clangd]
  toolchains:
    - gcc:12
```

Unspecified params fall back to the recipe's declared default. Hammer validates that each provided value matches the declared type.

### Param sources and merge

1. Platform recipe `params:` → typed declarations with optional defaults
2. Assignment `environment.platform.<kind>:` → value overrides (must type-check against declarations)
3. Hammer merges: assignment values over recipe defaults, deep-merge for `Map` params

OS and toolchain recipes have no `params` block. They receive the resolved param map and interpolation runs on their steps too — steps without `{...}` tokens are unchanged.

---

## Type system: `ParamType` and `ParamDef`

`ParamType` is the discriminant-only counterpart to `ParamVal` — the same five variants, no data:

```rust
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    Bool,
    Int,
    Str,
    List,
    Map,
}
```

`ParamVal` gains a `.param_type() -> ParamType` method so type-checking a provided value against a declaration is a single comparison.

`ParamDef` is a recursive enum, one variant per type. Each scalar variant carries its own typed default; `Map` carries typed sub-property declarations instead of a flat default (the effective default is computed from its properties' defaults recursively):

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ParamDef {
    Bool { default: Option<bool> },
    Int  { default: Option<i64> },
    Str  { default: Option<String> },
    List { default: Option<Vec<ParamVal>> },
    Map  { properties: HashMap<String, ParamDef> },
}
```

In YAML:
```yaml
params:
  terminal:
    type: bool
    default: true
  extensions:
    type: map
    properties:
      install:
        type: list
        default: []
      allow:
        type: list
        default: []
```

`ParamDef` replaces `ParamVal` as the value type in `PlatformRecipe.params`. The enum approach prevents invalid combinations (e.g. `type: bool` with `properties:`) at the schema level.

---

## Alternatives Considered

- **Keep `{packages}` as a special case, add new special cases for extensions etc.** — rejected; ad-hoc accumulation, not a general solution.
- **New `file:` step type** — considered for writing policy.json; deferred in favour of `run:` heredocs with `{...:json}` interpolation. Keeps the step type set minimal.
- **Compile-time templates (Tera/Minijinja)** — too heavy; the syntax above is sufficient and trivially implementable without a dep.
- **Untyped params (values-as-defaults only)** — rejected in favour of explicit typing; reuses the `ParamVal` variant structure at negligible cost and enables early validation.

---

## Open Questions

- None blocking implementation.

---

## Rollback Plan

All changes are additive to the schema (new fields are `#[serde(default)]` where possible). Reverting means removing the interpolation call from `resolve_step`, the new `ParamType`/`ParamDef` types, and restoring `PlatformRecipe.params` to `HashMap<String, ParamVal>` and `AssignmentYaml.Environment.platform` to `PlatformKind`.

---

## Implementation Steps

### 1. `pedagog-core`: add `ParamType` and `ParamDef` to `primitives.rs`

```rust
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ParamType { Bool, Int, Str, List, Map }

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ParamDef {
    Bool { default: Option<bool> },
    Int  { default: Option<i64> },
    Str  { default: Option<String> },
    List { default: Option<Vec<ParamVal>> },
    Map  { properties: HashMap<String, ParamDef> },
}
```

Add `.param_type() -> ParamType` to `ParamVal`:
```rust
impl ParamVal {
    pub fn param_type(&self) -> ParamType {
        match self {
            ParamVal::Bool(_) => ParamType::Bool,
            ParamVal::Int(_)  => ParamType::Int,
            ParamVal::Str(_)  => ParamType::Str,
            ParamVal::List(_) => ParamType::List,
            ParamVal::Map(_)  => ParamType::Map,
        }
    }
}
```

Add `#[derive(Serialize)]` to `ParamVal` (needed for `:json` format in hammer). The `serde` dep in `crates/core/Cargo.toml` already has `derive`; add `features = ["derive"]` if not already set.

### 2. `pedagog-core`: update `PlatformRecipe`

In `platform.rs`, change `params` field type:
```rust
pub params: HashMap<String, ParamDef>,
```

### 3. `pedagog-core`: update `AssignmentYaml`

The `platform` field changes from a bare `PlatformKind` to a single-key map that carries the param overrides:

```rust
// New type in platform.rs or assignment.rs
pub struct PlatformSpec {
    pub kind: PlatformKind,
    pub params: HashMap<String, ParamVal>,
}
```

Deserializes from:
```yaml
platform:
  interactive:          # key = PlatformKind
    terminal: true      # values = param overrides
```

`Environment.platform` becomes `PlatformSpec`.

### 4. `hammer`: add `serde_json` dependency

```toml
serde_json = { workspace = true }
```

Add to workspace `Cargo.toml` if not already present.

### 5. `hammer`: new `crates/hammer/src/params.rs`

**`resolve_params`** — validates and merges declarations + overrides into a flat `HashMap<String, ParamVal>`:
```rust
pub fn resolve_params(
    decls: &HashMap<String, ParamDef>,
    overrides: &HashMap<String, ParamVal>,
) -> Result<HashMap<String, ParamVal>, String>
```
- For each declared param: use override if present (type-check), else use default, else error (required)
- For `Map` params: recursively merge override keys onto default map
- Unknown keys in overrides: error (no undeclared params allowed)

**`interpolate`** — substitutes `{key.path}` and `{key.path:json}` tokens:
```rust
pub fn interpolate(
    cmd: &str,
    params: &HashMap<String, ParamVal>,
) -> Result<String, String>
```
- Regex: `\{([a-zA-Z_][a-zA-Z0-9_.]*?)(?::([a-z]+))?\}`
- Navigate dot-path through `ParamVal` map
- Format per specifier (none = default, `json` = serde_json)
- `Err` if key not found or format unsupported

### 6. `hammer/resolve/mod.rs`: thread params through resolution

- `resolve_build` calls `resolve_params(platform.params, assignment.platform.params)` → `Result<HashMap<String, ParamVal>, _>`
- Passes the resolved map into `resolve_steps`
- `resolve_step` calls `interpolate` on every `Command` string
- `{packages}` substitution in `expand_pkg_install` kept as-is (no conflict — runs in a separate code path)

### 7. Update recipes and assignment

- `interactive.yaml`: convert `params:` block to typed `ParamDef` format; add `policy.json` and machine settings write steps using `{extensions.install:json}`
- `pointers.yaml`: update `platform:` to new nested format with param overrides

### 8. Tests

In `crates/hammer/src/params.rs`:
- `resolve_params` with all defaults
- `resolve_params` with override (correct type)
- `resolve_params` with override (wrong type) → error
- `resolve_params` with missing required param → error
- `resolve_params` with unknown key → error
- `resolve_params` map deep-merge
- `interpolate` scalar bool, int, string
- `interpolate` list (space-joined)
- `interpolate` `:json` list and map
- `interpolate` dot-nested path
- `interpolate` missing key → error
