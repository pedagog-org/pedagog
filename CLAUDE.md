# CLAUDE.md — Working conventions for the `pedagog` project

## What this project is

`pedagog` is a system for administering **browser-based coding exams**. Students connect from a
locked-down browser (Respondus Lockdown / Safe Exam Browser) to an ephemeral, restricted
**Podman container** running **VS Code in the browser**, do their work, and submit. A control
server orchestrates containers, timing, auth, submissions, and archival to blob storage.

See `docs/` for the full prompt/design history.

## Docs convention

For **each prompt** the user gives, maintain a pair of numbered Markdown files in `docs/`:

- `NN-prompt-<title>.md` — captures the user's prompt/requirements for that step.
- `NN-design-<title>.md` — captures the **design we agree on** for that step (write/finalize
  this only after we've actually agreed; it may start as a draft).

`NN` is a zero-padded, monotonically increasing sequence shared across prompt+design files so the
folder lists chronologically. Each new prompt gets the next number(s). Keep these docs current as
decisions evolve; note status at the top (e.g. `Draft`, `Agreed`, `Superseded by NN`).

**Standing instruction (from the user):** When the user provides additional information or
clarifications in follow-up messages for a topic, **append that information to the relevant
`NN-prompt-*.md` doc** so the prompt record stays complete — don't only keep it in the design doc
or in conversation. Treat the prompt doc as the living source of truth for what the user asked for.

## Working style for this project

- This is a greenfield design effort. Favor **asking clarifying questions and proposing options**
  over jumping to implementation, until a design is explicitly agreed.
- **Do NOT write code (Rust, configs, Containerfiles, scaffolding, etc.) unless the user explicitly
  and unambiguously asks for code in that message.** Stay in design/docs mode by default. A design
  decision being "agreed" is not permission to implement it. When in doubt, ask before writing code.
- Security and student-isolation are first-class concerns (this administers exams). Call out
  threat-model implications of design choices.
- Keep FERPA / student-data-privacy in mind for anything that stores or transmits student data.

## Code conventions

When code is eventually written, it MUST follow [`docs/05-design-code-conventions.md`]. In short:
idiomatic Rust modeling the domain exactly; edition 2024 + pinned toolchain; `thiserror` in libs and
`anyhow` only at binary boundaries; **no panics in production paths** (clippy denies
`unwrap`/`expect`/`panic` outside tests); newtypes + state enums + typestate; `pedagog-core` stays
pure (no I/O/DB); **SQLx** with explicit row↔domain mapping; `tokio` services + sync CLI; one concept
per module (no god-files); I/O behind traits for testability; `tracing` for logs; `nextest`; no
warnings left in the tree.
