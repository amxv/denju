---
title: Repository layout
description: Ownership boundaries for the Rust workspace and distribution surfaces.
order: 3
category: Reference
summary: Where product logic, runtime wiring, tooling, installers, and documentation belong.
---

## Product binaries

- `apps/denju` — CLI and background-daemon process wiring.
- `apps/denju-server` — registry HTTP, migration, and operator process wiring.

Binary crates stay thin. Reusable product behavior belongs in the owning library crate rather than command or HTTP handlers.

## Product crates

- `crates/denju-core` — pure IDs, paths, Agent Skills validation, content identity, and merge primitives.
- `crates/denju-wire` — versioned API and CLI machine contracts.
- `crates/denju-sync` — deterministic desired-state and reconciliation logic.
- `crates/denju-local` — SQLite, filesystem materialization, projections, services, and credential adapters.
- `crates/denju-client` — registry HTTP/SSE and object-transfer execution.
- `crates/denju-registry` — PostgreSQL/S3 registry use cases and migrations.
- `crates/denju-testkit` — shared fixtures for tests, never a production dependency.

## Tooling and distribution

- `xtask/` — canonical developer, CI, generation, and local-environment command surface.
- `Justfile` — thin command aliases over Cargo, xtask, and Bun.
- `deploy/` — local and production server deployment configuration.
- `packages/npm/` — thin native-binary installer and launcher only.
- `docs/` — Astro/ZueDocs documentation site and docs-only Vercel build guard.
- `tmp/gg/` — ignored local planning and agent handoff artifacts.

## Documentation ownership

Denju keeps Markdown, product copy, site metadata, and deployment configuration locally under `docs/`. Shared layouts, navigation behavior, page actions, search, themes, and reading styles come from the `zuedocs` package. Do not copy shared ZueDocs components into this repository to make page-specific changes.
