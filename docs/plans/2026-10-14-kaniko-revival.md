# Kaniko OS Builds: Revival

**Date:** 2026-10-14
**Status:** Implemented
**Builds on:** [2026-08-06-jobs-kaniko-os-builds](2026-08-06-jobs-kaniko-os-builds.md) (the
design; unchanged except where noted here)

## Rationale

`feat/jobs-kaniko-os-builds` implements steps 1–10 of the August plan (`store`, `k8s::build`,
`jobs` dispatch and endpoint, `jobs` image, deploy manifests, docs). It has sat unmerged for
10 weeks while `main` moved on: design-doc-v2 (typed `ResolveError`, registry semantics,
recipe schema), CI, a pinned toolchain, and enforced rustfmt. The design still holds. This
plan covers what it takes to land the branch and finally run its end-to-end test (step 11),
which never happened.

## Findings

- **Conflicts:** a trial merge conflicts in only three files: `Cargo.toml` (workspace
  dependencies), `crates/k8s/src/lib.rs`, `docs/ARCHITECTURE.md`.
- **Core API:** `jobs` uses `resolve_base`, `RenderOptions { registry: None, from: Standalone }`,
  `ImageSpec::Base` and `RecipeStore::list_oses`. All still exist; `resolve_base` now returns
  `ResolveError` (implements `Error`) instead of `String`, which its `anyhow` wrapping
  absorbs. The base-image render is unaffected by the registry change (a base never gets a
  prefix). OS Containerfiles changed since August (no network hooks, noninteractive apt), so
  their hashes differ and the first run rebuilds every OS. That's intended.
- **Lint/format:** the branch predates the rustfmt gate and the 1.97.1 pin, so it needs a
  `cargo fmt` pass and possibly clippy fixes.
- **CI:** four `#[sqlx::test]` tests need a live Postgres, and `sqlx::query!` needs either a
  database or the committed `.sqlx/` cache at compile time.
- **Bug: the `jobs` image can't vend.** Its vend stage runs `hammer vend`, which fetches
  `github:` ingredients through the `gh` CLI. The Rust build image has no `gh`, and `gh`
  needs a login even for public releases.
- **Dev overlay vs kind:** the overlays assume k3s with Longhorn, MetalLB and cert-manager,
  and dev mode needs the recipes checkout at a host path on the node.
- **Floating base image:** the `jobs` Containerfile uses `rust:1-bookworm`, which doesn't
  match the pinned toolchain.

## Design changes

### Integration

Rebase the branch's six commits onto `main`, resolving the three conflicts as they arise.
Then add follow-up commits: compile fixes, `cargo fmt`, clippy. One PR.

### CI with Postgres

Extend `.github/workflows/ci.yml`:

- a `postgres:16-alpine` service container (user, password and db `pedagog`, health-checked);
- `DATABASE_URL=postgres://pedagog:pedagog@localhost:5432/pedagog` for the test step, so
  `#[sqlx::test]` runs for real;
- `SQLX_OFFLINE=true` for build, clippy and test, so compilation uses the committed `.sqlx/`
  cache and never depends on the database schema at compile time;
- a `cargo sqlx prepare --workspace --check` step (sqlx-cli installed via
  `taiki-e/install-action`, a prebuilt binary rather than a slow `cargo install`) that fails
  when a query changed without regenerating `.sqlx/`.

### `hammer vend` without `gh`

`github:` ingredients resolve through GitHub's public REST API with `curl`, matching how
`url:` ingredients already download:

1. `GET https://api.github.com/repos/{repo}/releases/tags/{tag}` (curl), parsed with
   `serde_json`.
2. Pick the single asset whose name matches the recipe's `asset` pattern. `*` is the only
   wildcard, matched by a small hand-rolled function. Zero or several matches is an error
   that lists the release's asset names.
3. Download its `browser_download_url` with `curl -fsSL`.

