# Performance and Cleanups after Grading

Follow-up to `2026-10-15-grading-on-submission.md`: what measuring the finished pipeline
showed, and the changes chosen in review on 2026-10-16. Three PRs: performance, images,
cleanup.

## Rationale

Measured on an Apple Silicon Mac and the kind cluster (the Pis are roughly 6–10× slower
for CPU-bound work; to be confirmed there):

| Measured | Result |
| --- | --- |
| `hammer` plan, describe, vend | ~10 ms each, 9 MB peak memory |
| `jobs` pod at idle | 15 MB memory, ~0.5% of a CPU |
| Archive work on small trees (40 KB) | ≤ 1 ms |
| 50 MiB archive: unpack and hash / pack at gzip 6 | 141 ms / 806 ms |
| One grading run, release to recorded result | ~9 s, ~5 s of it after the tests finished |
| `pedagog/jobs` image | 530 MB: 28 MB binary, 377 MB recipes and vendored downloads |

- **Result latency:** tests ended at :17, Kubernetes marked the Job complete at :19, and
  the watcher (polling the Job every 3 s) recorded it at :21.6. About half of a
  student's wait is detection.
- **Release rate:** admission releases at most one run per 2 s tick (after a release the
  new Job has no pod yet, so the loop sleeps a full tick): a cap of 30 runs/min
  whatever the capacity. Not binding at today's quota (3 slots × 9 s ≈ 20/min); binding
  once the Pis' quota grows.
- **Per-submission archive read:** intake downloads, unpacks and hashes the whole
  assignment archive on every submission to read `assignment.yml` (141 ms per 50 MiB
  here). Admission re-derives a build's plan the same way after a restart.
- **Image weight:** every test-run pod's initContainers (`await-egress`,
  `fetch-context`) use the full 530 MB image, pulled again on each node after every
  release. It also holds the code-server download twice (188 MB each, under
  `platform/workspace` and the pre-rename `platform/interactive`): the image build
  copies its whole download cache, stale folders included.
- **Code review findings:**
  - `plans: Arc<std::sync::Mutex<HashMap<i64, Plan>>>`: the right lock type (held
    briefly, never across `.await`), but unbounded, cloning a whole `Plan` per hit,
    silent on poisoning, and per process (lost on restart, different per replica).
  - `dispatch_lock: Arc<tokio::sync::Mutex<()>>`: guards code rather than data, is shared
    by OS and assignment dispatch, serializes unrelated assignments, and protects
    nothing across replicas (the unique index is the real guard).
  - Six detached `tokio::spawn` sites: dropped handles, so panics vanish and nothing
    stops them on shutdown; one polling task per in-flight run.
  - Isolation passed as a string across crates; `Notify` defined three times.
  - The same `SELECT` column list copied 4× for grading runs and 4× for builds.
  - `submissions.rs` borrows `intake.rs`' HTTP error helpers.

Not worth changing: `hammer`, `jobs`' idle footprint, hashing, small-tree archive work,
gzip level (big submissions only; declined in review).

## Design

### PR 1: performance

- **Run settings in Postgres** (replaces the `plans` cache and the per-submission
  archive read). Migration `0005` adds to `assignment_builds`: `platform` (`workspace`
  | `upload`), `isolation`, `egress` (JSONB), `submission` (JSONB: the parsed rules).
  `start_build` writes them in its transaction. Admission reads image, isolation and
  egress from the build row; intake reads platform and rules from the current build's
  row. Builds from before the migration have empty columns: they're derived once from
  the archive and backfilled.
- **Advisory lock for dispatch:** assignment dispatch runs in one transaction under
  `pg_advisory_xact_lock(hashtext('dispatch:' || id))`; OS dispatch under its own key.
  The store's dispatch queries take the transaction. `dispatch_lock` goes.
