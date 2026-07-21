# CLAUDE.md — Pedagog

## Collaboration Model

- Never implement without explicit instruction. Discuss and document first; wait for the user to say to proceed.
- Ask before acting on anything, including small edits, file creation, or config changes.
- Flag issues proactively and early, even if unrelated to the current task. Stop and surface the concern; do not silently fix or continue.
- Batch clarifying questions into a single response rather than asking one at a time.
- Always surface risks and concerns unprompted during discussions.
- When making any judgment call with real tradeoffs, present multiple options with tradeoffs — do not pick one unilaterally.

## Plans

- Every implementation plan goes in `docs/plans/YYYY-MM-DD-<slug>.md`.
- Plans must include: rationale, alternatives considered, open questions, rollback plan, and step-by-step implementation.
- Plans are reviewed by the user before implementation begins.
- After implementation, plans are left as-is (historical record).

## Documentation

- `docs/ARCHITECTURE.md` is the living architecture document. Keep it evergreen — update it as design evolves.

## Git Conventions

- Commit style: Conventional Commits (`feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`, etc.)
- Branch naming: `<type>/<slug>` (e.g. `feat/submission-pipeline`, `fix/timeout-handler`)
- Commit often — small, focused commits are preferred over large batched ones.

## Testing

- Tests are required for essentially all new logic. Exceptions must be explicitly justified.
- Tests live inline in the same file as the code under test, in a `#[cfg(test)] mod tests` submodule (Rust convention).

## Code Style

- Rust only.
- Comments only where behavior is non-obvious. No paragraph-length comment blocks.
- Dependencies: ask the user before adding any new dependency.

## Response Format

- Use bullet points, even for long answers, so the user can respond point by point.
- Keep prose minimal.

## Project Context

- Domain: programming assignment/assessment platform for universities.
- Scale: multi-tenant; starting on multiple Raspberry Pi nodes for a single course, with a path to full Kubernetes.
- Infrastructure: Podman + Kubernetes + Traefik.
- Dev experience is a first-class goal: local setup and production deployment must both be step-by-step easy.
- Versioning: deferred — to be decided later.
