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
    {{HAMMER}} plan -a {{ASSIGNMENT}}

# Build a container image from an assignment (uses vendored ingredients if present).
build ASSIGNMENT:
    #!/usr/bin/env bash
    set -euo pipefail
    tag="{{REGISTRY}}/pedagog/$(basename {{ASSIGNMENT}} .yaml)"
    {{HAMMER}} plan -a {{ASSIGNMENT}} --registry {{REGISTRY}} \
        | podman build --volume "{{RECIPES}}/ingredients:/pedagog/ingredients:ro,z" -t "$tag" -

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

# Apply migrations to the dev DB and refresh the committed .sqlx/ offline query cache.
# --all-features so the feature-gated store::db queries are reached.
db-prepare:
    cargo sqlx migrate run --source crates/store/migrations
    cargo sqlx prepare --workspace -- --all-features
