# Plan: Jobs Service — In-Cluster Kaniko OS Base Builds

**Date:** 2026-08-06
**Status:** Pending review
**Supersedes:** the unmerged `2026-07-14-jobs-kaniko-os-builds.md` (never landed on `main`; not carried into the repo — recoverable from the git object store. See *What changed* below.)

---

## What changed since the 2026-07-14 plan

The 07-14 plan predates the **resolve/render refactor** (`2026-07-21-recipe-resolve-render-refactor.md`, now landed). Recipe resolution and Containerfile rendering moved out of `hammer` into `pedagog-core`:

- `core::recipe::resolve::{resolve, resolve_base}` → `ImageSpec`
- `core::recipe::render::{ImageSpec, Containerfile, Render, RenderOptions}`
- `hammer` is now a thin CLI over `core`.

That invalidates the 07-14 premise (*"`jobs` shells out to the `hammer` binary because `hammer` is the single source of truth"*). `core` is the source of truth, and it's a **library**. Re-opening that cascaded into the decisions below, each reviewed with the user before writing. Two later decisions (pin-not-poll, hashes-in-DB) further reshaped it away from the 07-14 design.

---

## Rationale

ARCHITECTURE.md calls for all builds to run in-cluster via **Kaniko**, submitted as Kubernetes Jobs. This plan delivers the first in-cluster build path: a shared `k8s` crate that owns Kaniko orchestration, a `store` crate for persistent build state, and enough of the long-lived `jobs` service to build/push **OS base images** from the recipes pinned into the installation.

**Trigger/monitor split (still valid):** `api` initiates work with a clear external trigger (instructor upload, student start) by calling `k8s` directly. `jobs` owns work with *no* external trigger. OS base builds fall on the `jobs` side: nobody requests a rebuild; it's discovered by comparing the pinned recipes against recorded build hashes. Instructor image builds and session spin-up will later live in `api`, reusing `k8s::build`.

**Why `jobs` is separate from `api`:** background build work shouldn't degrade `api` latency for students mid-exam, and each service is scoped to only the k8s RBAC it needs.

---

## Scope

**In scope:**
- `crates/k8s` — client wrapper (`kube` + `k8s-openapi`) + a `build` submodule owning Kaniko job construction/submission/watch.
- `crates/store` — persistent build state via `sqlx` (Postgres), behind a `db` feature. **POC depth this milestone** — `os_builds` (current build per OS) + `build_runs` (attempt progress/failures); expand later.
- `crates/jobs` — startup + on-demand OS base builds: render via `core` in-process, diff against `store`, build via `k8s::build`.
- Deploy manifests: `jobs` Deployment, RBAC, ConfigMap-per-build, NetworkPolicy for registry access, `pedagog-builds` namespace, Postgres access, DB migration.
- `jobs` container image bakes the pinned recipes + vendored ingredients (`hammer vend` at image-build time).

**Out of scope (later milestones):**
- Instructor/assignment image builds (needs `core` DB models, archive intake, solution-vs-tests validation). Will reuse `k8s::build`.
- Submission test-run pipeline + state machine (`k8s::run`, later).
- `api`, auth. The full build-state schema — this milestone lands a POC slice only.
- Image GC / dependent tracking.

---

## Crate design & responsibilities

| Crate | Role this milestone | `kube`/`tokio`/`sqlx`? |
| --- | --- | --- |
| `core` | Owns resolve/render + `core::env` (`Env` enum). Shared domain types. | **No** — stays WASM-shareable |
| `store` | Persistent state. Plain model structs always available; `store::db` (sqlx/Postgres) behind `feature = "db"`. Embedded migrations. | `sqlx` only under `db` feature |
| `k8s` | Raw client wrapper + `k8s::build` (Kaniko) + `k8s::run` (later). **Only** crate touching `kube`/`k8s-openapi`. | Yes |
| `jobs` | Long-lived service. Links `core` (render) + `store` (`db`) + `k8s` (build). | Yes |
| `hammer` | Unchanged at runtime. Also invoked at **image-build time** to `vend` ingredients. | No |

**`k8s` layout:**
```
crates/k8s/src/
  lib.rs      # KubeClient — thin generic wrapper over kube::Client (shared by build/run)
  build.rs    # ALL build logic, grouped in a Builder: kaniko_job spec, ensure() dedup,
              #   per-build ConfigMap, wait, capture_logs — the one build entrypoint
  run.rs      # (later) session/submission pod construction
```

**`store` layout:**
```
crates/store/
  migrations/           # sqlx migrations, embedded via sqlx::migrate! (os_builds, build_runs)
  .sqlx/                # committed offline query cache (see Approved dependencies)
  src/
    lib.rs              # plain models (BuildStatus enum, …) — no sqlx, always available
    db/
      mod.rs            # #[cfg(feature="db")] connect() (MAX_CONNECTIONS const), run_migrations()
      builds.rs         # os_builds + build_runs queries (per-domain submodule)
```

**Decisive constraints behind this shape:**

