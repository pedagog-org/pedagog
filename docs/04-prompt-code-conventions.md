# 04 — Prompt: Code Expectations & Conventions

> **Date:** 2026-06-20
> **Status:** Prompt captured; conventions under discussion (see `05-design-*` once agreed).

## Goal

Agree on the engineering standards for the `pedagog` codebase **before** writing code.

## Stated expectations (from the user)

- **Idiomatic Rust that models exactly what is happening** — not the easy path. This is software used
  by **hundreds of students**, not a script. Model the domain precisely.
- **No compiler errors; warnings must be addressed** (not left in the tree).
- **Errors are handled, not unwrapped** — propagate via `Result`; **avoid panics** (no
  `unwrap()`/`expect()` that can blow up in production paths).
- **Proper file/module separation** — do not throw everything into one file; organize correctly.

## Notes
- Conventions, once agreed, will be recorded in `05-design-code-conventions.md` and enforced in
  `CLAUDE.md` (and tooling config) so they're applied consistently.
