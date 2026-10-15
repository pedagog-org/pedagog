# Assignment Flow in `jobs` (Step 4 Detail)

Detail plan for step 4 of `2026-10-14-assignment-builds.md`: from `hammer upload` to a built,
validated assignment image. Decisions here come from a design review on 2026-10-15; where they
differ from the parent plan, this plan wins.

## Rationale

- Steps 1–3 delivered Garage, the schema and renderers, the typed results model, and the
  pytest helper (v1.0.0). Nothing yet turns an uploaded assignment into an image.
- The existing OS-build path can't be reused as is: `build_runs`, the Kaniko builder and Job
  names are keyed by OS id, the Kaniko context is a ConfigMap only, and nothing waits with a
  deadline.
- Findings that shape this plan:
  - `jobs` was never wired to Garage (no S3 config), although the key Secret exists in
    `pedagog-builds`.
  - Kaniko pods stage ingredients under `/pedagog/recipes/ingredients/`, but recipes' `vend`
    reads `/pedagog/ingredients/`, so every in-cluster build silently downloads them instead.
  - Ingredients carry no checksum.
  - `environment.network` is described in the parent plan but doesn't exist in the schema.
  - `hammer` has no HTTP client, and the workspace has no tar/gzip crates.

## Design

### Data (`store`)

| Table | Columns (beyond ids and timestamps) |
| --- | --- |
| `assignments` | `id` (from `assignment.yml`), `name`, `current_build_id` |
| `assignment_uploads` | `assignment_id`, `tree_hash`, `object_key`, `size_bytes` |
| `assignment_builds` | `upload_id`, `image_hash`, `image_ref`, `build_status`, `validation_status`, `results` (JSONB), `validation_problems` (TEXT[]), `error`, `started_at`, `finished_at` |

- `build_status`: `running | succeeded | failed | superseded`.
- `validation_status`: `pending | running | passed | failed | skipped | invalid`.
  - `failed`: a consistent results file in which the reference solution disagrees with the
    tests (`solution_mismatches()`). Results are stored; mismatches go in
    `validation_problems` for the instructor.
  - `invalid`: no `results.yaml`, or it doesn't parse, or it fails `problems()`. Results are
    **not** stored (untrusted); the reasons go in `validation_problems`; the raw file stays
    in Garage for debugging.
  - `skipped`: the archive has no solution.
- At most one running build per assignment (partial unique index).
- `assignments.current_build_id` moves to a build only when it succeeds with validation
  `passed` or `skipped`; a failed build never replaces a working one.
- **Reversible migrations** (sqlx `.up.sql`/`.down.sql`) for the new tables, and the existing
  `0001`/`0002` converted to pairs, after confirming sqlx accepts the rename on a database
  that already applied them (fallback: pair only the new ones).

### Upload

- `hammer upload <dir> [--jobs-url URL] [--force] [--wait]` (URL from `PEDAGOG_JOBS_URL`,
  default `http://localhost:18081`):
  - checks the directory locally (parses, resolves against local recipes, files present);
  - packs it as `.tgz` and POSTs it to `jobs` `POST /internal/assignments`;
  - with `--wait`, polls the status endpoint and prints the outcome and any problems.
- `just kind-upload <dir>` port-forwards `jobs` (as `kind-rebuild` does) and runs it.
- No auth: the endpoint stays internal until api exists, when uploads move behind api.
- **`jobs` on receipt** opens the archive in memory and, in one pass:
  - rejects it (400/413 with a clear message) when: there's no `assignment.yml`; an entry is
    absolute, contains `..`, or is a symlink/hardlink/device; it exceeds 50 MiB compressed,
    200 MiB unpacked or 10 000 files (config: `PEDAGOG_MAX_UPLOAD_BYTES`,
    `PEDAGOG_MAX_UNPACKED_BYTES`, `PEDAGOG_MAX_UPLOAD_FILES`);
  - computes `tree_hash = blake3(sorted (path, exec bit, blake3(content)))`, so identical
    content dedupes no matter who packed it;
  - stores it as `assignments/<id>/<tree_hash>.tgz` and records the upload.
- **Image hash** = `blake3(rendered Containerfile || tree_hash)`; tag
  `pedagog/assignments/<id>:<image_hash[..16]>`. `jobs` resolves and renders with its own
  recipes; it is authoritative over `hammer`'s local check.
- **Dispatch rules:**
  - same image hash already built successfully → `already_built`, nothing dispatched;
  - same hash whose last build failed → rebuild; `--force` → always rebuild;
  - a running build for the same assignment → it's **superseded** (its Jobs deleted) and the
    new one starts.