1. **Build orchestration in `k8s`, DB access in `store` — not `core`.** `core` is shared with the Leptos **WASM** frontend, so it must not pull `kube`/`tokio`/`sqlx`. The `store` crate isolates the DB; its `db` feature is off by default, so a WASM consumer can depend on `store` for model types without dragging in `sqlx`. `jobs`/`api` enable `store/db`.
2. **`k8s::build` speaks a rendered `Containerfile` *string*, not `core::ImageSpec`.** Rendering is `core`'s job; submitting is `k8s`'s. This keeps `k8s` free of recipe types and lets `api` reuse the exact builder for instructor images later (only Containerfile, target tag, and context differ).

---

## Design

### Recipe delivery — pinned, baked, no pull loop

Recipes are **pinned to the installation** (the `recipes` submodule), not continuously polled. "Upgrade the installation → get new recipes." This drops the 07-14 pull loop, the sync interval, and all in-cluster `git`.

- **Image build (prod):** the `jobs` multi-stage Containerfile builds the `hammer`/`jobs` binaries, runs **`hammer vend`** against the pinned recipes to fetch ingredients, then `COPY`s recipes + vendored ingredients into the final `jobs` image. Ingredients are baked at release time → hermetic, offline Kaniko builds. (`hammer` is a *build-time* tool here; it is **not** in the runtime image — `jobs` links `core`.)
- **Delivering context to Kaniko pods — two shapes, chosen by env** (each Kaniko Job is built as a typed struct in code, see below):
  - **prod:** an **initContainer using the `jobs` image** (recipes+ingredients baked in) copies them into an **`emptyDir`** shared with the Kaniko container. The rendered Containerfile arrives via a per-build **ConfigMap**. **No `recipes-pvc`** — removes the RWX-Longhorn-on-RPi cost the 07-14 plan carried.
  - **dev:** the developer's local checkout is mounted RO **straight into the Kaniko container** as its context — **no initContainer** (nothing is baked to extract) and no jobs-image self-reference. Builds still run **in-cluster via Kaniko** — the real path end-to-end on k3s, not `podman build`.
  - The two shapes are one `match` in `k8s::build`; the branch reflects a real difference (baked-in-image vs on-host). Kaniko's invocation, the Containerfile ConfigMap, and the context *contents* are identical in both.

### Rendering & change detection

- For each `os/*.yaml`: load via `core::recipe::store::RecipeStore`, `resolve_base(os_id, &store)`, `Containerfile::render(&spec, &opts)` — **in-process**. No `hammer` subprocess.
- **Change unit = a content hash of the rendered Containerfile string.** Captures recipe-YAML *and* `core` render-logic changes.
- **Known gap (accepted, mitigated by force):** files copied by *path* at build time — notably `lib/vend` — aren't in the Containerfile text, so editing them won't change the hash. Force rebuild is the mitigation; full build-input hashing is future work.
- **Build state lives in Postgres** (`store::db`), not a PVC file — see *Build state — progress & failures* below. Hashing uses **`blake3`** (single crate).

### Build state — progress & failures

Two tables, queried through `store::db::builds`:

- **`os_builds`** — one row per OS id: the last **successfully** built Containerfile hash + image ref. What change-detection reads; written **only on success**.
- **`build_runs`** — one row per build *attempt*: `os_id`, `hash`, `image_ref`, a **`BuildStatus`** enum (`Running` → `Succeeded` | `Failed`), `started_at`/`finished_at`, and `error` (Kaniko logs on failure). The durable progress + failure history. `BuildStatus` is a plain Rust enum in `store` (WASM-safe) mapped to a `TEXT` column via `strum` (already a workspace dep) — not a Postgres `ENUM` type, so variants stay cheap to evolve during heavy schema iteration. **`strum` only guards the Rust *write* path** (a `BuildStatus` always serializes to a valid string); the `TEXT` column itself is unconstrained, so a raw `UPDATE`/migration/typo'd bind could store a bad value, caught only on read (a recoverable parse error). To make the DB the backstop without native-`ENUM` evolution friction, the column carries a **`CHECK (status IN ('running','succeeded','failed'))`** constraint — the check list is kept in sync with the enum in a migration when variants change.

Per attempt: `start_run` (`running`) **before** creating the Kaniko Job → on success mark `succeeded` and upsert `os_builds` → on failure capture pod logs into `build_runs.error`, mark `failed`. Persisting the failure first means the failed Job can be deleted on retry without losing the diagnosis. Each attempt runs in its **own spawned task** (see *Control flow*), so many attempts progress and finalize concurrently, each writing its own row on completion.

