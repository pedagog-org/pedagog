# Disk Safeguards

## Rationale

kind filled its disk once (Kaniko: "no space left on device"). On kind today the
registry holds 3.5 GB (on a PVC sized 1 Gi; local-path doesn't enforce it) and the
node's image store 27 GB. Nothing deletes assignment images or Kaniko's layer cache,
every build adds an image per course and assignment, and pods have no local-disk
limits, so one runaway build or test can fill a node. On the Pis (SD cards or small
SSDs) this would take the cluster down mid-course.

## Decisions (2026-10-20)

- **Retention:** keep each assignment's current build image, images of builds in
  flight (building or validating) and of builds with queued or grading runs, and the
  OS base images (`pedagog/ubuntu:*`); delete every other tag under
  `pedagog/assignments/`. Kaniko cache images (`pedagog/cache`) older than 14 days
  (by their config's `created`) go; a later build re-caches.
- **Mechanism:** jobs deletes unused manifests nightly through the registry API (it
  knows what's in use from Postgres, and its warnings reach GlitchTip), then runs
  `registry garbage-collect --delete-untagged` as a Job on the registry's volume.
- **Alerts:** a DaemonSet probe measures each node's disk and image store every 10
  minutes; jobs checks the registry volume after GC and Postgres's size; each logs
  a warning above 80% used, an error above 90%, and GlitchTip groups and emails.
- **Limits:** ephemeral-storage on Kaniko pods (request 2Gi, limit 8Gi) and on
  validation and grading pods (request 256Mi, limit 1Gi), overridable by env var.

## Design

### Registry cleanup (jobs)

- The registry allows deletes (`REGISTRY_STORAGE_DELETE_ENABLED=true`).
- Nightly at `PEDAGOG_REGISTRY_CLEANUP_AT` (default `03:00`, the cluster's UTC unless
  `TZ` is set), one jobs replica (advisory lock `registry-cleanup`):
  1. Lists `pedagog/assignments/*` and `pedagog/cache` (catalog and tags), resolves
     each tag's digest (HEAD with the OCI and Docker manifest types), and deletes
     the manifests not kept (by digest).
  2. Marks builds whose image it deleted (`assignment_builds.image_deleted_at`,
     migration 0014), so `usable_build_for` never reuses a deleted image: the same
     content uploaded again rebuilds.
  3. Waits until no build is running (up to 30 minutes, then skips GC tonight with a
     warning), and creates the GC Job: the `registry:2` image, the registry's PVC,
     on the registry pod's node (pod affinity; the PVC is ReadWriteOnce).
  4. Logs what it deleted and freed (info), and warns on any failure.
- **GC and pushes:** garbage-collect can delete blobs of a push in progress. jobs
  starts it only when no build is running; an upload arriving during the minute GC
  runs could fail its push, and its build then fails visibly (re-upload fixes it).
  A strict lock across replicas was judged not worth it at nightly volume.
- `hammer`-free: an operator can run it now with `POST /internal/registry/cleanup`
  (port-forward), e.g. to test it.

### Disk alerts (a DaemonSet probe, not kubelet stats)

- **Nodes:** a DaemonSet `pedagog-disk` in `pedagog-builds` runs the jobs binary
  (`pedagog-jobs disk-report`) on every node. Every 10 minutes it measures, with
  `statvfs` on read-only hostPath mounts, the node's root filesystem and its image
  store (`/var/lib/rancher/k3s/agent/containerd` on k3s, `/var/lib/containerd` on
  kind; set per overlay), and reports to GlitchTip itself (the jobs DSN Secret).
  No new RBAC.
- **Registry volume:** the nightly GC Job ends with `df` of its volume; jobs reads
  the Job's log and checks it.
- **Postgres:** jobs compares the databases' total size (`pg_database_size`) with
  the PVC's capacity (a Role to read PVCs in `pedagog-data`); approximate (WAL and
  bloat aren't counted), so its thresholds are the same but it's a hint.
- Above `PEDAGOG_DISK_WARN` (80%): `warn!`; above `PEDAGOG_DISK_ERROR` (90%):
  `error!`, with node, what, used and capacity. GlitchTip groups repeats.

### Limits

- Kaniko pods: `PEDAGOG_BUILD_EPHEMERAL_REQUEST` / `_LIMIT` (2Gi / 8Gi).
- Validation and grading pods: `PEDAGOG_TEST_EPHEMERAL_REQUEST` / `_LIMIT`
  (256Mi / 1Gi); `pedagog-grading`'s LimitRange gets the same defaults and its
  ResourceQuota a `requests.ephemeral-storage` cap.
- An evicted pod reads as a failed build or an errored run; the run error kind for
  an eviction over ephemeral storage is `out_of_disk` ("your program used too much
  disk"), alongside `out_of_memory`.

### SETUP guidance

- kubelet image GC thresholds (`imageGCHighThresholdPercent` /
  `LowThresholdPercent`) and eviction thresholds for k3s; image storage on an SSD
  rather than the SD card on Pis; how to read the alerts and run a cleanup now.

## Alternatives considered

- **zot instead of registry:2:** built-in retention and online GC (no window), but a
  new component; jobs would still decide what's in use.
- **A CronJob script (curl + psql) for deletion:** duplicates jobs' knowledge of
  what's in use, and its failures wouldn't reach GlitchTip.
- **Prometheus + Alertmanager:** richer, but several more components on the Pis,
  and alerts apart from GlitchTip.
- **Node conditions only (DiskPressure):** harmless RBAC, but they fire only at
  eviction thresholds, too late to act.

## Open questions

- ~~kubelet stats need `nodes/proxy`~~ (decided 2026-10-20: it also allows exec into
  any pod through kubelet, so a DaemonSet probe measures nodes instead).
- The run error kind name for evictions (`out_of_disk`) and its student sentence.

## Rollback

- Revert the PR; migration 0014 has a down migration (drops `image_deleted_at`).
  Deleted images can't be restored: the next upload of that content rebuilds.
- To stop cleanup without reverting: `PEDAGOG_REGISTRY_CLEANUP_AT=off`.

## Implementation steps

1. `feat(store)`: migration 0014 `image_deleted_at`; `usable_build_for` skips
   deleted images; queries for the image refs in use.
2. `feat(jobs)`: registry client (catalog, tags, digest, delete) and the cleanup
   decision (pure: kept set → tags to delete), with tests.
3. `feat(jobs)`: the nightly schedule, the GC Job (k8s), and the internal endpoint.
4. `feat(k8s)`: ephemeral-storage on Kaniko, validation and grading pods; eviction →
   `out_of_disk`.
5. `feat(jobs)`: `disk-report` (the DaemonSet's loop), the registry volume check
   after GC, and the Postgres size check.
6. `feat(deploy)`: registry deletes, RBAC, LimitRange and quota, env.
7. `docs`: SETUP and ARCHITECTURE; kind: run a cleanup and check space is freed,
   then kind-e2e.
