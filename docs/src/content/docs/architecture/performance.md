---
title: Performance and resilience
description: The design choices that keep Denju's local and registry hot paths small, plus the repository harness used to measure stateless failure recovery.
order: 35
category: Architecture
summary: "Denju avoids whole-tree transfers, event replay, subscriber fanout writes, and partial local updates; the load harness measures those assumptions directly."
---

Denju's performance goals follow from the product experience it wants:

- local CLI state should feel immediate;
- registry search/show should feel like normal package metadata lookup;
- online releases should reach subscribers quickly;
- coming back from a long offline period should not get slower because many irrelevant events happened.

The architecture removes work from the hot path before adding caches.

## Change one file, transfer one file

Merkle content lets unchanged blobs and subtrees keep their identities. A private save does not require serializing and uploading a brand-new copy of every file in the skill.

For a cold install, deterministic compressed snapshots avoid turning a large skill into hundreds of tiny HTTP transfers. The snapshot is then verified back against the same Merkle identity.

## Reconcile the present, not the past

Offline cost is based on the watched roots and missing current state, not event-log length.

A client can miss thousands of wake hints and still converge by comparing current generations and heads.

## Do not write one row per subscriber

Publishing a popular resource should not create durable update rows for every subscriber.

The resource ref changes once. Connected clients receive disposable wake hints. Disconnected clients discover the new ref on reconciliation.

Similarly, follow-latest pack fanout is durable bounded work outside the original skill-publish transaction rather than one unbounded synchronous transaction.

## Keep heavy reconciliation on clients

The registry does not perform arbitrary content merging for every race. Clients already have local content and CPU; deterministic three-way merge belongs there.

The server stays focused on transactional authority, verified object association, access, and ref updates.

## Build before switching

Atomic local generations trade a little temporary disk space for a much better correctness/performance boundary. Agents keep reading the old complete generation while the new one downloads and verifies, then see one fast logical switch.

## Measured in the repository

The repository includes an explicit release-mode load/stateless harness:

```bash
cargo xtask load
```

It exercises:

- local `status` and unified search latency;
- registry search/show latency;
- cold starts, concurrency, and horizontal instances;
- large subscriber and dependent-pack shapes;
- long-disconnected reconciliation;
- SSE disconnect/reconnect and PostgreSQL listener loss;
- process termination and scale-to-zero recovery;
- object-store interruption/restart;
- local daemon memory and watcher behavior;
- query plans for representative registry paths.

The checked acceptance report lives under `tests/load/reports/`.

For correctness-oriented repository verification rather than performance measurement, use:

```bash
cargo xtask check
cargo xtask fuzz
```

Performance optimization is expected to preserve the same content, wire, authorization, and desired-state semantics rather than introducing a faster second path with weaker guarantees.
