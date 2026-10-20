PORT     := "8080"
WORKDIR  := "."
HAMMER   := "cargo run -q -p hammer --"
# For local dev; override to push/pull from a real registry.
REGISTRY := "localhost"
# Recipes directory — falls back to HAMMER_RECIPES env var.
RECIPES  := env_var_or_default("HAMMER_RECIPES", "")
# Per-container resource limits, sized for small student projects on RK1 (RK3588, 32GB, ~30+/node).
MEMORY      := "512m"
CPUS        := "1"
PIDS_LIMIT  := "256"
# Dev Postgres connection (mirrors deploy/base/postgres; override to point elsewhere).
export DATABASE_URL := env_var_or_default("DATABASE_URL", "postgres://pedagog:pedagog@localhost:5432/pedagog")

# Point git at the tracked hooks in .githooks/ (run once per clone).
install-hooks:
    git config core.hooksPath .githooks
    @echo "git hooks installed (core.hooksPath = .githooks)"

# Build a base OS image (e.g. just build-base ubuntu-22 pedagog/ubuntu:22).
build-base OS_ID IMAGE_TAG:
    {{HAMMER}} plan --os {{OS_ID}} \
        | podman build --volume "{{RECIPES}}:/pedagog/recipes:ro,z" -t "{{REGISTRY}}/{{IMAGE_TAG}}" -

# Download and cache recipe ingredients (filters: --os/--platform/--toolchain <ID>).
vend *ARGS:
    {{HAMMER}} vend {{ARGS}}

# Print the Containerfile for an assignment (FROM the pre-built base image).
plan ASSIGNMENT:
    {{HAMMER}} plan -a {{ASSIGNMENT}} --registry {{REGISTRY}}

# Print a human-readable tree of an assignment's build: phases, layers, steps, runtime.
describe ASSIGNMENT:
    {{HAMMER}} plan -a {{ASSIGNMENT}} --registry {{REGISTRY}} -f describe

# Build a container image from an assignment (uses vendored ingredients if present).
build ASSIGNMENT:
    #!/usr/bin/env bash
    set -euo pipefail
    tag="{{REGISTRY}}/pedagog/$(basename {{ASSIGNMENT}} .yaml)"
    {{HAMMER}} plan -a {{ASSIGNMENT}} --registry {{REGISTRY}} \
        | podman build --volume "{{RECIPES}}/ingredients:/pedagog/ingredients:ro,z" -t "$tag" \
            -f - "$(dirname {{ASSIGNMENT}})"

# Squash an assignment image's layers in place to reclaim space from deleted files.
squash ASSIGNMENT:
    #!/usr/bin/env bash
    set -euo pipefail
    tag="{{REGISTRY}}/pedagog/$(basename {{ASSIGNMENT}} .yaml)"
    podman build --squash-all -t "$tag" - <<< "FROM $tag"

# Build and immediately run an assignment image.
dev ASSIGNMENT *ARGS: (build ASSIGNMENT)
    just run {{ASSIGNMENT}} {{ARGS}}

# Run an assignment image, mounting WORKDIR into /workspace and exposing PORT.
run ASSIGNMENT *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    tag="{{REGISTRY}}/pedagog/$(basename {{ASSIGNMENT}} .yaml)"
    podman run --rm -it \
        --memory {{MEMORY}} --memory-swap {{MEMORY}} \
        --cpus {{CPUS}} \
        --pids-limit {{PIDS_LIMIT}} \
        -v "$(realpath {{WORKDIR}}):/workspace" \
        -p {{PORT}}:{{PORT}} \
        "$tag" {{ARGS}}

# Start a local dev Postgres in podman (mirrors prod: postgres:16-alpine, db/user 'pedagog').
db-up:
    podman run -d --replace --name pedagog-db \
        -e POSTGRES_DB=pedagog -e POSTGRES_USER=pedagog -e POSTGRES_PASSWORD=pedagog \
        -p 5432:5432 -v pedagog-pgdata:/var/lib/postgresql/data \
        docker.io/library/postgres:16-alpine
    @echo "dev Postgres up: {{DATABASE_URL}}"

