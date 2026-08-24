---
title: Self-hosting
description: Run the same Denju registry process with PostgreSQL and S3-compatible object storage.
order: 4
category: Development
summary: The reference Compose stack uses the production server container, PostgreSQL 18, and Garage.
---

## One server process

Self-hosting runs the same `denju-server` binary and Dockerfile used by the hosted registry. There
is no self-host-specific Rust handler, queue, merge engine, or storage behavior. PostgreSQL/current
refs remain authority; S3-compatible object storage holds immutable content; SSE and PostgreSQL
notifications are disposable wake hints.

The reference stack lives at `deploy/compose.yml` and contains PostgreSQL 18.6, Garage 2.3.0, an
explicit one-shot migration service, and the ordinary server container. Copy
`deploy/self-host.env.example` to your own secret environment file, fill every required secret,
then start the stack from the repository root:

```bash
docker compose --env-file deploy/self-host.env -f deploy/compose.yml up -d
```

The database owner credential is used only by the migration container. Runtime request SQL uses
the restricted `denju_app` login; background/recovery work uses `denju_worker`. The reference
Garage instance also requires its own RPC secret and S3 access key pair.

The bundled Compose file sets `DENJU_S3_ALLOW_HTTP=true` only for the `garage` hostname on its
private Docker network. It publishes Garage's S3 port on host loopback and separately sets
`DENJU_S3_PRESIGN_ENDPOINT=http://127.0.0.1:3900`, so a Denju client running on that same host can
use the signed upload/download URLs even though the server itself talks to `http://garage:3900`.
If you change `DENJU_S3_PORT`, change the loopback presign endpoint to the same port. External
object-store endpoints stay HTTPS-by-default; opt into internal HTTP only for an equivalently
isolated network you control.

## Use external PostgreSQL or object storage

The container contract is deployment-neutral. Instead of the reference dependencies, provide:

- `DENJU_DATABASE_URL` for pooled/request SQL as `denju_app`.
- `DENJU_DATABASE_WORKER_URL` for worker SQL as `denju_worker`.
- `DENJU_DATABASE_DIRECT_URL` for the session connection used by LISTEN/NOTIFY.
- `DENJU_DATABASE_MIGRATION_URL` only when running `denju-server migrate`.
- `DENJU_S3_ENDPOINT`, bucket, region, access key ID, secret access key, and path-style setting for
  an S3-compatible provider. `DENJU_S3_ENDPOINT` is the registry process's own SDK endpoint.
- `DENJU_S3_PRESIGN_ENDPOINT` when clients reach the object store through a different origin than
  the registry process. Denju signs product transfer URLs for this origin, so it must be reachable
  by every client. Remote clients require HTTPS; the bundled `127.0.0.1` value is only for clients
  running on the Compose host.
- `DENJU_S3_ALLOW_HTTP=true` only when an intentionally private self-host network terminates no TLS
  between Denju and that object store.

Run migrations as a controlled deployment step before starting upgraded runtime instances:

```bash
denju-server migrate
denju-server serve
```

`/health/live`, `/health/ready`, and `/health/metrics` are the process/operator health surfaces.
The recovery bearer can call `/v1/internal/recover` to idempotently drain bounded outbox and
follow-latest pack work after recycle or downtime.

## Object-store requirement

Before treating a provider as production-ready, run the ordinary provider conformance probe:

```bash
denju-server check-object-store
```

The probe exercises SDK writes/reads, immutable canonical retry, internal presigned PUT/GET, and
deletion through the same adapter used by the service. The release self-host smoke additionally
subscribes from the host through a client-facing presigned URL, which verifies that the externally
reachable transfer origin is wired correctly. Garage and Cloudflare R2 are intentionally expected
to satisfy one domain contract rather than separate provider-specific behavior.
