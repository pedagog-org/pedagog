# hammer vend

**Date:** 2026-07-13
**Status:** Draft

## Rationale

Recipes can declare local dev assets (e.g. a custom code-server `.deb`) that speed up
or enable DEV image builds. Currently these are managed ad-hoc via a hand-written
Justfile. `hammer vend` makes vendoring a first-class operation: recipes declare what to
vend, `hammer vend` fetches them into a well-known location, and recipe steps check for
the vendored file before falling back to downloading from source.

## Alternatives Considered

- **Sidecar files** (`interactive.vend.yaml`) — more files, harder to cross-reference.
- **Justfile per platform** — not integrated with hammer, no uniform CLI.
- **Always fetch at build time** — requires network access and credentials inside the
  container; slower.

## Design

### `ingredients:` in recipe YAML

Added to `OsDef`, `PlatformRecipe`, and `ToolchainRecipe`. Each entry has an `output`
filename and either a `github` release asset or a plain `url`:

```yaml
# platforms/ubuntu-22/interactive.yaml
ingredients:
  - output: code-server.deb
    github:
      repo: pedagog-org/code-server
      asset: "code-server_*_arm64.deb"
      tag: v4.127.0-pedagog.1

# hypothetical toolchain
# toolchains/ubuntu-22/zig/0.14.yaml
ingredients:
  - output: zig-0.14.tar.xz
    url: https://ziglang.org/download/0.14.0/zig-linux-aarch64-0.14.0.tar.xz
```

If multiple assets match a `github.asset` glob, `hammer vend` hard-errors and lists the
matched filenames so the recipe author can tighten the pattern.

### `vend` shell function

A script at `recipes/lib/vend` is installed to `/usr/local/bin/vend` by every OS init
hook — this is part of the OS recipe contract. It resolves a filename to its vendored
path using two env vars that hammer emits before each recipe section's steps:

```sh
#!/bin/sh
echo "/pedagog/ingredients/${PEDAGOG_TYPE}/${PEDAGOG_ID}/$1"
```

Recipe steps use it as:
```bash
if [ -f "$(vend code-server.deb)" ]; then
  dpkg -i "$(vend code-server.deb)"
else
  curl -fsSL "https://..." -o /tmp/code-server.deb
  dpkg -i /tmp/code-server.deb && rm /tmp/code-server.deb
fi
```

### Context env vars

Hammer emits `ENV` instructions before each recipe section so `vend` knows its context:

```dockerfile
# Before platform steps:
ENV PEDAGOG_TYPE=platform
ENV PEDAGOG_ID=interactive

# Before toolchain steps:
ENV PEDAGOG_TYPE=toolchain
ENV PEDAGOG_ID=gcc/12
```

### Output location

Files land at `$recipe_root/ingredients/<type>/<id>/<output>` where `$recipe_root` is
the directory the recipe was loaded from. Extra `--recipes DIR` dirs vend into their own
`ingredients/` tree.

```
$HAMMER_RECIPES/
  lib/
    vend                       ← shell script, tracked in git
  ingredients/
    .keep                      ← only tracked file under ingredients/
    platform/
      interactive/
        code-server.deb        ← gitignored, written by hammer vend
    os/                        (created by hammer vend as needed)
    toolchain/                 (created by hammer vend as needed)
```

`ingredients/.keep` anchors the directory in git. Everything else under `ingredients/`
is gitignored:
```
ingredients/**
!ingredients/.keep
```

### CLI

```
hammer vend                          # vend everything from all recipe dirs
hammer vend --assignment FILE
hammer vend --platform ID
hammer vend --os ID
hammer vend --toolchain ID
hammer vend --recipes DIR            # extra recipe dir; its recipes also vended
```

`--recipes` is present on both `plan` and `vend`.

### Output (hammer vend, no flags)

```
Vending platform/interactive ... code-server.deb ✓
Vending os/ubuntu-22 ... nothing to vend
Vending toolchain/gcc:12 ... nothing to vend
```

### Download strategy

- `github` source → `gh release download` (uses ambient `gh` auth)
- `url` source → `curl -fsSL`

Both shell out rather than pulling in new Rust dependencies.

## Rust Types (primitives.rs)

Requires adding `url` as a new workspace dependency (`url = "2"`).

```rust
#[derive(Debug, Deserialize)]
pub struct Ingredient {
    pub output: String,
    #[serde(flatten)]
    pub source: IngredientSource,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IngredientSource {
    Github(GithubSource),
    Url(url::Url),
}

#[derive(Debug, Deserialize)]
pub struct GithubSource {
    pub repo: String,
    pub asset: String,
    pub tag: String,
}
```

Each recipe struct gains:
```rust
#[serde(default)]
pub ingredients: Vec<Ingredient>,
```

## Rollout Plan

No rollback needed — purely additive. `ingredients:` is `#[serde(default)]` so all
existing recipe YAMLs without it continue to parse unchanged.

## Implementation Steps

1. **`Cargo.toml`** — add `url = "2"` to workspace dependencies; add to `pedagog-core`.
2. **`primitives.rs`** — add `Ingredient`, `IngredientSource`, `GithubSource` types + tests.
3. **`os.rs`** — add `#[serde(default)] pub ingredients: Vec<Ingredient>` to `OsDef`.
4. **`platform.rs`** — add same to `PlatformRecipe`.
5. **`toolchain.rs`** — add same to `ToolchainRecipe`.
6. **`cli.rs`** — add `Vend(VendArgs)` variant; `VendArgs` has target group
   (`--assignment`, `--platform`, `--os`, `--toolchain`, all optional — omitting all
   vends everything) plus `--recipes`.
7. **`hammer/src/vend/mod.rs`** — implement: collect target recipes, iterate
   `ingredients`, shell out to `gh`/`curl`, create output dirs, print per-item status.
8. **`main.rs`** — wire `Command::Vend(args) => run_vend(args)`.
9. **`recipes/`**:
   - Add `lib/vend` shell script.
   - Add `ingredients:` block to `platforms/ubuntu-22/interactive.yaml`.
   - Add `install -m755 /pedagog/recipes/lib/vend /usr/local/bin/vend` to
     `os/ubuntu-22.yaml` init hook.
   - Add `ingredients/.keep`; add `recipes/.gitignore` with `ingredients/**` /
     `!ingredients/.keep`.
   - Create `recipes/examples/pointers.yaml` (moved from
     `pedagog/examples/assignments/pointers.yaml`).
10. **`pedagog/`** — delete `dev/`; delete `examples/`; remove `images/` remainder;
    update `ARCHITECTURE.md`.
11. **Tests** — unit tests for `Ingredient` deserialization (github + url variants,
    default-empty); smoke test that vending a recipe with no `ingredients:` prints
    "nothing to vend".
