---
title: Synchronization and desired state
description: How Denju resolves subscriptions, packs, team policy, wake hints, compare-and-swap updates, offline reconciliation, and concurrent edits.
order: 32
category: Architecture
summary: "Denju synchronizes the current desired refs, not filesystem events or missed event history."
---

Denju is not a filesystem event replication protocol. Filesystem events are noisy, clients go offline, processes die, and network notifications get dropped.

The synchronization model is instead: **derive the current desired state, compare it with what this installation has, and apply the difference**.

## Desired-state sources

A skill can be required by several relationships:

- a direct subscription;
- a subscribed pack;
- a personal owned workspace;
- a team pack;
- an enforced team assignment;
- a retained deleted direct subscription.

The client reduces those sources to one resolved desired result for each stable skill resource.

A stronger team policy can suppress a weaker personal source without deleting it. When the team policy disappears, the personal source can become active again.

Equal-authority incompatible requirements become an explicit desired-state conflict instead of last-write-wins.

## Reconcile roots, not every event

The client stores the resource IDs, generations, and heads it currently knows. Reconciliation sends those known roots to the registry.

Conceptually:

```text
client knows:
  skill A generation 12 / revision X
  pack  B generation  8 / version 5
  team  C generation  3

registry replies:
  skill A unchanged
  pack  B -> generation 9 / version 6
  team  C unchanged
```

If the client was offline while 500 unrelated registry events occurred, that does not create 500 replay steps. It asks what changed in the roots it actually watches.

## SSE is only a wake hint

When online, the registry can send a compact Server-Sent Events hint saying a resource generation changed.

The hint contains no authoritative content and can be coalesced or lost. Its purpose is simply to make the client reconcile sooner.

If a connection drops, a server instance is replaced, or the dirty-set overflows, the client reconnects and reads current refs again.

That is why Denju can run correctly on hosts that recycle processes or scale to zero.

## Publishing follows compare-and-swap

Mutable refs advance only when the caller still has the parent/generation it expected.

That avoids the classic race:

```text
A and B both start from revision X
A publishes/saves revision Y
B later tries to advance X -> Z
```

The registry does not silently accept Z over Y. It preserves the divergent immutable head and makes reconciliation explicit.

## Merge work stays on the client

When two private heads share a common parent, the client performs deterministic three-way merge.

Clean changes can create a two-parent revision automatically. Real overlapping edits become a conflict for that resource, preserving both heads and local working bytes.

The server remains much simpler: authenticate the actor, verify immutable objects, enforce generations/refs, and persist the result. It does not need to run arbitrary content merges inside registry transactions.

## Packs use durable release events

A follow-latest pack should advance when a member skill publishes a new immutable release. But a popular skill may appear in many packs, so publishing cannot synchronously rewrite every dependent pack inside one request.

Denju records the semantic release event durably and advances dependent packs in bounded idempotent work. If the process stops halfway through, a later request or recovery drain resumes from durable state.

Importantly, releases are not coalesced away: if a skill publishes `v7`, `v8`, and `v9`, a follow-latest pack preserves the corresponding ordered immutable pack history instead of jumping directly to a single final snapshot.
