---
title: Registry architecture
description: Why PostgreSQL owns control-plane authority, S3-compatible storage owns immutable bytes, and the server process can remain stateless and replaceable.
order: 34
category: Architecture
summary: "PostgreSQL holds refs, access, and relationships; S3 holds immutable content; the server is a replaceable coordinator around them."
---

The Denju registry is a Rust server with two durable dependencies: PostgreSQL and S3-compatible object storage.

That split follows the shape of the data rather than infrastructure fashion.

## PostgreSQL is the control plane

PostgreSQL owns data that needs transactions, authorization, joins, ordering, and compare-and-swap behavior:

- users, teams, namespaces, sessions, and tokens;
- stable resource identities and locators;
- subscriptions, shares, memberships, assignments, follows, and stars;
- private workspace refs and immutable release metadata;
- pack definitions and exact pack revisions;
- proposals, conflicts, quarantine state, and reports;
- generations and mutable refs;
- idempotent operations;
- durable authority events and the outbox;
- Merkle reachability and quota accounting.

These are small control-plane records whose consistency matters more than raw byte throughput.

## S3-compatible storage is the data plane

File bytes and derived release snapshots belong in object storage.

Clients can upload or download through short-lived signed URLs without forcing all large byte streams through the registry HTTP process. The registry still verifies staged object size and SHA-256 before associating content with authoritative refs.

The storage boundary is provider-neutral. Cloudflare R2 is the official service backend; Garage is the self-host/reference provider.

## The process is disposable

A `denju-server` process may hold useful in-memory acceleration such as:

- active SSE connections;
- a reverse index of which connected clients watch which resources;
- small authentication caches;
- a PostgreSQL `LISTEN` connection;
- bounded opportunistic outbox drain work.

None of those is correctness authority.

If the process disappears, another process rebuilds what it needs from PostgreSQL and object storage. Connected clients reconnect and reconcile current roots.

That lets the same server work on a conventional always-on container host or on a platform that recycles instances and scales to zero.

## Transactional outbox

A mutation that changes authoritative state also writes a durable event/outbox row in the same PostgreSQL transaction.

Derived work—wake hints, follow-latest pack advancement, future indexes—can then be retried without pretending an in-memory task is durable.

If a process commits a publish and dies one millisecond later, the release exists because PostgreSQL committed it. A later process can drain the remaining derived work.

## Authorization before object access

Knowing a resource UUID, revision hash, or canonical object key is not an access capability.

The server authorizes the actor's relationship to the resource before issuing a short-lived signed transfer. PostgreSQL row-level security adds defense in depth around private/team/object boundaries, while application use cases remain the primary business-authorization layer.

## One server path for hosted and self-hosted Denju

The official registry does not use a separate implementation. It runs the ordinary containerized server against managed PostgreSQL and R2.

Self-hosting runs the same server against the reference or managed dependencies of your choice. Deployment-specific configuration stays at the edge; resource, synchronization, and authorization semantics stay in the shared Rust implementation.
