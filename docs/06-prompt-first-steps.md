# 06 — Prompt: First Implementation Steps

> **Date:** 2026-06-20
> **Status:** Prompt captured; plan under discussion (see `07-design-*` once agreed).
> No code is written until explicitly approved; implementation follows
> [`05-design-code-conventions.md`](./05-design-code-conventions.md).

## User's stated direction

- Get a **container with `code-server` working first**.
- **Then** write the **CLI & daemon**.
- Open to ideas on sequencing.

## Decisions (2026-06-21)

- **Image build tooling:** **Containerfiles** on a Wolfi base — for the whole project, not a
  throwaway. There is **no required migration to apko/melange**; cosign signing and multi-arch work
  fine from a Containerfile, and we already remove the package manager via `restrict apt`.
  apko/melange remain an *optional later* hardening (bit-reproducible builds + SBOMs) only if wanted.
- **Sequencing:** the proposed milestones are accepted (M1 thin code-server → M1.5 path-routing
  spike → M2 full base image → M3 CLI + daemon against a control-plane stub → Phase 3+ real control
  plane). Detailed in `07-design-first-steps.md`.
- **Local runtime:** **Podman** (rootless); on macOS via `podman machine`.
