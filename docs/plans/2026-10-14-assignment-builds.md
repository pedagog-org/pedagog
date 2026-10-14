# Assignment Image Builds and Validation

**Date:** 2026-10-14
**Status:** Planned
**Builds on:** [2026-08-06-jobs-kaniko-os-builds](2026-08-06-jobs-kaniko-os-builds.md),
[2026-10-14-kaniko-revival](2026-10-14-kaniko-revival.md), [design-doc-v2](2026-10-13-design-doc-v2.md)

## Rationale

OS base images now build in-cluster (`jobs` + Kaniko). The next stage in ARCHITECTURE.md's
pipeline is the **instructor (assignment) image**: built from the instructor's archive and
validated by running the reference solution against the tests. This plan delivers a thin
vertical slice:

```
archive in → stored → Kaniko build → validation run → results checked → status stored
```

There's no UI or auth yet (an internal `jobs` endpoint plus `hammer upload`), and only one
test framework (pytest), but with its intended security model (process isolation) from the
start.

## Scope

**In:**

- Garage object store.
- Archive intake.
- The `testing:` schema block.
- The ASSIGNMENT FILES render phase.
- `grader` and `runner` users.
- In-cluster assignment builds and validation runs in `jobs`.
- The `python-pytest` helper with process isolation.
- kind end-to-end.
- Docs.

**Out (recorded below):**

- Student-triggered hidden runs (need sessions and the daemon).
- Grading on submission.
- Catch2 and cargo helpers.
- api/UI/auth.
- Seeding workspaces from `/pedagog/starter` (sessions).

## Archive and storage

An archive is a `.tgz` containing:

```
assignment.yml
starter/     # given to students
solution/    # reference implementation (baked, grader-only)
tests/       # test suite (baked, grader-only)
```

- `hammer upload <dir>` validates the directory locally (`assignment.yml` parses and
  resolves), packs it, and POSTs it to `jobs` `POST /internal/assignments` (body: the tgz).
- `jobs` stores it in Garage as `assignments/<assignment-id>/<sha256>.tgz` (every upload is
  kept), records an `assignment_uploads` row, and dispatches the build. Returns
  `{ assignment_id, upload, dispatched | already_built }`, dispatch-and-return like OS
  rebuilds.

### Garage

[Garage](https://garagehq.deuxfleurs.fr/) (maintained Rust S3 server, arm64 images, AGPL)
replaces MinIO, whose community edition is archived and source-only since February 2026.

- Single-node StatefulSet in `pedagog-data` with a `pedagog-storage` PVC (replication 1). A
  multi-node layout across the Pis comes later.
- A bootstrap Job applies the layout, creates the `pedagog` bucket and an access key, and
  writes the key into a Secret that `jobs` reads.
- **Client:** `jobs` uses the `object_store` crate (new dependency) with the S3 backend.
  Build and run pods never receive S3 credentials, only short-lived presigned URLs
  (GET for archives, PUT for results).

## Schema

The implemented `assignment.yml` shape stays canonical (`id`, `name`, `environment`). A new
top-level block:

```yaml
testing:
  framework: pytest:1        # helper id:version
  isolation: process         # process (default) | in-process
```

- `framework: pytest:1` maps to the helper addon `pedagog-pytest:1` and is **added to the
  matching toolchain's addons** during resolve (python here), with the normal addon rules:
  the toolchain recipe must declare that helper version as compatible, it's vended and
  layered right after the toolchain, and duplicates are deduped. No matching toolchain
  selected is a resolve error.
- Exactly one framework per assignment.
- ARCHITECTURE.md's YAML sketch is updated to the implemented shape plus `testing:`.
  `submission:` and `assignment:` policy blocks come in later milestones.

## Image

- **Tag:** `pedagog/assignments/<assignment-id>:<hash>`, where the hash (blake3) covers the
  rendered Containerfile and the archive contents. Identical inputs never rebuild; a changed
  upload gets a new tag.
- **Base:** `FROM` the pre-built OS image in the in-cluster registry (`FromSource::PrebuiltBase`
  with the registry prefix).
- **New final phase, ASSIGNMENT FILES**, after OS CLEANUP (last, so editing tests only rebuilds
  this layer):

  | Source | Destination | Owner, mode |
  | --- | --- | --- |
  | `starter/` | `/pedagog/starter` | `root:root`, read-only (`a=rX`) |
  | `tests/` | `/pedagog/tests` | `grader:grader`, `0700` |
  | `solution/` | `/pedagog/solution` | `grader:grader`, `0700` |

  `ImageSpec::Full` gains an optional `files` part; it's absent for environment-only renders
  (examples, `hammer plan` without an archive).
- **Context:**
  - in-cluster, an initContainer (the jobs image running `pedagog-jobs fetch-context <url>
    <dir>`) downloads the archive via a presigned GET and unpacks it into the Kaniko context
    `emptyDir`;
  - locally, the assignment directory is the context.
- **Users:** the OS init contract adds `grader` (owns tests and solution, runs the harness)
  and `runner` (runs student code under isolation) next to `student`. Every OS recipe must
  create them; documented in ARCHITECTURE.md's OS init contract.

The solution is baked in grader-only, so validation needs no fetch. With process isolation,
student code runs as `runner` and can't read `grader` files (see Threat model).

## Validation run

A Kubernetes Job from the built image, in `pedagog-builds`:

1. Starts as **root in an unprivileged pod** (no added capabilities). The helper's launcher,
   installed as `/pedagog/bin/run-tests`:
   - starts the student-code server as `runner` (`setpriv`), pointed at the code under test
     (here `/pedagog/solution`, copied to a `runner`-readable scratch dir);
   - starts the test harness as `grader` against `/pedagog/tests`;
   - waits, then uploads `results.yaml` to the presigned PUT URL it was given (env
     `PEDAGOG_RESULTS_URL`).

   Nothing student-controlled ever runs as root.
2. **Network:** follows the assignment's `environment.network` (networking assignments need
   it), plus egress to Garage for the upload.
