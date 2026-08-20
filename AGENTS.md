# AGENTS.md

Denju is a greenfield Rust implementation. The former Go/Agentbox product is preserved on `legacy/go-agentbox-v0.2.0`; do not recreate or preserve its architecture on `main`.

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
- `packages/npm/` — thin binary installer/launcher; never a source-build fallback.
- `docs/` — Astro/ZueDocs site.

Keep dependencies one-way toward `denju-core`; binaries wire product logic rather than owning it. Do not introduce generic utility crates or traits without a real ownership/I/O boundary.

## Verification

Use the narrowest useful check while iterating, then run the scoped full check before handoff:

```bash
cargo check -p <crate>
cargo test -p <crate>
cargo clippy -p <crate> --all-targets -- -D warnings
cargo xtask check
```

`cargo xtask check` is the canonical repository-wide gate. It runs Rust format/lint/tests plus npm/docs checks. The docs and npm workspaces are intentionally separate from runtime Rust code.

## Safety

- Keep `tmp/gg/` local and untracked.
- Never commit credentials, local databases, object blobs, or live private fixtures.
- Preserve the single Rust implementation path on `main`; no Go or Agentbox fallback.
- Current source and tests are truth if they have advanced beyond the planning baseline. Amend the plan when a load-bearing assumption proves false instead of silently diverging.
