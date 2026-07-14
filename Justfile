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

# Build a base OS image (e.g. just build-base ubuntu-22 pedagog/ubuntu:22).
build-base OS_ID IMAGE_TAG:
    {{HAMMER}} plan -o {{OS_ID}} -f containerfile \
        | podman build --volume "{{RECIPES}}:/pedagog/recipes:ro" -t "{{REGISTRY}}/{{IMAGE_TAG}}" -

# Print a human-readable build plan for an assignment.
plan ASSIGNMENT:
    {{HAMMER}} plan -a {{ASSIGNMENT}}

# Print a Containerfile for an assignment.
plan-containerfile ASSIGNMENT:
    {{HAMMER}} plan -a {{ASSIGNMENT}} -f containerfile

# Build a container image from an assignment.
build ASSIGNMENT:
    #!/usr/bin/env bash
    set -euo pipefail
    tag="{{REGISTRY}}/pedagog/$(basename {{ASSIGNMENT}} .yaml)"
    {{HAMMER}} plan -a {{ASSIGNMENT}} -f containerfile --registry {{REGISTRY}} \
        | podman build -t "$tag" -

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