- **One tracked watcher:** a single `kube` watch (the `runtime` feature is already
  enabled) on pods in `pedagog-grading` labeled `pedagog.io/test-run`, reacting when the
  main container terminates: validation is judged, grading recorded. It also notices
  pods that vanish. On (re)start it lists first, so runs in flight are picked up
  (reconcile becomes "watch"). Background work (admission, the watcher, build waits) is
  owned by a `tokio::task::JoinSet` in `main`, so panics are logged and shutdown stops
  them. Kaniko and OS build waits keep polling (builds are minutes long; latency there
  doesn't matter).
- **Quick re-check after a release:** after releasing a run, admission checks again
  after 250 ms instead of 2 s, so the release rate follows pod creation.

### PR 2: images

- **Stale downloads:** the image build copies only the ingredient folders the current
  recipes declare (`hammer vend` reports them), not the whole download cache.
  `pedagog/jobs` 530 → ~340 MB.
- **`pedagog/jobs-init`:** a second stage of the same Containerfile with only the
  `pedagog-jobs` binary on a minimal base (`gcr.io/distroless/cc-debian12:nonroot`,
  ~45 MB with the binary; `debian:stable-slim` is the fallback, ~100 MB). Used by
  `await-egress` and `fetch-context` in validation, grading and Kaniko build pods;
  `stage-recipes` keeps the full image.
  - Config `PEDAGOG_JOBS_INIT_IMAGE` (defaults to `PEDAGOG_JOBS_IMAGE`, so a
    deployment without it keeps working).
  - `just kind-load-jobs` builds and loads both; the overlays set both; SETUP covers
    pushing both to the Pis' registry.

### PR 3: cleanup

- **Typed `Isolation` and `Notify`** in `core`, used by `jobs`, `k8s`, `store` (mapping
  to and from `TEXT` as for statuses; `store` gains `core` as an in-repo dependency) and
  `hammer` (clap derives on them).
- **Views for the repeated columns:** `grading_run_rows` and `assignment_build_rows`
  (text casts included); queries become `SELECT * FROM <view> WHERE …`, keeping
  compile-time checks.
- **`jobs/src/http/error.rs`:** `Refused`, `refused`, `internal`, used by `intake` and
  `submissions`.

## Alternatives considered

- **Keep the cache, bound it, store `Arc<Plan>`:** smaller change, but still per process
  and still re-derived after every restart.
- **A cache crate (`moka`) or `DashMap`:** new dependencies for a problem the database
  already solves.
- **Per-assignment in-process locks:** fixes unrelated assignments waiting, but not
  replicas.
- **Faster polling (1 s) instead of a watch:** simpler, but still one task per run and
  N API calls per second at scale; the watch also replaces the detached tasks.
- **Watching for the results file in Garage:** no API to watch; would still be polling.
- **gzip level 1 for submissions:** 6× faster packing, 8% more storage; only matters for
  large submissions (declined).
- **`tokio-util`'s `TaskTracker`:** a new dependency; `JoinSet` covers it.
- **A string-building macro for the SELECTs:** sqlx's checked macros need literal SQL.

## Open questions

- Does `gcr.io/distroless/cc-debian12` run the binary as built (glibc version, no
  shell needed)? Checked first in PR 2; fall back to `debian:stable-slim`.
- How large the Pis' `pedagog-grading` quota should be, which decides when the release
  rate matters; measured on the Pis.

## Rollback

- PR 1: migration `0005` is reversible (the columns are additive; the cache path is
  gone, so reverting the code reverts to deriving plans from archives). The watcher and
  the lock revert with the code.
- PR 2: `PEDAGOG_JOBS_INIT_IMAGE` unset falls back to the full image.
- PR 3: no behavior change.

## Implementation steps

1. **PR 1, performance** (`perf/test-run-latency`):
   - migration `0005`, `start_build` with settings, backfill for older builds; intake
     and admission read the row; drop `plans`;
   - advisory-locked dispatch transaction; drop `dispatch_lock`;
   - the pod watcher and `JoinSet` ownership; validation and grading judged from it;
   - admission's quick re-check;
   - tests; on kind: before/after grading latency and release rate for a burst, and
     kind-e2e.
2. **PR 2, images** (`build/jobs-init-image`): Containerfile stages, vend-only copy,
   config, justfile, overlays, SETUP; on kind: image sizes and a first run on a fresh
   node.
3. **PR 3, cleanup** (`refactor/typed-settings`): types, views, error module; tests and
   kind-e2e unchanged.
