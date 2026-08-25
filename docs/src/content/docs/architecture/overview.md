---
title: Architecture overview
description: A high-level map of the Denju client, local state, registry, PostgreSQL, object storage, and synchronization boundaries.
order: 30
category: Architecture
summary: "Denju separates durable content, mutable desired state, and disposable wake hints so each layer can stay simple and recoverable."
---

You do not need Denju's internals to use it. This section exists for readers who want to understand why the product can synchronize quickly without turning Agent Skills into Git repositories or copying whole directories on every update.

## The system in one diagram

```text
                        registry
                 ┌─────────────────────┐
                 │     denju-server    │
                 │                     │
                 │  PostgreSQL     S3  │
                 │  refs/access   bytes│
                 └─────────┬───────────┘
                           │ HTTPS + SSE hints
                           │ signed object transfers
                           ▼
                 ┌─────────────────────┐
                 │       denju         │
                 │ CLI + background svc│
                 │                     │
                 │ SQLite + local CAS  │
                 └─────────┬───────────┘
                           │ native managed projections
                 ┌─────────┴───────────┐
                 ▼                     ▼
              Codex               Claude Code
```

Three ideas make the architecture easier to reason about.

## 1. Immutable content is separate from mutable intent

Skill file content becomes immutable content-addressed objects and revisions. A published `v7` never changes.

Separately, mutable refs answer questions such as:

- what is this skill's latest release?
- what private revision is this user editing?
- what version does this pack currently resolve to?
- what resources should this installation currently have?

Changing a ref does not require rewriting the underlying unchanged content.

## 2. Clients reconcile current state instead of replaying history

Live notifications are useful for latency, but they are not correctness authority. If a client disconnects for an hour—or a month—it does not need to replay every event that happened in between.

It sends the current roots it knows. The registry returns what differs now. That keeps reconnection cost tied to the current watched state rather than the number of missed events.

## 3. Local filesystem views are rebuildable

The files agents see are managed projections of durable local state. Denju builds remote updates in complete verified generations and switches them into view atomically.

If a process crashes, a notification is missed, or a harness link breaks, Denju repairs the view from SQLite plus immutable content. The visible filesystem is important to the agent, but it is not the only copy of the truth.

## Where to go deeper

- [Merkle content and revisions](/docs/architecture/content-model) — blobs, trees, revisions, snapshots, and deduplication.
- [Synchronization and desired state](/docs/architecture/synchronization) — subscriptions, packs, reconcile, SSE, CAS, and conflicts.
- [Local storage and projections](/docs/architecture/local-projections) — SQLite, generations, Codex/Claude links, watchers, and atomic switching.
- [Registry architecture](/docs/architecture/registry) — PostgreSQL/S3 authority, outbox, authorization, and stateless hosting.
- [Performance and resilience](/docs/architecture/performance) — what makes the hot paths small and how the repository measures them.
