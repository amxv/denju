---
title: Self-host operations
description: Run migrations and upgrades safely, check health and object storage, recover durable work, and use the server-side operator quarantine surface.
order: 22
category: Self-host
summary: "Operate Denju as a stateless application process around durable PostgreSQL and S3 authority."
---

Denju is designed so the application process can be restarted, replaced, or scaled without becoming the durable source of truth. PostgreSQL, object storage, and current refs remain authoritative.

## Apply migrations explicitly

Before starting an upgraded runtime against an older schema:

```bash
DENJU_DATABASE_MIGRATION_URL='postgresql://...' denju-server migrate
```

The normal server validates the schema but does not silently acquire migration-owner authority at startup.

The reference Compose stack handles this with a one-shot `migrate` service before `server` becomes ready.

## Start the server

With the runtime environment configured:

```bash
denju-server serve
```

Running `denju-server` with no subcommand also starts the server.

## Health and metrics

Use three HTTP surfaces:

```text
/health/live     process is alive
/health/ready    registry dependencies and schema are ready
/health/metrics  bounded operational counters and gauges
```

Metrics cover request latency, server errors, SSE connections, dirty-set overflow, database/object-store failures, transfer bytes, reconciliation work, outbox lag, and wake-listener state. Request bodies, bearer tokens, presigned URLs, passwords, recovery secrets, and private skill content are not metric payloads.

## Verify object storage

After changing provider configuration or credentials:

```bash
denju-server check-object-store
```

The probe exercises the same generic storage adapter used by the product. The reference release smoke also verifies that a real client can use the configured client-facing signed download origin.

## Recover durable work

Most requests perform small bounded recovery work automatically. If a process dies while durable outbox or follow-latest pack work remains, invoke the protected recovery endpoint with the configured recovery bearer.

The operation is idempotent. It is safe for a scheduler to invoke after downtime, but correctness does not depend on an immortal background worker or a permanent SSE connection.

For pack release fanout specifically, operators can also run:

```bash
denju-server drain-packs --limit 256
```

## Bootstrap an operator

Operator authority is deliberately separate from normal client identity.

With the migration-owner database URL available only for this command:

```bash
DENJU_DATABASE_MIGRATION_URL='postgresql://...' \
  denju-server admin bootstrap --name primary
```

The operator token is shown once. Store it securely.

For normal operator actions, configure that bearer as `DENJU_OPERATOR_TOKEN` together with the restricted runtime database/object-store settings.

Review reports:

```bash
denju-server admin reports
```

Quarantine a whole resource:

```bash
denju-server admin quarantine @owner/skill --reason malicious
```

Quarantine one exact release:

```bash
denju-server admin quarantine @owner/skill --version 7 --reason malicious
```

Lift quarantine:

```bash
denju-server admin unquarantine @owner/skill
denju-server admin unquarantine @owner/skill --version 7
```

Revoke an operator credential with migration authority:

```bash
DENJU_DATABASE_MIGRATION_URL='postgresql://...' \
  denju-server admin revoke <operator-id>
```

## Run the published image directly

Every Denju release publishes the same multi-architecture server image used by the official registry:

```text
ghcr.io/amxv/denju-server:vX.Y.Z
```

For production, prefer an exact release tag rather than `latest`. If PostgreSQL and S3-compatible storage are already provisioned, no Compose stack or source checkout is required. Pull the image, apply migrations with migration-owner authority, then start the ordinary runtime without that privileged database credential:

```bash
IMAGE=ghcr.io/amxv/denju-server:vX.Y.Z
docker pull "$IMAGE"

docker run --rm \
  -e DENJU_DATABASE_MIGRATION_URL='postgresql://...' \
  "$IMAGE" migrate

docker run -d --name denju-server --restart unless-stopped \
  --env-file denju-server.env \
  -e PORT=80 \
  -p 127.0.0.1:7788:80 \
  "$IMAGE" serve
```

`denju-server.env` contains the restricted runtime settings described in [Configuration](/docs/self-host/configuration): public origin, app/worker/direct PostgreSQL URLs, S3 settings, recovery token, and any deployment limits. Do not put `DENJU_DATABASE_MIGRATION_URL` in the long-lived runtime environment.

On Kubernetes, Nomad, ECS, Fly.io, Vercel, or another container platform, use the same image and environment contract. The platform does not change Denju's registry semantics.

## Deploy on a container platform

Denju does not require a provider-specific runtime implementation. The official service runs the published container on Vercel; the same image can run anywhere that can provide an HTTP container plus PostgreSQL and S3-compatible storage.

For a new deployment:

1. provision PostgreSQL and object storage;
2. create restricted runtime roles/credentials;
3. apply migrations with the migration-owner URL;
4. run `denju-server check-object-store` against the production storage config;
5. deploy the container with the runtime environment;
6. verify readiness and a real client setup/subscription flow;
7. configure a recovery invocation appropriate for the platform's process lifecycle.

Scale-to-zero and process recycling are expected conditions, not exceptional recovery modes.