# Stop and remove the dev Postgres (keeps the named volume; add 'podman volume rm pedagog-pgdata' to wipe).
db-down:
    podman rm -f pedagog-db

# Open a psql shell on the dev DB.
db-shell:
    podman exec -it pedagog-db psql -U pedagog -d pedagog

# A freshly migrated, empty database for the .sqlx/ cache: column nullability is
# inferred from query plans, which change with table statistics, so a populated
# dev DB can give a different cache than CI's.
PREPARE_URL := "postgres://pedagog:pedagog@localhost:5432/pedagog_prepare"

[private]
db-empty:
    podman exec pedagog-db psql -U pedagog -q -c "DROP DATABASE IF EXISTS pedagog_prepare" -c "CREATE DATABASE pedagog_prepare"
    DATABASE_URL={{PREPARE_URL}} cargo sqlx migrate run --source crates/store/migrations

# Apply migrations to the dev DB and refresh the committed .sqlx/ offline query cache
# (against an empty DB, as CI). --all-features so the feature-gated store::db
# queries are reached, --all-targets for the tests'.
db-prepare: db-empty
    cargo sqlx migrate run --source crates/store/migrations
    DATABASE_URL={{PREPARE_URL}} cargo sqlx prepare --workspace -- --all-targets --all-features

# Check the committed .sqlx/ cache against an empty DB (the pre-commit hook runs this
# when a query or migration is staged).
db-prepare-check: db-empty
    DATABASE_URL={{PREPARE_URL}} cargo sqlx prepare --workspace --check -- --all-targets --all-features

# ---- Web UI (crates/web: Leptos in the browser, built with trunk) -----------

# Serve the web UI on http://localhost:3000, rebuilding on change; /api/ goes to
# the kind api (run `just kind-api` alongside). Needs trunk (`brew install trunk`).
web-dev:
    cd crates/web && trunk serve

# Build the web UI's static files (index.html, .wasm, .js, .css) into target/web.
web-build:
    cd crates/web && trunk build --release

# ---- Local kind cluster (podman) for end-to-end tests -----------------------
KIND := "KIND_EXPERIMENTAL_PROVIDER=podman kind"
KIND_CTX := "kind-pedagog"

# (Re)create the kind cluster with this checkout's recipes mounted into the node.
# Deletes an existing `pedagog` kind cluster first. Needs a rootful podman machine.
kind-up:
    #!/usr/bin/env bash
    set -euo pipefail
    if {{KIND}} get clusters 2>/dev/null | grep -qx pedagog; then {{KIND}} delete cluster --name pedagog; fi
    sed "s|@RECIPES@|{{justfile_directory()}}/recipes|" deploy/kind/cluster.yaml | {{KIND}} create cluster --config -

# Delete the kind cluster.
kind-down:
    {{KIND}} delete cluster --name pedagog

# Build the jobs image with podman and load it into the kind node.
kind-load-jobs:
    podman build -f crates/jobs/Containerfile -t localhost/pedagog/jobs:dev .
    podman build -f crates/jobs/Containerfile --target init -t localhost/pedagog/jobs-init:dev .
    podman save --format docker-archive -o target/jobs-image.tar localhost/pedagog/jobs:dev
    podman save --format docker-archive -o target/jobs-init-image.tar localhost/pedagog/jobs-init:dev
    {{KIND}} load image-archive target/jobs-image.tar --name pedagog
    {{KIND}} load image-archive target/jobs-init-image.tar --name pedagog

# Build the api image and load it into the kind node.
kind-load-api:
    podman build -f crates/api/Containerfile -t localhost/pedagog/api:dev .
    podman save --format docker-archive -o target/api-image.tar localhost/pedagog/api:dev
    {{KIND}} load image-archive target/api-image.tar --name pedagog

# Make someone an admin on kind (how the first admin gets in).
kind-add-admin EMAIL:
    kubectl --context {{KIND_CTX}} -n pedagog-system exec deploy/pedagog-api -- /usr/local/bin/pedagog-api add-admin "{{EMAIL}}"

