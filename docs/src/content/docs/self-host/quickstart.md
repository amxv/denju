---
title: Self-hosting quickstart
description: Run a private Denju registry from the published server image, with either the complete reference Compose stack or your own PostgreSQL and S3 services.
order: 20
category: Self-host
summary: "Pull the same server image used by the official registry, bring up durable storage, and point Denju clients at your own origin."
---

Every Denju release publishes the production server as a multi-architecture container image:

```text
ghcr.io/amxv/denju-server:vX.Y.Z
ghcr.io/amxv/denju-server:latest
```

That is the same `denju-server` product used by the official registry. **You do not need a Rust toolchain or a local server build to self-host Denju.**

The server needs two durable services behind it:

1. PostgreSQL for identities, resources, relationships, current refs, and registry state.
2. S3-compatible object storage for immutable skill content and release snapshots.

There are two normal deployment paths:

- **Complete reference stack:** use `deploy/compose.yml` to run the published Denju image with PostgreSQL 18 and Garage. This is the easiest way to try or operate a small standalone registry.
- **Bring your own infrastructure:** run the published image directly on your container platform and point it at managed PostgreSQL and S3-compatible storage. See [Operations](/docs/self-host/operations#run-the-published-image-directly).

## Requirements for the reference stack

- Docker with Compose support.
- The `deploy/` directory from a Denju source checkout.
- For remote clients: an HTTPS hostname for the registry and an HTTPS object-store origin reachable by those clients.

For a same-machine trial, the bundled loopback defaults work without TLS.

## 1. Create the environment file

From the repository root:

```bash
cp deploy/self-host.env.example deploy/self-host.env
```

The example uses `ghcr.io/amxv/denju-server:latest`, which is convenient for a trial. For a production deployment, change `DENJU_SERVER_IMAGE` to an exact release such as `ghcr.io/amxv/denju-server:vX.Y.Z` so upgrades happen only when you choose them.

Generate strong values for every blank secret. This snippet prints compatible values you can paste into the file:

```bash
printf 'DENJU_DB_OWNER_PASSWORD=%s\n' "$(openssl rand -hex 24)"
printf 'DENJU_DB_APP_PASSWORD=%s\n' "$(openssl rand -hex 24)"
printf 'DENJU_DB_WORKER_PASSWORD=%s\n' "$(openssl rand -hex 24)"
printf 'DENJU_GARAGE_RPC_SECRET=%s\n' "$(openssl rand -hex 32)"
printf 'DENJU_S3_ACCESS_KEY_ID=GK%s\n' "$(openssl rand -hex 10)"
printf 'DENJU_S3_SECRET_ACCESS_KEY=%s\n' "$(openssl rand -hex 32)"
printf 'DENJU_RECOVERY_TOKEN=%s\n' "$(openssl rand -hex 32)"
```

Protect the finished file because it contains registry credentials:

```bash
chmod 600 deploy/self-host.env
```

For a same-host trial, keep the example values for:

```text
DENJU_PUBLIC_URL=http://127.0.0.1:7788
DENJU_PORT=7788
DENJU_S3_PORT=3900
DENJU_S3_PRESIGN_ENDPOINT=http://127.0.0.1:3900
```

## 2. Pull and start the stack

```bash
docker compose \
  --env-file deploy/self-host.env \
  -f deploy/compose.yml \
  pull

docker compose \
  --env-file deploy/self-host.env \
  -f deploy/compose.yml \
  up -d
```

The Compose stack pulls the published Denju server image, starts PostgreSQL and Garage, runs database migrations once with the same image, then starts the registry. It does not compile Denju from source.

Check the services:

```bash
docker compose \
  --env-file deploy/self-host.env \
  -f deploy/compose.yml \
  ps
```

## 3. Verify the registry

The reference local registry exposes:

```text
http://127.0.0.1:7788/health/live
http://127.0.0.1:7788/health/ready
http://127.0.0.1:7788/health/metrics
```

Verify readiness:

```bash
curl -fsS http://127.0.0.1:7788/health/ready
```

The server also includes an object-store conformance check. Run it inside the server container so it uses the same configured provider boundary:

```bash
docker compose \
  --env-file deploy/self-host.env \
  -f deploy/compose.yml \
  exec server denju-server check-object-store
```

## 4. Connect a Denju client

On the same host:

```bash
denju setup --registry http://127.0.0.1:7788
```

A production client should instead use your HTTPS registry origin:

```bash
denju setup --registry https://denju.example.com
```

A Denju installation is bound to one registry in v1, so choose the intended registry when setting up each machine.

## 5. Put TLS in front for remote users

The reference Compose file binds the registry and Garage to loopback. That is deliberate. For remote access, put your normal reverse proxy or load balancer in front of them.

A typical deployment exposes:

```text
https://denju.example.com     -> Denju server
https://objects.example.com   -> Garage S3 endpoint
```

Then set:

```text
DENJU_PUBLIC_URL=https://denju.example.com
DENJU_S3_PRESIGN_ENDPOINT=https://objects.example.com
```

`DENJU_S3_PRESIGN_ENDPOINT` matters because Denju gives clients short-lived signed upload/download URLs. That origin must be reachable by the clients, not only by the server container.

## Upgrade the reference stack

For a production registry, change `DENJU_SERVER_IMAGE` to the next exact release, then pull and recreate the migration/server services:

```bash
docker compose \
  --env-file deploy/self-host.env \
  -f deploy/compose.yml \
  pull migrate server
docker compose \
  --env-file deploy/self-host.env \
  -f deploy/compose.yml \
  up -d
```

The one-shot migration service must succeed before the new server becomes ready. Keeping an exact image tag makes rollback and change control straightforward.

Next: [Configuration](/docs/self-host/configuration) covers managed PostgreSQL/S3 and every important server setting. [Operations](/docs/self-host/operations) covers running the image directly, migrations, health, recovery, and operator quarantine.
