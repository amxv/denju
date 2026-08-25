---
title: Self-hosting quickstart
description: Bring up a complete private Denju registry with Docker Compose, PostgreSQL, Garage object storage, and the production server container.
order: 20
category: Self-host
summary: "Start the reference stack, verify it, then bind Denju clients to your own registry origin."
---

Self-hosted Denju runs the **same `denju-server` product** as the official registry. You need two durable services behind it:

1. PostgreSQL for identities, resources, relationships, current refs, and registry state.
2. S3-compatible object storage for immutable skill content and release snapshots.

The reference Docker Compose stack includes PostgreSQL 18 and Garage, a lightweight S3-compatible service.

## Requirements

- Docker with Compose support.
- A Denju repository checkout, or at minimum the `deploy/` files from the release/source tree.
- For a registry used by remote clients: an HTTPS hostname for the registry and an HTTPS object-store origin reachable by those clients.

For a same-machine trial, the bundled loopback defaults work without TLS.

## 1. Create the environment file

From the repository root:

```bash
cp deploy/self-host.env.example deploy/self-host.env
```

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

## 2. Start the stack

```bash
docker compose \
  --env-file deploy/self-host.env \
  -f deploy/compose.yml \
  up -d
```

The Compose stack starts PostgreSQL and Garage, runs database migrations once, then starts the ordinary Denju server.

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

Next: [Configuration](/docs/self-host/configuration) covers managed PostgreSQL/S3 and every important server setting. [Operations](/docs/self-host/operations) covers migrations, upgrades, health, recovery, and operator quarantine.