- **Response:** `{ assignment_id, upload, build_id, dispatched | already_built }`.
- **Status:** `GET /internal/assignments/<id>/builds/<build_id>` → statuses, error, results
  summary, `validation_problems`.
- These rules (what counts as identical, what's rejected) are documented in ARCHITECTURE.md's
  assignment pipeline section and in `hammer upload --help`.

### Build

- The k8s builder is generalized with a job kind (name prefix, labels, context source), so OS
  and assignment builds share `ensure`/`wait`/`poll`/log capture. Job names:
  `os-build-<os>-<hash12>`, `assignment-build-<id>-<hash12>`,
  `assignment-validate-<id>-<hash12>` (id shortened to fit 63 characters).
- **Context:** an `emptyDir` filled by an initContainer, the jobs image running
  `pedagog-jobs fetch-context <presigned GET> /workspace`, which downloads and unpacks the
  archive with the same safety checks as intake. Kaniko runs with
  `--context=dir:///workspace`; the Containerfile still comes from the per-build ConfigMap.
- **Base:** `FROM` the prebuilt OS image in the in-cluster registry.
- **Ingredients:** mounted at `/pedagog/ingredients` (see PR 0), so builds use vended assets.
- **First task:** confirm on kind that Kaniko applies `COPY --chown=grader:grader` and the
  modes in the ASSIGNMENT FILES phase (an open question in the parent plan).

### Validation

- A Job from the built image in `pedagog-builds` runs
  `/pedagog/bin/run-tests --code /pedagog/solution --isolation <testing.isolation>`, with
  `PEDAGOG_RESULTS_URL` set to a presigned PUT for `results/<build_id>/results.yaml` (TTL:
  deadline plus margin). No solution → no Job; validation `skipped`.
- **Pod security:**

  ```yaml
  securityContext:
    runAsUser: 0
    allowPrivilegeEscalation: false
    capabilities: { drop: [ALL], add: [SETUID, SETGID, CHOWN, FOWNER, DAC_OVERRIDE] }
    seccompProfile: { type: RuntimeDefault }
  ```

  The launcher needs exactly these to copy grader-only files and drop to `grader`/`runner`.
  This fits Pod Security "baseline", recorded as `pedagog-builds`' ceiling ("restricted"
  would break the launcher).
- **Network:** the existing `environment.network` (`mode: allow | deny`, `allow:` rules of
  `host` or `cidr` with optional `ports`). Revised during PR (b), when it turned out the
  schema already existed:
  - its default changes from `allow` to **`deny`** (no egress beyond DNS and Garage);
  - static NetworkPolicies select validation pods by label
    (`pedagog.io/role=validate`, `pedagog.io/network=deny|allow|rules`): `validate-base`
    (DNS, `garage-service:3900`) for all, `validate-internet` (0.0.0.0/0 minus
    `PEDAGOG_CLUSTER_CIDRS`) for `mode: allow`;
  - `cidr` rules (with their ports) get a per-run NetworkPolicy, owned by the Job (deleted
    with it); the jobs Role gains `networkpolicies` create/delete;
  - `host` rules are accepted but **not enforced** until the egress proxy exists: those
    hosts stay unreachable, and uploads warn about them;
  - Kaniko build pods keep their egress (package installs).
- **Results check:** `jobs` reads `results.yaml` from Garage, parses it with
  `core::results`, and sets `validation_status` and `validation_problems` as above.

### Operations

- **Completion:** poll every 3 s (as OS builds do); every Job gets `activeDeadlineSeconds`,
  and `wait` gives up at the deadline plus 60 s (failed: "timed out").
- **Limits** from config, as fallbacks until per-assignment overrides exist:
  `PEDAGOG_ASSIGNMENT_BUILD_DEADLINE` (1800 s), `PEDAGOG_VALIDATION_DEADLINE` (600 s),
  `PEDAGOG_VALIDATION_CPU` (1), `PEDAGOG_VALIDATION_MEMORY` (1Gi), and Kaniko pod
  requests/limits.
- **Reconcile** (at startup in prod and before each dispatch):
  - running builds and validations: Job active → keep waiting; Job gone → failed
    ("interrupted");
  - uploads stored without a build row (a crash between storing and dispatching) → dispatch.
- **Garage in `jobs`:** S3 config (`PEDAGOG_S3_*`) and the `garage-s3-key` Secret wired into
  the Deployment (missed in step 1).

## Dependencies

All approved in review:

- `tar`, `flate2`: `hammer` packs, `jobs` unpacks (intake and `fetch-context`).
- `reqwest`: `hammer`'s upload and `fetch-context`'s download (already in the tree through
  `object_store`).