3. `jobs` waits for the Job, reads `results.yaml` from Garage, and validates it against the
   typed results model in `core`.
4. **Pass:** every test passes, except tests the helper marked as expected to fail with the
   reference solution (`@solution_expect("fail")`), which must fail. Anything else fails the
   build, and the results are kept so the instructor sees which tests disagree.

## Test helpers

- **One repo per framework**, all under pedagog-org:
  - `python-pytest` (this slice);
  - `cpp-catch2` and `rust-cargo` (later).

  Each is mounted in pedagog as a submodule under `test-frameworks/<name>`, used for CI and
  integration tests.
- **Distribution:** each tagged release publishes to its language registry (PyPI etc.) and
  attaches its artifact (e.g. the wheel) to the GitHub release. An addon recipe (e.g.
  `pedagog-pytest/1.yaml`) vends that asset and installs it; toolchain recipes declare
  compatible helper versions like any addon (`python/3.12.yaml`: `addons: { pedagog-pytest:
  ["1"] }`).
- **Output:** helpers write `/pedagog/results/results.yaml` directly, in ARCHITECTURE.md's
  results format, with metadata from decorators:

  ```python
  @weight(10)
  @number("1.1")
  @visibility(score="visible", output="after_end")
  @solution_expect("fail")      # optional; validation expects the reference solution to fail
  def test_append(): ...
  ```

- **Process isolation (pytest), on by default.** An import hook makes student modules
  resolve to **RPyC** proxies connected to the `runner` process, so tests stay native:

  ```python
  from linked_list import LinkedList   # proxy module; code runs as runner
  ```

  - RPyC runs in a restricted configuration that is **one-way**: values passed into student
    code cross as copies, never as proxies back into the grader process, so student code
    can't walk from an argument to the test's globals. This configuration gets its own
    adversarial tests in the helper repo.
  - `isolation: in-process` opts out (faster; student code shares the harness process).

### Isolation per framework (recorded)

Each framework declares what isolation it supports:

| Framework | Isolated | Not isolated |
| --- | --- | --- |
| pytest (and other dynamic languages) | Transparent proxies (default) | `in-process` opt-out |
| Catch2, cargo (compiled) | Black-box tests: student program as `runner`, driven via stdin/stdout, files, args, exit codes | White-box (linked) tests: marked *source hidden, logic exposed* and shown as such to instructors |

Generated RPC stubs from a declared interface (C header, Rust trait) are a possible future
option for native-feeling isolation in compiled languages.

## Threat model

- **Image extraction:** tests and solution are in the image. Students can't pull images (the
  registry's NetworkPolicy admits only platform and build namespaces), and in-container
  files are `grader` `0700`.
- **Student code during runs:** with process isolation, student code runs as `runner` and
  can't read tests or the solution. In `in-process` mode (and white-box compiled tests) it
  shares the harness process and can read them; that's documented as the opt-out trade-off.
- **Result probing:** whenever students see pass/fail on hidden tests, they can probe inputs
  bit by bit. Mitigations are visibility settings and (later) run-rate limits; isolation
  can't prevent it.
- **Network:** runs follow `environment.network`, so an assignment that allows egress also
  allows student code to reach those destinations during runs.

## Recorded for later

- **Student-triggered hidden runs:** student → daemon → the same root launcher → `grader` +
  `runner`. Results are filtered by visibility before the student sees them, with
  configurable rate limits. Reuses the helper and launcher unchanged.
- **Grading on submission:** the same run shape against the student's work instead of
  `/pedagog/solution`.
- **Starter seeding:** the first session copies `/pedagog/starter` into the empty workspace
  volume.

## Data model (`store`)

New migrations, following the OS-build pattern (TEXT status plus `CHECK`, reconcile on
dispatch):

- `assignments` — `id` (from `assignment.yml`), `name`, `created_at`.
- `assignment_uploads` — `assignment_id`, `sha256`, `object_key`, `uploaded_at`.
- `assignment_builds` — `upload_id`, `hash`, `image_ref`, `build_status`,
  `validation_status`, `results` (JSONB), `error`, timestamps. Plus an "is this hash already
  built?" lookup.