If `GH_TOKEN` (or `GITHUB_TOKEN`) is set, it's sent as a bearer token, which raises the
anonymous rate limit of 60 requests per hour; it's never required for public repos. Adds
`serde_json` back to hammer (it's a workspace dependency already used by core).

### `kind` overlay for local end-to-end tests

- `deploy/kind/cluster.yaml`: a kind cluster config that mounts the repo's `recipes/` into
  the node at `/srv/pedagog/recipes` (`extraMounts`). Podman's VM already shares the home
  directory, so the path chain is Mac → VM → kind node.
- `deploy/overlays/kind/`: base resources minus Longhorn, MetalLB and cert-manager.
  - A `pedagog-storage` StorageClass backed by kind's `rancher.io/local-path` provisioner.
  - The dev jobs patch with both host paths set to `/srv/pedagog/recipes`.
  - The jobs image set to `localhost/pedagog/jobs:dev` with `imagePullPolicy: Never`, loaded
    into the node with `kind load image-archive`.
- `just kind-up` creates the cluster from the config, `just kind-load-jobs` builds the jobs
  image with podman and loads it, `just kind-deploy` runs `kubectl apply -k
  deploy/overlays/kind`, and `just kind-down` deletes the cluster. Your current `pedagog`
  kind cluster is recreated by `kind-up`, since mounts can only be set at creation.
- `dev` and `prod` overlays are unchanged.

### Pin the jobs build image

Change `rust:1-bookworm` to `rust:1.97.1-bookworm`, matching `rust-toolchain.toml`.

## Alternatives considered

- **Merge `main` into the branch / re-implement** — rejected; a rebase keeps history linear
  and the design doesn't need rework.
- **Skip DB tests in CI** — rejected; the build-state code would only be tested by hand.
- **`gh` + a token in the image build / `url:`-only ingredients** — rejected; a mandatory
  token for public downloads, or losing tag/asset pinning.
- **Make the dev overlay generic for kind** — rejected; kind-specific storage and paths would
  leak into the k3s dev overlay. A separate overlay keeps both honest.
- **Test only on the Pi cluster** — deferred; the Pis aren't reachable yet, and kind gives a
  repeatable local path (and a future CI candidate).

## Open questions

- **kind ≠ k3s:** kind runs Kubernetes 1.37 with containerd, the Pis run k3s 1.31. The
  `k8s-openapi` feature (`v1_32`) works against both, but the real cluster still needs one
  run when it's reachable.
- **Postgres in kind:** the base Postgres Deployment and PVC run as-is on local-path storage.
  No separate dev database needed in-cluster.
- **Registry access from the node:** Kaniko pushes to `registry-service.pedagog-data.svc:5000`
  over cluster DNS, which is enough for OS builds. Pulling those images back onto the node
  (needed later for assignment builds) requires containerd registry config in kind. Out of
  scope here.
- **August open questions** (image size, vend-time network, quota sizing, full input
  hashing) still stand.

## Rollback

The branch is additive (new crates, manifests, a migration). Reverting the PR removes it.
The kind overlay and Justfile tasks are dev-only. The CI changes revert with the workflow
file.

## Implementation steps

On the existing branch, rebased:

1. Rebase `feat/jobs-kaniko-os-builds` onto `main`, resolving the three conflicts.
2. Compile fixes against the current core API, if any.
3. `cargo fmt`, then clippy `-D warnings` on 1.97.1.
4. `hammer vend`: curl + GitHub API for `github:` ingredients, with tests for the wildcard
   matcher and the asset-selection errors (API calls stay behind a small function, so the
   tests don't hit the network).
5. `jobs` Containerfile: pin `rust:1.97.1-bookworm`.
6. CI: Postgres service, `SQLX_OFFLINE`, `cargo sqlx prepare --check`.
7. kind: `deploy/kind/cluster.yaml`, `deploy/overlays/kind/`, Justfile `kind-*` tasks,
   `docs/SETUP.md` section.
8. End-to-end on kind (August step 11):
   - `just kind-up kind-load-jobs kind-deploy`;
   - `POST /internal/recipes/rebuild` via port-forward;
   - Kaniko builds `ubuntu-22` and pushes to the in-cluster registry;
   - `os_builds` and `build_runs` rows land in Postgres;
   - a second call dispatches nothing (hash matches); `force=true` rebuilds.
9. Update the August plan's status line ("Implemented; see kaniko-revival") and
   ARCHITECTURE.md where the merge touched it. Open the PR.

## Implementation notes

Deviations and findings while implementing:

- **The rebase needed no compile fixes**, and the branch was already rustfmt- and
  clippy-clean on 1.97.1. `.sqlx/` was current.
- **`.containerignore` added:** the jobs image's `COPY . .` would otherwise send `target/`
  (several GB) and `.git` as build context.
- **The kind overlay uses the full base.** Longhorn and MetalLB were never in `base/`, and
  cert-manager and Traefik run fine on kind (the ClusterIssuer lives only in the dev/prod
  overlays). Dev secrets come from a `secretGenerator`, so nothing is created by hand.
- **Bug fixed: `force=true` was a no-op** while the previous Job for the same hash existed
  (up to its 1 h TTL). `ensure()` skipped any succeeded Job, and Job names derive from the
  hash. `ensure()` now takes `IfBuilt::{Skip, Rebuild}`.
- **End-to-end on kind:** `ubuntu-22` built by Kaniko and pushed to the in-cluster registry,
  `os_builds` and `build_runs` rows recorded, a second call dispatched nothing, `force=true`
  rebuilt (after the fix). The jobs image vended `code-server.deb` with the new curl-based
  vend.
- **Kaniko upstream is archived** (Google, 2025). `gcr.io/kaniko-project/executor:v1.24.0`
  still pulls and works; a maintained fork (e.g. Chainguard's) is the follow-up if that
  changes.