- `sha2`: `hammer vend` verifies ingredient checksums. Everything else hashes with `blake3`.

## Alternatives considered

- **One polymorphic runs table** for OS builds, assignment builds and validations: one
  reconcile path, but nullable kind-specific columns and a migration of OS data. Own tables
  give typed columns, foreign keys and simple constraints; a combined view can come later.
- **Kube watchers instead of polling:** more efficient at scale but adds reconnect and
  resync handling; polling a handful of Jobs is cheap. Revisit with sessions.
- **Kaniko's remote tar/S3 context:** no initContainer, but its handling of Garage and
  presigned URLs is unverified.
- **Deterministic packing and hashing the archive bytes:** cheaper, but dedupe only works
  for archives `hammer` packed, and `jobs` must open uploads anyway.
- **Queueing or rejecting concurrent uploads:** slower feedback or annoying; the latest
  upload is the one the instructor wants.
- **Root with default capabilities, or a rootless two-container launcher:** more privilege
  than needed, or a change to the helper's v1.0.0 launcher contract.
- **Cilium FQDN policies for hostnames:** full protocol coverage, but replaces cluster
  networking on the Pis; the egress proxy is chosen for the follow-up plan.
- **Deferring network rules:** validation runs only trusted solutions today, but the same
  pods run student code once grading arrives.

## Open questions

- Does sqlx accept renaming the existing migrations to `.up.sql` on a database that already
  applied them (checked in PR a)?
- Does Kaniko apply `COPY --chown` and modes as expected (checked first in PR c)?
- Cluster CIDRs for `PEDAGOG_CLUSTER_CIDRS` on kind and k3s (looked up in PR d; k3s defaults
  are 10.42.0.0/16 and 10.43.0.0/16).

## Follow-ups (recorded, out of scope)

- Hostname allowlists through an in-cluster egress proxy (own plan, right after step 4).
- One shared Job watcher once run volume grows.
- Per-assignment limits (e.g. `testing.limits`) over the config defaults.
- Retention: Garage and registry cleanup of old uploads and superseded images.
- Scoping assignment ids by course/tenant (with api/auth); today any upload with the same id
  replaces it.
- `hammer vend` path collision: platform ingredients are labeled by kind only, so two OSes
  with the same platform kind vend to the same path.
- Ingredient checksums required (not just optional) once every recipe has one.

## Rollback

- Each PR is additive and revertable on its own: new tables (down migrations), new routes
  and code paths, new manifests (policies), an optional schema field (`environment.network`
  absent now means `deny`), and an optional ingredient field.
- PR 0 changes OS builds' mounts; reverting it restores the network fallback behavior.

## Implementation steps

0. **Ingredients** (pedagog + recipes):
   - mount the staged ingredients at `/pedagog/ingredients` in Kaniko pods, ignored like
     `/pedagog/recipes`;
   - optional `sha256` on ingredients, verified by `hammer vend` (fails on mismatch); set for
     the code-server `.deb` and the pytest bundle (recipes PR);
   - tests; then rebuild an OS on kind and confirm no fallback download.
1. **(a) Migrations and store:**
   - the three tables, statuses, indexes; reversible migrations (with the sqlx rename check);
   - query functions in `store::db` (one per query, `sqlx::query!`, offline cache);
   - tests against Postgres.
2. **(b) Intake:**
   - Garage wiring in `jobs` (config, Deployment env, Secret);
   - archive inspection (limits, path safety, tree hash), storage, upload rows;
   - `POST /internal/assignments` with the dispatch rules (dispatch itself stubbed until c);
   - `environment.network` defaults to `deny`; host-rule warnings;
   - `hammer upload` (local checks, packing, `--jobs-url`, `--force`), `just kind-upload`;
   - documentation of the dedupe and rejection rules; tests.
3. **(c) Assignment build:**
   - verify Kaniko `--chown` on kind;
   - generalize the k8s builder (job kinds, names, labels, deadlines);
   - `pedagog-jobs fetch-context`; Kaniko with the emptyDir context;
   - build dispatch, supersede, current-build rules; tests.
4. **(d) Validation:**
   - the validation Job (security context, presigned PUT, isolation flag);
   - static and per-run NetworkPolicies, RBAC, `PEDAGOG_CLUSTER_CIDRS`;
   - results check (`passed`/`failed`/`invalid`/`skipped`), storage;
   - reconcile (running rows, orphaned uploads);
   - `GET /internal/assignments/<id>/builds/<build_id>` and `hammer upload --wait`; tests.
5. Then the parent plan's step 5 (kind end-to-end) and step 6 (docs).
