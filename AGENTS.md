# AGENTS.md

Denju is a greenfield Rust implementation. The former Go/Agentbox product is preserved only as historical Git data on `legacy/go-agentbox-v0.2.0`.

**Implementation agents must not read the old code.** Do not check out the legacy branch, `git show` its source, diff against it, grep it, copy tests from it, or use it for behavioral parity or implementation examples. Build from the product specification, current `main`, and the current workstream package only. A separate user-requested historical investigation is outside this implementation workflow.

## Start here

1. Read the authoritative product specification at `tmp/gg/denju-product-spec-2026-08-20.md` when your task is part of the Rust workstream.
2. Read the current implementation plan and progress ledger under `tmp/gg/denju-rust-greenfield/` before changing product behavior.
3. Read only the phase-scoped Sweep/source regions named by the plan; do not wander through old planning artifacts.

## Repository map

- `apps/denju/` — CLI + background daemon process wiring.
- `apps/denju-server/` — registry process wiring.
- `crates/denju-core/` — pure IDs, paths, Merkle/revision/merge domain logic.
- `crates/denju-wire/` — versioned JSON, CLI structured output, API/SSE contracts.
- `crates/denju-sync/` — deterministic reconciliation state machine, no I/O.
- `crates/denju-local/` — SQLite, filesystem generations, watchers, projections, OS services.
- `crates/denju-client/` — HTTPS/SSE/auth/object-transfer client.
- `crates/denju-registry/` — registry use cases, PostgreSQL, S3, search, outbox.
- `crates/denju-testkit/` — shared deterministic fixtures only.
- `xtask/` — canonical developer/CI commands.
- `Justfile` — thin discoverable aliases only; recipes delegate to Cargo/xtask/Bun and contain no build logic.
- `packages/npm/` — thin binary installer/launcher; never a source-build fallback.
- `docs/` — Astro/ZueDocs site.
- `deploy/dev.compose.yml` — pinned local PostgreSQL + S3-compatible dependencies; Phase 2 makes `cargo xtask dev` own their lifecycle plus the registry process.

Keep dependencies one-way toward `denju-core`; binaries wire product logic rather than owning it. Do not introduce generic utility crates or traits without a real ownership/I/O boundary.

## Verification

Use the narrowest useful check while iterating, then run the scoped full check before handoff:

```bash
just
just check-crate denju-core
just test denju-core
cargo check -p <crate>
cargo test -p <crate>
cargo clippy -p <crate> --all-targets -- -D warnings
cargo xtask check
```

`cargo xtask check` is the canonical repository-wide gate and CI contract. `just` is only the low-friction command menu; never duplicate Rust build/generation/dev logic in the Justfile. Do not add a Makefile as a second command authority. The docs and npm workspaces are intentionally separate from runtime Rust code.

## Safety

- Keep `tmp/gg/` local and untracked.
- Never commit credentials, local databases, object blobs, or live private fixtures.
- Preserve the single Rust implementation path on `main`; no Go or Agentbox fallback.
- Current source and tests are truth if they have advanced beyond the planning baseline. Amend the plan when a load-bearing assumption proves false instead of silently diverging.
