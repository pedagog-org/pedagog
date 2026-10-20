# Pedagog

A platform for secure browser-based coding assignments.

## Getting Started

See [docs/SETUP.md](docs/SETUP.md) for infrastructure setup instructions.

## Development

Run once per clone to enable the git hooks (a pre-commit hook runs `cargo fmt --check`
and `cargo clippy --workspace --all-targets --all-features -- -D warnings`, rejecting
unformatted code or any warning, and, when a query or migration is staged, checks the
`.sqlx/` cache against an empty database (`cargo xtask db-prepare-check`; needs `cargo xtask db-up`).
Bypass with `git commit --no-verify`):

```sh
cargo xtask install-hooks
```

CI (`.github/workflows/ci.yml`) runs the same checks plus `cargo test --workspace` on
every pull request and push to `main`, with a Postgres service for the database tests, and
fails if the committed `.sqlx/` query cache is stale (regenerate with `cargo xtask db-prepare`).
The toolchain is pinned in `rust-toolchain.toml`.

To keep `git blame` from attributing lines to bulk-formatting commits:

```sh
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

Tests resolve the real recipe corpus in the `recipes/` submodule and fail if it isn't
checked out. Fetch it once per clone (or set `PEDAGOG_SKIP_RECIPE_CORPUS=1` to skip
those tests):

```sh
git submodule update --init recipes
```

## Development tasks (`cargo xtask`)

Every development task is a `cargo xtask` subcommand (Rust, in `crates/xtask`; no shell
scripts): `cargo xtask --help` lists them. The main ones:

| Task | What it does |
| --- | --- |
| `db-up`, `db-down`, `db-shell` | The dev Postgres in podman |
| `db-prepare`, `db-prepare-check` | Refresh or check `.sqlx/` against an empty, migrated database |
| `precommit` | What the git hook runs: fmt, clippy, and the sqlx cache when queries change |
| `web-dev`, `web-build` | The web UI with trunk (http://localhost:3000) |
| `kind-up`, `kind-load all`, `kind-deploy`, `kind-down` | The local kind cluster |
| `kind-api` | Port-forward the api, Mailpit and GlitchTip |
| `kind-add-admin EMAIL`, `kind-login EMAIL`, `kind-as NAME ARGS…` | Personas on kind |
| `e2e [FILTER…]` | The end-to-end tests on kind |
| `plan`, `describe`, `vend`, `build-base`, `build`, `squash`, `run`, `dev` | Build and run assignment images locally |

### Building & running assignments locally

Needs `podman` and a `recipes` checkout (`os/`, `platforms/`, `toolchains/`) in
`HAMMER_RECIPES`. Images are tagged `localhost/pedagog/<assignment-basename>`; `run`
mounts a working directory at `/workspace` (`--workdir`, default `.`), exposes a port
(`--port`, default 8080) and caps each container at 512 MiB, 1 CPU and 256 processes.

- **`plan ASSIGNMENT`**: the generated Containerfile (FROM the pre-built base image).
- **`describe ASSIGNMENT`**: what it builds, as a tree: phases, layers, steps, runtime and
  network policy.
- **`vend [FILTERS…]`**: download recipe ingredients (e.g. the code-server `.deb`) into
  `recipes/ingredients/`, so builds don't download at build time
  (`--os/--platform/--toolchain <ID>`).
- **`build-base OS_ID IMAGE_TAG`**: a base OS image from an OS recipe (once per OS recipe,
  or when it changes).
- **`build ASSIGNMENT`**, **`squash ASSIGNMENT`** (reclaim space from deleted files),
  **`run ASSIGNMENT [ARGS…]`**, **`dev ASSIGNMENT [ARGS…]`** (build, then run).

```sh
cargo xtask build-base ubuntu-22 pedagog/ubuntu:22   # once per OS recipe
cargo xtask plan ../recipes/examples/pointers.yaml   # what it will build
cargo xtask dev ../recipes/examples/pointers.yaml    # build + run
cargo xtask squash ../recipes/examples/pointers.yaml # optionally, after a build
```
