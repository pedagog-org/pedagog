# Build inputs: always rebuild, pinned ingredients in the steps, ingredients in Garage

## Rationale

Previewing pedagog-pytest 2.2.0 found three problems with how images are built:

- **Stale ingredients.** An image's hash covers its rendered Containerfile and the
  assignment's files, not the ingredient files its steps install (`vend`). Changing an
  ingredient's pinned file without changing a step's text reuses the old image, and
  Kaniko's layer cache (keyed on a step's text and the layers before it) reuses the old
  step even on a rebuild. The 2026-08-06 plan deferred this ("full build-input hashing";
  force rebuild as the interim).
- **Reuse surprises.** An identical re-upload does nothing ("already built"); the user
  expects a re-upload to rebuild.
- **Every build copies every ingredient.** Ingredients are vended when the jobs image is
  built and baked into it (~200 MB today: `code-server.deb` 196 MB, two pytest bundles
  2 MB each). Every build pod's `stage-recipes` init copies all of them into its scratch
  space, a Python assignment included. Changing one means rebuilding and redeploying
  jobs. In dev, kind builds read the checkout's `recipes/`, so an ingredient nobody
  vended there silently falls back to downloading.

## Decisions

1. **Every upload rebuilds.** No "already built": an upload always starts a build (and
   supersedes one in flight). `--force` is removed (hammer's flag, the `force` query on
   the api and jobs, and the OS rebuild's): nothing is ever reused, so there's nothing to
   force. Regrades and `--take-dates` always follow the new build.
2. **Steps carry their ingredients' checksums.** Each recipe layer that declares
   ingredients renders `ENV PEDAGOG_INGREDIENTS="<file>=<sha256> ..."` after its
   `PEDAGOG_TYPE`/`PEDAGOG_ID`, so a changed pin changes the image hash and every later
   step's cache key. Ingredients without a sha256 render their source (URL, or repo, tag
   and asset) instead.
3. **Ingredients live in Garage, once per content.** `ingredients/sha256/<hex>`, shared by
   every recipe and assignment that pins the same file.
   - **Jobs syncs at startup:** it reads every ingredient its recipes pin and downloads
     the missing ones (checking sha256) into Garage. After start, builds never reach
     GitHub. A pin without a sha256 is downloaded and stored by the hash it gets, with a
     warning to pin it.
   - **Builds fetch only what they declare,** by checksum, with `jobs-init`'s
     `fetch-context` (which already fetches and verifies by hash), into
     `/pedagog/ingredients/<type>/<id>/<file>` where `vend` looks. The `stage-recipes`
     init (the jobs image copying everything) goes away; the rendered Containerfile
     doesn't need the recipes folder at build time (the OS init step that installs
     `vend` reads `/pedagog/recipes/lib/vend`: that one file is fetched too, or inlined).
   - **Cleanup, at once:** an ingredient no recipe pins any more is deleted by the
     startup sync that finds it unpinned (images are already built, so nothing needs it
     again), except while a build in flight still declares it (a jobs replica on the
     old recipes during a rollout); those go at the next sync. The nightly maintenance
     repeats the check as a backstop.
   - **Dev:** kind uses the same Garage path; no hostPath recipes for ingredients, so
     nothing to vend by hand.
   - The jobs image drops its vend stage (~200 MB smaller).

## Alternatives considered

- **Keep baking, copy only what a build needs:** a small change, but new ingredients still
  mean a jobs release, and the jobs image grows with every toolchain.
- **A separate ingredients image** (`pedagog/ingredients:<hash of pins>`): decoupled from
  jobs releases, but every build still copies everything; the registry dedupes layers,
  not files.
- **Ingredients in base images:** no per-build fetch, but toolchain ingredients (the
  pytest bundle) aren't in the base, and base images grow.
- **Exact hashing with reuse kept:** cheapest, but keeps "already built", which isn't
  wanted.
- **Downloading during the build:** not hermetic; builds depend on GitHub.

## Open questions

- **Startup with GitHub down:** jobs starts anyway, logs and reports missing ingredients
  (GlitchTip), and builds needing one fail with "ingredient X isn't available yet"; it
  retries the sync every 10 minutes. (Agreed.)
- **The `kind` hostPath for recipes:** with ingredients in Garage, is the hostPath still
  needed in dev (recipes are rendered by jobs, not read by builds)? Drop it if not.

## Rollback

Each step is its own PR. Step 1 (checksums) and step 2 (always rebuild) revert on their
own. Step 3 keeps the old `stage-recipes` path behind a config switch for one release
(`PEDAGOG_INGREDIENTS=baked|garage`), so a revert is a config change.

## Steps

1. **Checksums in the steps** (`fix/ingredient-checksums`): core's Containerfile
   renderer emits `ENV PEDAGOG_INGREDIENTS` per layer; tests that a changed pin changes
   the rendered text (and so the image hash); ARCHITECTURE.
2. **Always rebuild** (`feat/always-rebuild`): jobs' dispatch drops the `usable_build_for`
   short cut; regrade and take-dates always attach to the new build; hammer's "already
   built" message goes; `--force` removed everywhere (hammer, api, jobs, the OS rebuild
   and xtask's `kind-rebuild`, which always rebuilds); the e2e checks stop passing it,
   and `reupload_is_recognized` becomes "a re-upload rebuilds"; ARCHITECTURE.
3. **Ingredients in Garage** (`feat/garage-ingredients`):
   - move the vend download code (GitHub API, URL, sha256 check) from hammer into core
     so jobs can use it; `objects::ingredient_key(sha256)`;
   - jobs' startup sync (and a retry loop), reported in its health and logs;
   - `kaniko_job` adds per-ingredient fetches to `fetch-context` (a list of
     `<sha256> <dest>`), drops `stage-recipes`, keeps a switch for the old path;
   - deleting unpinned ingredients in the sync (skipping ones a build in flight
     declares), with the nightly maintenance as a backstop;
   - the jobs image without the vend stage; xtask stops mounting recipes for kind
     builds if the open question allows;
   - tests: core (key, render), jobs (sync with a fake source; cleanup), k8s (the job
     spec fetches exactly the declared ingredients), kind e2e (a build after a pin
     change uses the new file).
4. **Then the pytest 2.2.0 rollout** resumes (sanitized-errors plan, steps 4–5), on the
   fixed build path.
