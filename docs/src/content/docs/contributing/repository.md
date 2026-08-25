---
title: Repository architecture
description: A task-oriented map of Denju's Rust workspace, deployment surfaces, specifications, tests, and the ownership boundary each directory is expected to keep.
order: 51
category: Contributing
summary: "Pure domain logic stays in core, I/O lives at explicit edges, binaries stay thin, and xtask owns repository automation."
---

Denju is one Rust product with a deliberately small number of ownership boundaries.

```text
apps/
  denju/             CLI + daemon process wiring
  denju-server/      registry process wiring

crates/
  denju-core/        pure IDs, paths, Merkle content, revisions, merges
  denju-wire/        versioned CLI/API/SSE data contracts
  denju-sync/        deterministic desired-state reconciliation
  denju-local/       SQLite, filesystem, projections, services, credentials
  denju-client/      registry HTTP/SSE + signed object transfers
  denju-registry/    PostgreSQL/S3 registry use cases and migrations
  denju-testkit/     shared test fixtures only

xtask/               canonical repository automation
spec/                checked wire/object/conformance contracts
tests/               e2e/load fixtures and reports
fuzz/                untrusted-input fuzz/property surfaces
deploy/              Compose and hosted-container configuration
packages/npm/        thin native-binary npm installer/launcher
docs/                Astro/ZueDocs documentation site
```

## `denju-core` stays environmental-I/O free

This crate owns semantic rules that should be deterministic from input alone: stable identifiers, resource locators, portable paths, Agent Skills validation, Merkle identities, revision ancestry, and merge primitives.

It should not know about Tokio, PostgreSQL, SQLite, HTTP, process environment, or the local filesystem.

## `denju-sync` decides; edge crates execute

Desired-state reconciliation is modeled so the important decision can be tested independently from network or filesystem timing.

`denju-local` executes local SQLite/filesystem/service effects. `denju-client` executes registry HTTP/SSE/object transfers. This keeps the synchronization model from becoming an implicit pile of callbacks spread across I/O code.

## `denju-registry` owns hosted authority

Registry use cases, PostgreSQL queries/migrations, S3 association, authorization, packs, teams, social relationships, outbox, quarantine, and search belong here.

`apps/denju-server` should remain HTTP/process wiring rather than accumulating duplicate product rules in handlers.

## Binaries stay thin

`apps/denju` owns command parsing, concise text/JSON presentation, and construction of the local/client/sync pieces. The reusable logic belongs in the owning crate.

The same rule applies to `denju-server`: wire the process, configuration, transport, health, migration, and operator entry points; keep domain behavior in the registry crate.

## `xtask` is the automation API

Build checks, contract generation, local dependency startup, performance harnesses, release manifests/smokes, self-host smoke, and deployment-context generation belong under `cargo xtask`.

`just` may expose shortcuts but should delegate rather than duplicate logic. There is intentionally no Makefile as a second command authority.

## Specifications and generated contracts

`spec/` contains stable checked artifacts such as semantic object contracts, fixtures, and the generated/checked wire description. Changes should originate from the actual owning Rust source and be regenerated through the canonical xtask rather than hand-maintained independently.

For product behavior and the user-facing mental model, prefer these documentation pages over implementation plans or historical phase notes. The source code and current tests remain the final implementation truth.
