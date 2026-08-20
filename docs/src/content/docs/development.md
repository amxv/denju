---
title: Development
description: Use the canonical repository commands to verify Denju, run the local registry stack, and exercise setup without touching your real agent configuration.
order: 2
category: Development
summary: The commands and isolation rules for safe, repeatable Denju development.
---

## Verify the repository

From the repository root, use the Rust-native command surface:

```bash
cargo xtask check
cargo build --workspace
```

Rust is the primary project. The root Bun workspace exists only for the documentation site and the published npm installer shim.

`cargo xtask check` is the comprehensive handoff gate. `just` is a discoverable alias layer; it does not own build, migration, or environment logic.

## Run the local registry

`cargo xtask dev` owns the local dependency and registry lifecycle. It starts the pinned PostgreSQL 18.6 and Garage 2.3.0 services, applies registry migrations, and runs the registry at `http://127.0.0.1:7788`. Re-running it while the registry is already live is safe.

```bash
cargo xtask dev
```

## Exercise setup and public subscriptions safely

For setup development, use an isolated home and the explicit local registry rather than your real harness roots:

```bash
TEST_HOME="$(mktemp -d)"
HOME="$TEST_HOME" \
  CODEX_HOME=.codex \
  CLAUDE_CONFIG_DIR=.claude \
  DENJU_TEST_FILE_CREDENTIALS=1 \
  DENJU_TEST_SERVICE_INSTALL_ONLY=1 \
  cargo run -p denju -- setup --registry http://127.0.0.1:7788

HOME="$TEST_HOME" CODEX_HOME=.codex CLAUDE_CONFIG_DIR=.claude \
  cargo run -p denju -- search review
```

Phase-scoped integration fixtures may seed public releases through the hidden `denju-server seed-public` development command. That command writes the same PostgreSQL/S3 release model read by the public HTTP API; it is not a second in-memory catalog or a user-facing publishing path.

The hidden provider-conformance probe exercises the exact generic object-store adapter used by the registry. With the normal `cargo xtask dev` environment values exported, run:

```bash
cargo run -p denju-server -- check-object-store
```

The probe covers a presigned staging PUT, verified reads, canonical write/retry, presigned GET, and idempotent deletion. Garage is the deterministic local/reference provider. The same probe is used against R2 when deployment credentials are available; provider-specific product behavior must not be added to make one backend pass.

Owned-workspace tests deliberately cover both the fast watcher path and the authority fallback. Native filesystem notifications only wake bounded/coalesced work; SQLite plus a complete managed-tree scan remains the recovery path after overflow, missed events, daemon restart, or polling fallback. Collision-derived projection writeback has its own journal and must pass interruption recovery at every pre-complete state.

## Work on the docs

The docs app is isolated under `docs/` and consumes the shared ZueDocs package. Run Astro validation serially:

```bash
bun run docs:check
bun run docs:build
bun run docs:dev
```

Only changes under `docs/` trigger the Denju documentation project on Vercel. Keep product documentation and metadata inside that directory so deployment scope remains predictable.
