# Pedagog

A platform for secure browser-based coding assignments.

## Getting Started

See [docs/SETUP.md](docs/SETUP.md) for infrastructure setup instructions.

## Development

Run once per clone to enable the git hooks (a pre-commit hook runs
`cargo clippy --workspace --all-targets -- -D warnings`, rejecting commits with any
warning — bypass with `git commit --no-verify`):

```sh
just install-hooks
```

## Building & Running Assignments (Justfile)

The root [`Justfile`](Justfile) wraps `hammer` (the recipe-resolution CLI) and `podman`
to build and run assignment images locally.

### Prerequisites

- `just` installed
- `podman` installed
- A `recipes` checkout (`os/`, `platforms/`, `toolchains/`), pointed to either by:
  - the `HAMMER_RECIPES` environment variable, or
  - the Justfile's `RECIPES` variable (`just RECIPES=/path/to/recipes ...`)

### Variables

Override any of these on the command line, e.g. `just PORT=3000 run ...`.

| Variable     | Default                          | Purpose                                       |
| ------------ | --------------------------------- | ---------------------------------------------- |
| `PORT`       | `8080`                            | Host/container port exposed by `run`/`dev`    |
| `WORKDIR`    | `.`                                | Host directory mounted to `/workspace`        |
| `REGISTRY`   | `localhost`                       | Image tag prefix; set to a real registry to push/pull from one |
| `RECIPES`    | `$HAMMER_RECIPES`                 | Path to the recipes checkout                  |
| `MEMORY`     | `512m`                            | Per-container memory limit (swap disabled)    |
| `CPUS`       | `1`                                | Per-container CPU limit                       |
| `PIDS_LIMIT` | `256`                              | Per-container process limit                   |

### Tasks

- **`just plan ASSIGNMENT`** — print the generated Containerfile for an assignment
  (e.g. `just plan ../recipes/examples/pointers.yaml`).
- **`just vend [FILTERS...]`** — download and cache recipe ingredients (e.g. the
  code-server `.deb`) into `recipes/ingredients/`, so builds use vendored assets instead
  of downloading at build time. Optional filters: `--os/--platform/--toolchain <ID>`.
- **`just build-base OS_ID IMAGE_TAG`** — build a base OS image from an OS recipe
  (e.g. `just build-base ubuntu-22 pedagog/ubuntu:22`). Run this once per OS recipe,
  or whenever the OS recipe changes.
- **`just build ASSIGNMENT`** — build an assignment's image, tagged
  `<REGISTRY>/pedagog/<assignment-basename>`.
- **`just squash ASSIGNMENT`** — squash an already-built assignment image's layers in
  place to reclaim space from files deleted during the build.
- **`just run ASSIGNMENT [ARGS...]`** — run a built assignment image: mounts `WORKDIR`
  to `/workspace`, exposes `PORT`, and applies the `MEMORY`/`CPUS`/`PIDS_LIMIT` caps.
- **`just dev ASSIGNMENT [ARGS...]`** — `build` then `run` in one step.

### Typical workflow

```sh
# once per OS recipe
just build-base ubuntu-22 pedagog/ubuntu:22

# inspect what an assignment will build before running it
just plan ../recipes/examples/pointers.yaml

# build + run
just dev ../recipes/examples/pointers.yaml

# or separately
just build ../recipes/examples/pointers.yaml
just run ../recipes/examples/pointers.yaml

# optionally, after a build, reclaim space from deleted files
just squash ../recipes/examples/pointers.yaml
```