Progress is observable two ways: the live **k8s Job** while it runs (real-time; also where an API/UI streams pod logs, per ARCHITECTURE's live build-log streaming) and the durable **`build_runs`** rows after. `os_builds` stays the minimal source of truth for "does this need rebuilding."

### Crash / restart reconciliation

The k8s Job is the source of truth; `build_runs` is a projection. If `jobs` (or the cluster) restarts between `start_run` and `finish_run`, that row is left `Running` — but nothing is truly lost: k8s Jobs live in etcd and are named deterministically (`os-build-<id>-<hash>`), so the outcome is always recoverable from the cluster.

**Every dispatch pass starts by reconciling** the still-`Running` rows before new work. For each, poll its Job by name:
- **Succeeded** → `finish_run(Succeeded)` + upsert `os_builds`.
- **Failed** → capture logs → `finish_run(Failed)`.
- **Active** → still building; **re-adopt it** — spawn a task that `wait`s and finalizes on completion, exactly like a freshly dispatched build (completion-order). Doesn't block the dispatch pass.
- **Gone** (deleted / TTL-reaped / never created) → `finish_run(Failed, "interrupted")`. Outcome unknown but safe: if it had actually succeeded, `os_builds` still lacks the hash, so change-detection just rebuilds it (idempotent).

`build_runs` carries `image_ref` so a reconcile-success can upsert `os_builds` without re-resolving. Reconcile runs at the **start of each dispatch pass** and adopts orphans as concurrent tasks; the short dispatch-lock (see *Control flow*) prevents adopting the same run twice. (The lightweight analogue of ARCHITECTURE's watchdog — reconcile-at-pass-start rather than a periodic lease sweep, since there's no continuous loop.)

### Control flow — dispatch-and-return, no ticker

Builds run as an **asynchronous background executor**, not a synchronous pass. A trigger *dispatches* work and returns immediately; each build finalizes its own `build_runs` row when its Job completes (completion-order). There is **no shared in-memory queue** — the real queue is the cluster (Kaniko Jobs + the namespace `ResourceQuota`), and the shared in-flight/history record is `build_runs` in Postgres.

A trigger runs the **dispatch pass** under a short in-process `tokio::sync::Mutex` held only for the *dispatch critical section*:
1. reconcile still-`Running` `build_runs` (adopt orphans as concurrent tasks — see *Crash / restart reconciliation*);
2. render + hash each pinned `os/*.yaml`, diff against `os_builds`;
3. for each changed OS **not already in-flight**, `ensure()` (create Kaniko Job) + `start_run` + **spawn** a task that `wait`s and finalizes.

The lock covers only detect→`ensure`→`start_run` (fast); it is **released before** any build is awaited, so builds run concurrently and a second trigger blocks only for the brief dispatch window, not for the builds. Dedup makes overlap safe: deterministic Job name + get-status-branch in `ensure`, plus an "is this OS already `Running`?" check (single-replica in-process set, or a partial-unique index on `os_id WHERE status='running'`), so a concurrent trigger dispatches only genuinely-new work and skips what's building.

Invoked:
- **At startup (prod):** connect Postgres, migrate, then run one dispatch pass over the baked/pinned recipes and idle (serving the endpoint). Builds finalize in the background. No polling ticker.
- **Via the endpoint (both envs):** `POST /internal/recipes/rebuild` (`axum`) runs the same dispatch pass and **returns immediately** — `{ dispatched: [...], already_in_flight: [...] }`, *not* the build outcome. Callers read progress/result from `build_runs` (and the live Job / pod-log stream). `force=true` bypasses the hash (covers the `lib/vend` gap and "rebuild now"); optional `os` filter. No auth — in-cluster only. This is exactly the contract `api` needs later: an instructor upload triggers a build ad-hoc and returns without blocking the HTTP handler.

### Concurrency & limits

Builds run **concurrently, not sequentially**, each finalizing in completion order. Concurrency is bounded by the **cluster**, not the app: a `ResourceQuota` + `LimitRange` on `pedagog-builds` cap how many Kaniko pods run at once, and k8s queues the rest. No `MAX_CONCURRENT_BUILDS` semaphore in `jobs`.

- **Why namespace-enforced, not app-enforced:** it's multi-tenant-correct — when `pedagog-builds` becomes per-course namespaces, each course gets its own quota (fair isolation) for free; an app semaphore couldn't express that.
- **Accepted wrinkle:** an over-quota Job has its pod rejected at admission, and the Job controller retries creation with a backoff (up to ~minutes) — so a queued build can start slowly after a slot frees. Fine at OS-build frequency; noted for assignment-build bursts.
- **Future — Kueue.** For assignment-build bursts, [Kueue](https://kueue.sigs.k8s.io/) gives namespace/quota-aware queueing without the backoff (it *suspends* Jobs instead of failing pod creation). Deferred: it's a pre-1.0 operator to own, and migrating later is cheap — add the `kueue.x-k8s.io/queue-name` label to each Job and swap `ResourceQuota`→`ClusterQueue`+`LocalQueue`, no app rewrite. (arm64 images ✓ since Kueue v0.18; needs k8s ≥1.29, cluster is 1.31.)

### Kaniko job construction (`k8s::build`)

- **Built as a typed struct, not YAML:** constructs a `k8s_openapi::api::batch::v1::Job` (Pod/Container/Volume from `k8s-openapi`) and submits via `kube::Api<Job>`. These Jobs are created dynamically per build at runtime, so it's compile-time-checked construction rather than templated manifests. Pin the `k8s-openapi` version feature to **`v1_31`** (matches the cluster's k3s `v1.31.4`; see `docs/SETUP.md` Pinned Versions).
- **Input:** rendered Containerfile string; target ref (`registry-service.pedagog-data.svc:5000/pedagog/<os-id>:<tag>`); context (prod: `emptyDir` filled by the jobs-image initContainer / dev: `hostPath` mounted straight in) + Containerfile ConfigMap.
- **Config (env, per overlay):** `PEDAGOG_JOBS_IMAGE` (prod initContainer image ref — the jobs image), `PEDAGOG_RECIPES_HOSTPATH` (dev node path → the Kaniko `hostPath` volume), `PEDAGOG_RECIPES_DIR` (where the jobs pod itself reads recipes to render; default `/opt/pedagog/recipes`, dev points it at the mounted checkout).
- Image `gcr.io/kaniko-project/executor`; `--insecure`/`--insecure-pull` (registry has no TLS).
- Build pods carry explicit CPU/memory **requests+limits** so the namespace `LimitRange`/`ResourceQuota` can bound concurrency (see *Concurrency & limits*).
- Finished Jobs (success **and** failure) get **`ttlSecondsAfterFinished: 3600`** (1 h) for auto GC — logs are already durable in `build_runs.error`, so the TTL is only a live-`kubectl` window; tunable per overlay.

### Job naming & deduplication (get-status-branch)

Deterministic Job name `os-build-<os-id>-<hash-prefix>` from the rendered-Containerfile hash. Before creating, **get the Job by name and branch on status**:

| Existing Job state | Action |
| --- | --- |
| none | `create` |
| Active (Pending/Running) | skip — in flight |
| Succeeded | skip — already built |
| **Failed** | `delete` + `create` (retry); failure already in `build_runs` (capture-before-delete only as a crash fallback) |

- `create` treats `AlreadyExists` as a harmless race backstop (single replica + mutex already prevent overlap; covers a crash-restart edge).
- The DB hash is written **only on success**, so a failed build re-triggers next startup/endpoint call; the get-status-branch above makes that retry actually fire instead of being blocked by a lingering failed Job.

### Failed-build logs

A run that ends **failed** has its pod logs captured to `build_runs.error` (durable, queryable) and `tracing` (Loki/`stern`) at finish. The failed Job then lingers (kubectl-inspectable) until the next `ensure` supersedes it or its TTL reaps it — the diagnosis is already durable, so deletion loses nothing. Fallback: if `jobs` crashed before capturing, the next `ensure` captures the lingering Job's logs before deleting. Aligns with ARCHITECTURE.md Observability.

### Dev vs. prod (`PEDAGOG_ENV`)

`core::env` — the var **name as a constant** plus a plain enum:
```rust
pub const PEDAGOG_ENV: &str = "PEDAGOG_ENV";
pub enum Env { Dev, Prod }
impl Env { pub fn current() -> Env { /* match std::env::var(PEDAGOG_ENV) */ } }
```
No `is_dev()`/`is_prod()` booleans — callers `match`. Other env-var names (`PEDAGOG_JOBS_IMAGE`, `PEDAGOG_RECIPES_HOSTPATH`, `PEDAGOG_RECIPES_DIR`, `DATABASE_URL`) are likewise constants — shared ones in `core`, jobs-specific ones in `jobs`.

- **prod:** startup runs the build routine against baked/pinned recipes; endpoint available for ops.
- **dev:** startup does **not** auto-build (recipes are a live hostPath checkout you're editing); the endpoint is the trigger. Both build in-cluster via Kaniko.

### Local dev environment (dev Postgres)

New devs get a database with one command — Justfile recipes over `podman`, mirroring the prod image so `DATABASE_URL` shape matches:

- `just db-up` — `podman run postgres:16-alpine` (db/user `pedagog`, port 5432, named volume).
- `just db-down` / `just db-shell` — teardown / `psql`.
- `just db-prepare` — `sqlx migrate run` + `cargo sqlx prepare --workspace`, to refresh the committed `.sqlx/` offline cache.
- `DATABASE_URL = postgres://pedagog:pedagog@localhost:5432/pedagog` (dev-only static password, documented in `docs/SETUP.md`).

Chosen over a `compose.yaml`: no new tooling (podman + just already required), matches the prod image, and compose can't express the migrate/prepare *tasks* we also need (we'd keep the Justfile regardless).

### Deploy changes

- **No `recipes-pvc`** (recipes are baked; Kaniko context is emptyDir/hostPath). Removes RWX Longhorn.
- `jobs` Deployment + ServiceAccount + Role/RoleBinding: `create`/`get`/`watch`/`delete` on `batch/v1` Jobs, `get` on `pods/log`, in `pedagog-builds`; plus creating per-build ConfigMaps.
- Postgres access for `jobs` (Postgres already deployed under `pedagog-data`); DB migration run at `jobs` startup.
- NetworkPolicy: allow Kaniko pods → `registry-service` in `pedagog-data`.
- **`ResourceQuota` + `LimitRange` on `pedagog-builds`** — the LimitRange sets default CPU/mem requests for build pods; the ResourceQuota caps namespace totals, so k8s admits only as many Kaniko builds as fit and queues the rest. This is where build concurrency is governed (not an app-level semaphore).
- Env per overlay: `PEDAGOG_ENV`, `PEDAGOG_JOBS_IMAGE` (prod initContainer ref), `PEDAGOG_RECIPES_DIR`; the dev overlay adds `PEDAGOG_RECIPES_HOSTPATH` and mounts the hostPath recipes on the jobs pod.
- `jobs` image: multi-stage, bakes recipes + `hammer vend` ingredients.
- Pin the `k8s-openapi` version feature to **`v1_31`** (cluster runs k3s `v1.31.4`); documented in `docs/SETUP.md` Pinned Versions.

### Namespace

Kaniko build Jobs run in a temporary shared `pedagog-builds` namespace, replaced by per-course namespaces once course provisioning lands. Its `ResourceQuota`/`LimitRange` govern how many builds run at once; per-course namespaces will each carry their own quota → fair multi-tenant isolation for free.

### ARCHITECTURE.md corrections (living doc)

Corrected as part of this milestone:
- *"`hammer` … submits a Kubernetes Job"* / *"Built by `hammer build-os`"* → in-cluster Kaniko submission is `jobs` (OS) / `api` (instructor, later) via `k8s`; there is no `hammer build-os`. OS images are built at `jobs` startup / via its endpoint. `core` owns resolve/render.
- Crate table: `hammer` no longer credited with Kaniko submission; add the `store` crate (DB access, feature-gated) and note `k8s` `build`/`run` submodules. Clarify `core` holds domain types (not the DB driver).
- Note recipes are **pinned + baked**, not git-synced in-cluster.

---

## Approved dependencies

`kube`, `k8s-openapi`, `tokio`, `axum`, `tracing`, `tracing-subscriber`, **`sqlx`** (Postgres, `migrate`), **`blake3`** (Containerfile hashing — single crate). All approved. Concurrent dispatch uses `tokio::task::JoinSet`/`spawn` (already have `tokio`) — **no `futures` crate needed**.

**`sqlx` offline cache.** `sqlx::query!` type-checks SQL against a live DB at compile time. To build without a DB (CI, other machines), commit an offline cache: run `cargo sqlx prepare` once against a dev DB **with migrations applied** → it writes `.sqlx/` (per-query metadata) → commit it. CI builds with `SQLX_OFFLINE=true` and runs `cargo sqlx prepare --check` to fail when a query changed without regenerating the cache. Only regenerating the cache needs a live DB — the local dev Postgres (`just db-up`). Schema iterates fast this milestone, so the loop is: add a migration → `just db-prepare` (applies it to the dev DB + `cargo sqlx prepare`) → commit the migration + refreshed `.sqlx/`.

---

## Alternatives Considered

- **Shell out to `hammer`** (07-14) vs. **link `core`** — chose link `core` (refactor made `core` the library SoT; subprocess = version skew + stdout parsing + bundled binary for no benefit).
- **Continuous pull loop tracking `recipes` main** (07-14) vs. **pin + bake + startup check** — chose pin. Eliminates in-cluster git, the sync interval, and pin-drift; gives reproducible prod builds (no silent rebuild on an upstream merge). Cost: recipe changes take effect on redeploy, which is the intended discipline.
- **Hash state on a PVC file** (07-14) vs. **Postgres (`store::db`)** — chose Postgres. It's where build state belongs long-term (image versions, GC). POC slice now.
- **DB models/driver in `core`** vs. **feature-gated in a `store` crate** — chose `store` with a `db` feature, keeping `core` WASM-clean (same reason `kube` stays out of `core`).
- **`recipes-pvc` populated at startup** vs. **initContainer → emptyDir per build** (prod) — chose emptyDir (no RWX Longhorn; self-contained builds).
- **One unified pod shape** (always an initContainer) vs. **two shapes** (prod initContainer / dev hostPath straight into Kaniko) — chose two shapes. The initContainer exists only to extract *baked* recipes; dev has none to extract, so mounting the checkout directly drops the per-build copy and the jobs-image self-reference in dev. Cost: a small `match` in `k8s::build`.
- **`k8s::build` takes `ImageSpec`** vs. **a Containerfile string** — chose the string (keeps `k8s` recipe-free; `api` reuses it).
- **`BuildStatus` storage: TEXT+`strum` alone / native PG `ENUM` / TEXT+`strum`+`CHECK`** — chose TEXT+`strum`+`CHECK`. `strum` only guards the Rust write path (bad values still land via raw SQL, caught only on read); a native `ENUM` enforces at the DB but `ALTER TYPE … ADD VALUE` fights heavy schema iteration. The `CHECK` gives DB enforcement at one line per migration, keeping strum's ergonomics.
- **`AlreadyExists`-only dedup** (07-14) vs. **get-status-branch** — chose get-status-branch (the former silently TTL-gated retries).
- **`podman build` in dev** vs. **in-cluster Kaniko in dev** — chose in-cluster (exercise the real path end-to-end).
- **One-shot CLI rebuild** vs. **`axum` endpoint** — chose the endpoint.
- **Fetch ingredients during the Kaniko build** vs. **`hammer vend` at image-build time** — chose vend-at-build (hermetic/offline builds).
- **Sequential builds** vs. **concurrent completion-order executor** — chose concurrent; each build finalizes its own `build_runs` row on its own completion. Generalizes to assignment builds.
- **App-level `MAX_CONCURRENT_BUILDS` semaphore** vs. **namespace `ResourceQuota`/`LimitRange`** — chose namespace-enforced (multi-tenant-fair; aligns with per-course namespaces; app stays limit-free). Cost: the quota-gated Job-controller backoff.
- **Kueue now** vs. **`ResourceQuota` now, Kueue later** — chose defer. Migrating later is a Job label + `ClusterQueue`/`LocalQueue` swap (no app change); avoids owning a pre-1.0 operator this milestone.
- **Synchronous pass** (mutex around the whole pass; endpoint awaits the builds) vs. **dispatch-and-return background executor** (short dispatch-lock; endpoint returns `{dispatched, already_in_flight}`) — chose dispatch-and-return. `api` will trigger assignment builds ad-hoc and can't block its HTTP handler; the queue is the cluster + `build_runs`, not an in-memory `JoinSet`.
- **Dev Postgres via `compose.yaml`** vs. **Justfile `db-*` recipes over `podman`** — chose Justfile: no new tooling, matches the prod image, and compose can't model the migrate/prepare tasks we also need.

---

## Open Questions

1. **True prod/dev symmetry later (`image:` volume source).** k8s 1.31+ can mount an OCI image as a read-only volume; if k3s on the nodes supports it, prod could mount a recipes image at `/context` with **no initContainer**, matching dev (both just a volume). Too new to depend on now; noted as a future simplification.
2. **`jobs` image size.** Baking `hammer vend` ingredients (e.g. `code-server.deb`, ~100 MB+) inflates the `jobs` image, which is also the Kaniko initContainer image. Acceptable (already pulled for the Deployment; node-cached), but noted.
3. **Vend-at-build needs network + pinned GH tags** during the `jobs` image build. Fine in CI; flagged for offline/air-gapped build environments.
4. **Postgres coupling.** OS builds now depend on Postgres being up at `jobs` startup. Everything needs Postgres eventually, so accepted; blast radius slightly larger than a self-contained file.
5. **Full build-input hashing** (Containerfile + `lib/vend` + ingredients) — deferred; force rebuild is the interim mitigation.
6. **Build-pod resource sizing + quota values** — the `LimitRange` default requests and the `pedagog-builds` `ResourceQuota` totals need tuning to RK1 capacity (how many concurrent Kaniko builds a node tolerates). Start conservative.
7. **Quota-gated start latency** — accepted for OS builds; Kueue is the planned upgrade if assignment-build bursts make the Job-controller backoff painful (see *Concurrency & limits*).

*(Resolved during review: `ttlSecondsAfterFinished = 3600`; active-orphan reconcile = re-adopt as a concurrent task; concurrency governed by the namespace, not an app semaphore; dispatch-and-return control flow.)*

---

## Rollback Plan

- Additive: new `k8s` build code, new `store` crate + one migration, new `jobs` logic, new deploy manifests. No `core` schema/recipe changes (beyond ARCHITECTURE.md edits).
- `kubectl delete -k deploy/...` removes the `jobs` Deployment and RBAC. The one migration is additive (a new table); a `down` migration drops it.
- Reverting the crate additions is a plain git revert; no runtime side effects outside the listed cluster resources.

---

## Step-by-Step Implementation

1. **Workspace `Cargo.toml`** — add `kube`, `k8s-openapi`, `tokio`, `axum`, `tracing`, `tracing-subscriber`, `sqlx`.
2. **`core::env`** — `Env { Dev, Prod }` + `Env::current()` parsing `PEDAGOG_ENV`. No booleans.
3. **`crates/store`** — plain models (incl. `BuildStatus`) in `lib.rs`; `db/mod.rs` (behind `feature = "db"`) with `connect()` (`MAX_CONNECTIONS`) + `run_migrations()`; a `db/builds.rs` submodule with the `os_builds`/`build_runs` queries (`last_hash`, `record_build`, `start_run`, `finish_run`, `running`); migrations for both tables (`build_runs.status` `CHECK`; partial-unique index on `os_id WHERE status='running'` for in-flight dedup); commit `.sqlx/`. Add Justfile `db-up`/`db-down`/`db-shell`/`db-prepare` (podman `postgres:16-alpine`; `DATABASE_URL=postgres://pedagog:pedagog@localhost:5432/pedagog`) so a new dev gets a DB in one command and can regenerate the offline cache.
4. **`crates/k8s` `lib.rs`** — `KubeClient` wrapping `kube::Client`; `create_job`, `watch_job`, `delete_job`, `get_pod_logs`.
5. **`crates/k8s` `build.rs`** — a `Builder` grouping *all* build logic: the Kaniko Job spec (initContainer/emptyDir prod vs hostPath dev), per-build ConfigMap, `ensure()` (get-status-branch dedup), `wait()` (→ succeeded / failed+logs), and fallback failed-log capture.
6. **`crates/jobs`** — the **dispatch pass** under a short mutex: connect Postgres + migrate, **reconcile still-`Running` `build_runs`** (adopt orphans as spawned tasks), load `RecipeStore` (baked/pinned or hostPath), `resolve_base` + `render` (blake3-hash) per `os/*.yaml`, diff against `os_builds`; for each changed OS **not already in-flight**, `ensure()` + `start_run` + **spawn** a task (`tokio::task::JoinSet`/`spawn`) that `wait`s and `finish_run`s (upserting `os_builds` on success). Release the lock before awaiting; return the dispatched set. Run at startup (prod) and from the endpoint.
7. **`crates/jobs`** — `axum` `POST /internal/recipes/rebuild` (`force`, `os`), **dispatch-and-return** `{ dispatched, already_in_flight }`; available in both envs.
8. **`jobs` container image** — multi-stage: build binaries, `hammer vend` the pinned recipes, `COPY` recipes+ingredients into the final image. No `hammer` at runtime.
9. **Deploy manifests** — `pedagog-builds` namespace **+ `ResourceQuota` + `LimitRange`**, `jobs` Deployment + SA/RBAC (Jobs, `pods/log`, ConfigMaps), Postgres env/secret, registry NetworkPolicy, `PEDAGOG_ENV` per overlay, dev hostPath patch, build-pod resource requests. Pin `k8s-openapi = v1_31`. No `recipes-pvc`.
10. **Docs** — `ARCHITECTURE.md` corrections (hammer/Kaniko, `store` crate, pinned+baked recipes); `docs/SETUP.md` — add the `k8s-openapi` feature (`v1_31`) to Pinned Versions and document `just db-up` for new devs.
11. **End-to-end test** — dev: hit `/internal/recipes/rebuild`, confirm Kaniko builds in-cluster and the image lands in the registry, and the hash is recorded in Postgres; prod: fresh startup builds all, second startup builds nothing (hashes match), `force` rebuilds.
12. **Tests** (inline `#[cfg(test)] mod tests`):
    - `store::db`: `os_builds` hash upsert/read + `build_runs` start→finish (succeeded/failed) round-trips (behind `db`; against a test Postgres or `sqlx` test harness).
    - `k8s::build`: given a Containerfile string + target, assert the generated Job manifest incl. deterministic name and the prod initContainer/emptyDir wiring; `AlreadyExists` no-op; failed→delete+recreate.
    - `jobs` change detection: fixture `os/*.yaml` + recorded hashes → which rebuild; empty-DB bootstrap (all); `force` (all regardless).
    - `jobs` reconcile: a `Running` `build_runs` row finalized from its Job's state (succeeded / failed / gone).
    - `jobs` dispatch dedup: a second trigger while a build is in-flight dispatches only new work (already-`Running` OS skipped).
    - `core::env`: `Env::current()` parsing.

---

## Appendix — Illustrative implementation

Design-level sketches, not final code (`unwrap`/`panic`/`expect` omitted per the workspace lint policy).

**`core::env`**
```rust
pub const PEDAGOG_ENV: &str = "PEDAGOG_ENV";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Env { Dev, Prod }

impl Env {
    pub fn current() -> Env {
        match std::env::var(PEDAGOG_ENV).as_deref() {
            Ok("dev")  => Env::Dev,
            Ok("prod") => Env::Prod,
            other => { tracing::warn!(?other, "PEDAGOG_ENV unset/unknown; defaulting to prod"); Env::Prod }
        }
    }
}
```

**`store`** — `BuildStatus` (plain, WASM-safe, `strum` ↔ `TEXT`) in `lib.rs`; `db` split into generic infra + a per-domain `builds` submodule
```rust
// store/src/lib.rs — plain models, no sqlx
#[derive(Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum BuildStatus { Running, Succeeded, Failed }

// store/src/db/mod.rs — generic infra (behind feature = "db")
const MAX_CONNECTIONS: u32 = 5;
pub async fn connect(url: &str) -> sqlx::Result<PgPool>;                                // max_connections(MAX_CONNECTIONS)
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError>;  // sqlx::migrate!("./migrations")

// store/src/db/builds.rs — OS-build domain queries
pub async fn last_hash(pool: &PgPool, os_id: &str) -> sqlx::Result<Option<String>>;
pub async fn record_build(pool: &PgPool, os_id: &str, hash: &str, image_ref: &str) -> sqlx::Result<()>; // upsert os_builds
pub async fn start_run(pool: &PgPool, os_id: &str, hash: &str, image_ref: &str) -> sqlx::Result<i64>;    // status = Running
pub async fn finish_run(pool: &PgPool, run: i64, status: BuildStatus, error: Option<&str>) -> sqlx::Result<()>;
pub async fn running(pool: &PgPool) -> sqlx::Result<Vec<BuildRun>>;                                      // still-Running (reconcile)
```
```sql
-- migrations/0001_os_builds.sql
CREATE TABLE os_builds (
    os_id TEXT PRIMARY KEY, containerfile_hash TEXT NOT NULL,
    image_ref TEXT NOT NULL, built_at TIMESTAMPTZ NOT NULL DEFAULT now());
-- migrations/0002_build_runs.sql
CREATE TABLE build_runs (
    id BIGSERIAL PRIMARY KEY, os_id TEXT NOT NULL, hash TEXT NOT NULL, image_ref TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running','succeeded','failed')),  -- BuildStatus via strum; CHECK = DB backstop
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(), finished_at TIMESTAMPTZ, error TEXT);
```

**`k8s::build`** — the two-shape context is one match; all build logic grouped in `Builder`
```rust
fn context(env: &BuildEnv) -> (Vec<Container> /*init*/, Volume /*"context"*/) {
    match (&env.kind, &env.recipes_hostpath) {
        (Env::Dev, Some(hp)) => (vec![], hostpath_vol("context", hp)),                   // dev: checkout IS the context
        _                    => (vec![stage_recipes(&env.jobs_image, &env.recipes_dir)], // prod: initContainer copies baked
                                 empty_dir("context")),
    }
}
// Builder::ensure(os_id, hash, dest, cf) -> Outcome { Created | Retried | Skipped }
//   put ConfigMap(cf); get Job by name; none->create | active|succeeded->skip
//   | failed->(fallback capture) delete + create.  create() maps 409 AlreadyExists -> Ok.
// Builder::wait(os_id, hash)  -> Waited { Succeeded | Failed { logs } }
// Builder::poll(os_id, hash)  -> JobState { Active | Succeeded | Failed { logs } | Gone }  (reconcile)
```

**`jobs` dispatch pass** — short lock for dispatch only; builds run as spawned tasks (completion-order)
```rust
// Returns immediately with the dispatched set; each build finalizes its own row when its Job ends.
async fn dispatch(ctx: &Ctx, force: bool, only: Option<&str>) -> Dispatched {
    let _guard = ctx.dispatch_lock.lock().await;      // held only for the loop below, NOT the builds

    // 1. reconcile orphaned runs — adopt each as a concurrent task (see Crash / restart reconciliation)
    for run in db::builds::running(ctx.pool).await? {
        spawn_finalize(ctx, run.id, run.os_id, run.hash, run.image_ref);   // poll -> (wait) -> finish_run [+ record_build]
    }
    let in_flight: HashSet<String> = db::builds::running(ctx.pool).await?.into_iter().map(|r| r.os_id).collect();

    // 2. dispatch changed, not-already-in-flight recipes
    let mut dispatched = vec![];
    for os_id in ctx.store.list_oses() {
        let id = os_id.to_string();
        if only.is_some_and(|o| o != id) || in_flight.contains(&id) { continue }   // dedup: skip what's building
        let spec = resolve_base(os_id, &ctx.store)?;
        let ImageSpec::Base { image, .. } = &spec else { continue };
        // registry: None → canonical Containerfile (public FROM); registry is only the push target
        let cf   = Containerfile::render(&spec, &RenderOptions { registry: None, from: FromSource::Standalone }).to_string();
        let hash = blake3::hash(cf.as_bytes()).to_hex().to_string();
        if !force && db::builds::last_hash(ctx.pool, &id).await? == Some(hash.clone()) { continue }

        let dest = format!("{registry}/{image}");
        match ctx.k8s.ensure(&id, &hash, &dest, &cf).await? {         // get-status-branch; 409 AlreadyExists -> Ok
            Outcome::Skipped => {}                                    // already building/built in-cluster
            Outcome::Created | Outcome::Retried => {
                let run = db::builds::start_run(ctx.pool, &id, &hash, &dest).await?;   // build_runs: Running
                spawn_build(ctx, run, id.clone(), hash, dest);                         // wait -> finish_run [+ record_build]
                dispatched.push(id);
            }
        }
    }
    Dispatched { dispatched, already_in_flight: in_flight.into_iter().collect() }
    // _guard drops here — the spawned tasks run on past the return, finalizing rows as each Job completes.
}

// one build task — the only place a row flips out of `Running`; completion-order falls out naturally
fn spawn_build(ctx: &Ctx, run: i64, os_id: String, hash: String, dest: String) {
    let (pool, k8s) = (ctx.pool.clone(), ctx.k8s.clone());
    tokio::spawn(async move {
        match k8s.wait(&os_id, &hash).await {
            Ok(Waited::Succeeded)       => { db::builds::finish_run(&pool, run, Succeeded, None).await.ok();
                                             db::builds::record_build(&pool, &os_id, &hash, &dest).await.ok(); }
            Ok(Waited::Failed { logs }) =>   { db::builds::finish_run(&pool, run, Failed, Some(&logs)).await.ok(); }
            Err(e) => tracing::error!(?e, %os_id, "build wait failed; row stays Running for next reconcile"),
        }
    });
}
```

**`jobs` image** — multi-stage: `cargo build -p pedagog-jobs -p pedagog-hammer` → `hammer vend` (fills `recipes/ingredients/`) → `COPY /src/recipes /opt/pedagog/recipes`; `ENV PEDAGOG_RECIPES_DIR=/opt/pedagog/recipes`. `hammer` is build-time only.
