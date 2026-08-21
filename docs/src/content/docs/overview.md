---
title: Overview
description: Understand Denju's current product boundary, local model, and registry architecture before changing implementation details.
order: 1
category: Start
summary: "The product model: one native CLI, durable local state, and one registry boundary."
---

Denju is an agent-native, CLI-only registry and synchronization system for Agent Skills.

## What exists today

The Rust implementation provides deterministic Agent Skills validation and content identity, anonymous `denju setup`, public skill discovery, account-wide direct subscriptions, verified cold installs from S3-compatible storage, optional password/recovery identity claim and login, authenticated private skill import, automatic and explicit forks with immutable provenance, private sharing and live private subscriptions, private moving proposals that accept into upstream private history without publishing, editable owned workspaces with durable private revisions, deterministic multi-device merges and explicit conflict resolution, stable-ID rename/unpublish/delete/deprecation and explicit history pruning, revocable devices and scoped automation credentials, durable SQLite-backed local state, Codex and Claude projections, per-user service management, and the PostgreSQL-backed registry foundation used by the CLI.

The repository also contains a thin npm installer for the native binary and this documentation site. Product behavior lives in Rust; JavaScript is not a second implementation path.

## Local model

Each Denju installation owns one local state database and one canonical managed skills tree. Harness-facing paths are projections of that managed state rather than independent copies. The CLI remains correct when the background service is stopped because durable local state, not local IPC, is authoritative.

## Registry model

The registry is a separate Rust server backed by PostgreSQL and required S3-compatible object storage. The official service uses Neon PostgreSQL and Cloudflare R2, while local development uses the same interfaces with PostgreSQL and Garage.

Mutable references and registry metadata live in PostgreSQL. Skill bytes are content-addressed objects. This keeps synchronization and recovery based on durable state rather than process memory or filesystem events.

Public reads and direct subscriptions are cold-start safe: restarting the registry process does not change discovery or installation results because PostgreSQL, object storage, and durable installation/account state are authoritative. Anonymous subscriptions are adopted by the account when that installation claims or logs into an identity; another authenticated device then reconciles the same desired state.

Private import uses the same object model. A namespace must prove staged bytes before it can reference them even when another tenant already caused identical physical content to exist. Logical quota accounting therefore remains namespace-specific while the object store can deduplicate verified bytes physically.

Owned workspaces remain writable after import. Valid coherent saves are recorded locally before network execution and advance the private workspace using generation-and-parent compare-and-swap. Offline/quota-blocked revisions remain queued; invalid content remains local and paused. Concurrent stale writers preserve both immutable heads rather than using last-write-wins; deterministic client-side three-way merge creates two-parent revisions when edits are compatible and explicit conflict state preserves overlapping edits until resolution.
