---
title: Self-host configuration
description: Configure the Denju server for its public origin, PostgreSQL roles, S3-compatible storage, client-facing presigned URLs, limits, and recovery.
order: 21
category: Self-host
summary: "The server is deployment-neutral: provide one public origin, PostgreSQL authority, and an S3-compatible object store."
---

The Denju registry is one containerized HTTP service with external durable state. Each release publishes a multi-architecture `ghcr.io/amxv/denju-server:vX.Y.Z` image, so self-hosting does not require compiling the Rust server yourself. You can run the reference Compose dependencies or replace either dependency with managed infrastructure.

## Public registry origin

```text
DENJU_PUBLIC_URL=https://denju.example.com
```

This is the registry origin Denju clients are configured against. Non-loopback production origins must use HTTPS.

The server reads `PORT` on container platforms that provide it. Outside those platforms, `DENJU_BIND` can set an explicit socket address.

## PostgreSQL connections

Denju separates database authority by job:

```text
DENJU_DATABASE_URL
DENJU_DATABASE_WORKER_URL
DENJU_DATABASE_DIRECT_URL
DENJU_DATABASE_MIGRATION_URL
```

### Request SQL

`DENJU_DATABASE_URL` is the ordinary request connection. It should authenticate as the restricted `denju_app` role.

### Recovery/background SQL

`DENJU_DATABASE_WORKER_URL` is used by durable recovery/background work and should authenticate as the restricted `denju_worker` role.

### Session SQL

`DENJU_DATABASE_DIRECT_URL` is the session-capable `denju_app` connection used for PostgreSQL `LISTEN/NOTIFY` wakeups. If your managed PostgreSQL provider offers transaction-pooled and direct/session endpoints, use the direct endpoint here.

### Migration authority

`DENJU_DATABASE_MIGRATION_URL` is privileged and should be supplied only to controlled migration or operator-bootstrap commands. It is intentionally not part of normal server runtime authority.

## S3-compatible object storage

Required settings:

```text
DENJU_S3_ENDPOINT
DENJU_S3_BUCKET
DENJU_S3_REGION
DENJU_S3_ACCESS_KEY_ID
DENJU_S3_SECRET_ACCESS_KEY
DENJU_S3_FORCE_PATH_STYLE
```

Denju uses one generic S3-compatible data plane. The official service uses Cloudflare R2; the reference self-host stack uses Garage.

### Internal versus client-facing endpoints

`DENJU_S3_ENDPOINT` is the endpoint the registry process itself uses.

`DENJU_S3_PRESIGN_ENDPOINT` is optional when clients reach the same endpoint. Set it when the registry uses an internal/private object-store address but clients need a different public origin.

Example:

```text
DENJU_S3_ENDPOINT=http://garage:3900
DENJU_S3_PRESIGN_ENDPOINT=https://objects.example.com
```

The second origin is the one placed into signed client transfer URLs. Remote clients therefore need it to be routable and HTTPS.

The bundled same-host Compose configuration uses `http://127.0.0.1:3900` as the presign endpoint because both the test client and the exposed Garage port are on the same machine.

### HTTP object storage

Production object-store origins should use HTTPS. `DENJU_S3_ALLOW_HTTP=true` exists for intentionally private deployment networks such as the bundled Docker network. Denju fails closed rather than silently accepting arbitrary remote plain HTTP storage endpoints.

## Recovery token

```text
DENJU_RECOVERY_TOKEN=<high-entropy-secret>
```

The recovery endpoint performs idempotent bounded draining of durable outbox and pack-release work after downtime or process recycling. It uses a credential separate from ordinary Denju users, sessions, automation tokens, and operator credentials.

## Storage and transfer limits

Registries advertise their active limits to clients. Per-object, per-release, namespace storage, and transfer limits are deployment policy rather than hard-coded client constants.

That means a self-hosted registry can choose different capacity policy without rebuilding `denju`.

## Use managed PostgreSQL and S3

You do not need the bundled PostgreSQL or Garage containers. A common production topology is:

```text
Denju server container
  ├── managed PostgreSQL
  └── managed S3-compatible object storage
```

Keep the same environment contract, run migrations before upgraded application instances, and verify the storage provider with:

```bash
denju-server check-object-store
```

The provider must satisfy Denju's actual presign/read/write/delete behavior. Do not weaken content verification to accommodate an incompatible service.