## Dependencies

- `object_store` (S3 backend), for `jobs`. It brings an HTTP client, which
  `pedagog-jobs fetch-context` reuses.
- RPyC, a Python dependency of the `python-pytest` helper only.
- No other new Rust dependencies expected; `blake3`, `sqlx`, `kube` and `tokio` are already
  there.

## Alternatives considered

- **Tests mounted at run time, never baked** — rejected in favor of one image per assignment,
  with tests guarded by the `grader` user.
- **Solution supplied only to the validation run** — rejected; baking it grader-only keeps
  validation self-contained, and isolation protects it.
- **Separate grader image** — rejected; two images per assignment.
- **api triggers builds via `k8s` directly** (the August split) — rejected for now; one
  builder service (`jobs`) behind an internal endpoint, called by api later.
- **Postgres bytea or PVC for archives; MinIO** — rejected in favor of an object store; MinIO
  community is archived. Garage over SeaweedFS for simplicity.
- **`aws-sdk-s3` / `rust-s3`** — rejected; `object_store` is lighter than the AWS SDK and
  better supported than `rust-s3`.
- **Results via pod logs** — rejected in favor of a presigned PUT, which also scales to
  artifacts.
- **Upload-counter tags (`:v1`)** — rejected; content hashes avoid redundant rebuilds.
- **Native output plus per-framework parsers** — rejected; helpers write `results.yaml`.
- **Helpers in this repo / one helpers monorepo** — rejected for one repo per framework,
  submoduled under `test-frameworks/`.
- **Installing helpers from registries at build time** — rejected; vend pinned release
  assets (hermetic).
- **Isolation opt-in / hand-rolled RPC** — rejected; isolation is the default, on RPyC with a
  restricted one-way configuration.
- **No egress during runs** — rejected; networking assignments need the assignment's policy.

## Open questions

- **RPyC hardening limits:** how far the restricted one-way configuration really holds
  (special methods, generators, exceptions carrying frames). The adversarial test suite
  decides whether gaps are acceptable or need a hand-rolled layer.
- **Kaniko and uid/gid-preserving `COPY --chown`:** confirm Kaniko applies ownership and
  modes for the ASSIGNMENT FILES phase as expected.
- **Registry pulls by nodes:** validation pods pull assignment images from the in-cluster
  registry. kind needs a containerd registry patch in `deploy/kind/cluster.yaml`; k3s uses
  SETUP Phase 4.
- **Run resources and timeouts** for validation pods (LimitRange defaults to start).
- **Garage on Longhorn** at RK1 scale: single replica now; layout and replication when the
  Pi cluster is up.
- **Upload size limit** for the intake endpoint.

## Rollback

- Each PR is additive:
  - a Garage manifest;
  - new migrations (with down migrations);
  - new `jobs` endpoints and code;
  - an optional `testing:` block and `files` render part (absent means today's behavior);
  - a new helper repo.
- The OS init contract change (two extra users) is backward-compatible for existing
  recipes.
- Reverting a PR removes its piece. Garage data is disposable in dev.

## Implementation steps

One PR per area, in order:

1. **Garage:**
   - StatefulSet, Service and PVC in `pedagog-data`;
   - bootstrap Job (layout, bucket, key Secret);
   - kind overlay support;
   - `object_store` wiring in `jobs` (config, client, presign helpers);
   - a smoke test against kind.
2. **Schema and render** (core and recipes):
   - the `testing:` block and framework → addon mapping in resolve;
   - `grader`/`runner` in the OS init contract;
   - the ASSIGNMENT FILES phase in both renderers;
   - the typed `results.yaml` model in `core`;
   - tests.
3. **`python-pytest` helper** (new repo `pedagog-org/python-pytest`, created after your
   go-ahead; submodule at `test-frameworks/python-pytest`):
   - decorators and `results.yaml` writer;
   - `solution_expect`;
   - the root launcher and `/pedagog/bin/run-tests`;
   - the RPyC import-hook isolation with adversarial tests;
   - a release with the wheel attached;
   - recipes: `pedagog-pytest/1.yaml` addon, plus python recipes declaring it.
4. **jobs assignment flow:**
   - migrations;
   - `POST /internal/assignments` (store, dispatch);
   - `fetch-context`;
   - Kaniko build with the archive context;
   - validation Job (presigned PUT for results);
   - results check;
   - reconcile;
   - `hammer upload`.
5. **kind end-to-end:**
   - containerd registry patch;
   - upload an example (python, with tests and solution);
   - image built and pushed;
   - validation passes;
   - an assignment whose solution fails a test produces a failed validation with results;
   - re-upload of identical content dispatches nothing.
6. **Docs:**
   - ARCHITECTURE.md: schema, assignment pipeline, Garage, users, threat model;
   - SETUP.md: Garage, `hammer upload`;
   - BUILD-ORDER.md: ASSIGNMENT FILES phase.
