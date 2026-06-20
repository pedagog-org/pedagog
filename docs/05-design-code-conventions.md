# 05 — Design: Code Conventions

> **Date:** 2026-06-20
> **Status:** Agreed. Standards the `pedagog` codebase must follow. Implements
> [`04-prompt-code-conventions.md`](./04-prompt-code-conventions.md).
> These apply once we actually write code (not yet — code is written only when explicitly asked).

---

## 1. Guiding principle

Idiomatic Rust that **models exactly what is happening**, not the easy path. This is software run by
**hundreds of students under exam conditions**; correctness, clarity, and failure-handling come
before brevity.

## 2. Language & toolchain

- **Edition 2024.**
- **Pinned toolchain** via `rust-toolchain.toml` (reproducible builds across dev and the cluster).
- **Async:** `tokio` for the network services (`pedagog-daemon`, `pedagog-control`, `pedagog-web`).
  The **CLI (`pedagog`) stays synchronous** (a single socket round-trip).

## 3. Error handling

- **Library crates** define **precise, typed error enums** with [`thiserror`]; propagate with `?`;
  use `#[from]`/`#[error(transparent)]` for source conversions. Callers can match specific variants.
- **Binary crates** use [`anyhow`] **only at the top boundary** (`main` / top-level run fn) for
  human context (`.context("…")`) and a clean nonzero exit. No `anyhow` deep in libraries.
- **No panics in production paths.** `unwrap()`, `expect()`, `panic!`, `todo!`, `unimplemented!`,
  and panicking indexing are **denied by clippy** in non-test code. A rare `expect("…")` is allowed
  **only** for a genuinely-proven invariant, with a comment explaining why it can't fail.
- **Tests** may use `unwrap`/`expect` freely (the restriction lints are allowed under `cfg(test)`).

## 4. Domain modeling

- **Newtypes for domain identifiers** (`SessionId`, `Sid`, `Uid`, `AllocId`, …) — never bare
  `String`/`u32`. Validate on construction (`parse`-don't-validate at boundaries).
- **States as enums**, not stringly-typed status; data-carrying variants where the state owns data.
- **Typestate** (compile-time state, e.g. `Session<Active>` vs `Session<Ended>`) where it prevents
  real misuse — make illegal operations fail to compile rather than at runtime.
- **`pedagog-core` is pure**: domain types only, **no I/O and no persistence/`sqlx` dependencies**.

## 5. Persistence (control plane → Postgres)

- **[`SQLx`]** (not an ORM): hand-written SQL **checked against the real schema at compile time**,
  async.
- **Domain type ≠ DB row.** The storage layer owns flat `FromRow` row structs and **explicit
  `TryFrom<Row>` / `Into<Row>` conversions** to/from the rich `pedagog-core` domain types. The DB
  boundary is where invariants are **re-established** via fallible conversion.
- Newtypes map via `#[sqlx(transparent)]`; simple enums via `#[derive(sqlx::Type)]`; data-carrying
  domain enums via a tag column + nullable data columns + the conversion above.
- Typestate is in-memory only; on read, load a row → runtime enum → checked constructor when a path
  needs the compile-time guarantee.

## 6. Module & crate organization

- **Workspace crate split** (clear separation of work): `pedagog-core` (pure domain),
  `pedagog-proto` (wire DTOs), `pedagog-identity`, `pedagog-control`, `pedagog-web`,
  `pedagog-nomad`, `pedagog-daemon`, `pedagog-cli`, `pedagog-storage`. Crates depend **inward**.
- **One concept per module**; `lib.rs`/`main.rs` is a thin surface, not a dumping ground. No
  god-files.
- **Abstract I/O and syscalls behind traits** (peer-cred, nftables, Nomad API, object storage, DB)
  so domain/logic is unit-testable without a Linux host or privileges; real impls live behind the
  trait and are exercised in integration tests.

## 7. Lints & formatting

- A workspace **`[lints]`** table; **warnings are not left in the tree** and CI runs
  `cargo clippy -- -D warnings` (and `cargo fmt --check`).
- **`clippy`** baseline `clippy::all`; **restriction lints denied** in non-test code:
  `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented` (tests exempt via
  `cfg_attr(test, allow(...))`).
- **`unsafe`:** `unsafe_code = "deny"` workspace-wide, downgraded to `allow` **only** in the
  specific low-level modules that need it (peer-cred, nft), each call with a `// SAFETY:` rationale.
  (Using `deny` not `forbid` so those documented exceptions are possible.)
- **`rustfmt`** with stock settings.
- **`#![warn(missing_docs)]`** on the **library** crates (public API documented).

## 8. Observability

- **Structured logging via [`tracing`]** (+ `tracing-subscriber`) across the services — spans and
  structured fields. Important for an exam/audit system; never log secrets (`session_id`, tokens).

## 9. Testing

- **`cargo nextest`** as the runner; unit tests colocated (`#[cfg(test)] mod tests`), cross-crate /
  integration tests in `tests/`.
- Consider **`proptest`** for the manifest and wire-protocol parsers.
- Platform-specific code (Linux syscalls) is tested behind its trait; the real syscall paths run in
  **CI on Linux**.

## 10. Dependencies

- Prefer **few, well-maintained** crates; justify additions. Minimize transitive surface.
- **Not adopted (for now):** `cargo-deny` / `cargo-audit` supply-chain gating — revisit later.

## 11. CI (to be set up when code begins)

- Gates: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo nextest run`, build.
- Must build/test for **arm64** (the Raspberry Pi targets) as well as the dev arch.
- CI platform: **TBD** (GitHub Actions / GitLab CI — decide before implementation).