# Port-forward the api (http://localhost:18082), Mailpit's inbox
# (http://localhost:18025) and GlitchTip (http://localhost:18000) until interrupted.
kind-api:
    #!/usr/bin/env bash
    set -euo pipefail
    kubectl --context {{KIND_CTX}} -n pedagog-data port-forward svc/mailpit 18025:8025 >/dev/null &
    mail=$!
    kubectl --context {{KIND_CTX}} -n pedagog-data port-forward svc/glitchtip 18000:8000 >/dev/null &
    trap 'kill $mail $!' EXIT
    echo "api: http://localhost:18082   mail: http://localhost:18025   errors: http://localhost:18000"
    kubectl --context {{KIND_CTX}} -n pedagog-system port-forward svc/pedagog-api 18082:8080

# Apply the kind overlay (full base, dev-mode jobs, local-path storage).
kind-deploy:
    kubectl --context {{KIND_CTX}} apply -k deploy/overlays/kind
    just kind-registry

# Let the kind node pull from the in-cluster registry by its service name (plain
# HTTP), e.g. assignment images for validation pods. Idempotent; enables
# containerd's per-registry config on clusters created before cluster.yaml did.
kind-registry:
    #!/usr/bin/env bash
    set -euo pipefail
    ip=$(kubectl --context {{KIND_CTX}} -n pedagog-data get svc registry-service -o jsonpath='{.spec.clusterIP}')
    dir=/etc/containerd/certs.d/registry-service.pedagog-data.svc:5000
    podman exec pedagog-control-plane sh -c '
      grep -q config_path /etc/containerd/config.toml || {
        printf "\n[plugins.\"io.containerd.grpc.v1.cri\".registry]\n  config_path = \"/etc/containerd/certs.d\"\n" >> /etc/containerd/config.toml
        systemctl restart containerd
      }'
    podman exec pedagog-control-plane sh -c "mkdir -p '$dir' && printf 'server = \"http://$ip:5000\"\n\n[host.\"http://$ip:5000\"]\n  capabilities = [\"pull\", \"resolve\"]\n' > '$dir/hosts.toml'"
    echo "kind node pulls registry-service.pedagog-data.svc:5000 from http://$ip:5000"

# Log a persona in on kind with the code from Mailpit (they must be registered,
# e.g. by `just kind-add-admin`); the session is kept in target/kind/<NAME>,
# NAME being the email's local part unless given.
kind-login EMAIL NAME="":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -q -p hammer
    source scripts/kind-lib.sh
    kind_forwards
    trap kind_unforward EXIT
    name="{{NAME}}"
    name=${name:-$(echo "{{EMAIL}}" | cut -d@ -f1)}
    kind_login "{{EMAIL}}" "$name"
    kind_as "$name" whoami

# Run hammer on kind as a persona from `just kind-login`, e.g.
# `just kind-as prof upload recipes/examples/linked-list-python --wait`.
kind-as NAME *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -q -p hammer
    source scripts/kind-lib.sh
    kind_forwards
    trap kind_unforward EXIT
    HAMMER_RECIPES="{{justfile_directory()}}/recipes" kind_as "{{NAME}}" {{ARGS}}

# End-to-end check through the api on kind: logins, roles, builds, validation,
# egress, submissions, dates, visibility, release, activation, regrades and
# notices. Run after kind-load-jobs, kind-load-api and kind-deploy.
kind-e2e:
    scripts/kind-e2e.sh

# Trigger an OS rebuild through the jobs endpoint (port-forwarded); e.g. `just kind-rebuild force=true`.
kind-rebuild QUERY="":
    #!/usr/bin/env bash
    set -euo pipefail
    kubectl --context {{KIND_CTX}} -n pedagog-builds port-forward svc/pedagog-jobs 18081:8080 >/dev/null &
    trap 'kill $!' EXIT
    sleep 2
    curl -fsS -X POST "http://localhost:18081/internal/recipes/rebuild?{{QUERY}}"; echo
