---
title: Hosted registry operations
description: Deployment contract for the official Vercel registry, Neon PostgreSQL, and Cloudflare R2.
order: 5
category: Development
summary: The official service is the ordinary Denju server container running statelessly in Singapore.
---

## Official runtime shape

The official origin is `https://registry.denju.ashray.xyz`. The registry deploys the repository's
ordinary `Dockerfile.vercel` with Vercel's **Container** framework preset in `sin1`; the server
reads Vercel's `PORT` and does not depend on writable local disk or process memory.
`deploy/vercel.registry.json` carries the registry-specific, non-secret region/cron configuration
without replacing the root docs project's Astro configuration.

The production custom domain is already attached to the dedicated `denju-registry` Vercel project.
Routine releases replace the production deployment behind that domain; they do not require a DNS
cutover. DNS only needs to change if the registry moves to a different hosting boundary.

Because the docs and registry are separate Vercel projects rooted at the same repository,
deploying the registry directly from the repository root would pick up the docs `vercel.json`.
Stage an exact registry deployment context through the canonical xtask first:

```bash
CONTEXT="$(mktemp -d)"
rm -rf "$CONTEXT"
cargo xtask vercel-context --out "$CONTEXT"
cd "$CONTEXT"
vercel link --yes --project denju-registry --scope zue
vercel deploy --yes --scope zue
```

The staging command copies the current tracked/unignored working tree and replaces only the root
Vercel config in the disposable context. It never rewrites the docs project's checked config.

Neon PostgreSQL and Cloudflare R2 remain the durable authorities. Hosted environment variables
must provide separate restricted request/worker/direct database URLs plus a migration-owner URL
for the controlled migration step. R2 must use its S3 API hostname for signed object URLs, region
`auto`, and scoped S3 application credentials; a public/custom R2 domain is not a presign
substitute.

## Recovery after scale-to-zero

Vercel lifecycle is treated exactly like arbitrary process recycle. A scheduled request to
`/v1/internal/recover` carries Vercel's `CRON_SECRET` bearer and invokes the existing bounded
outbox and pack-release drains. The checked official config runs this only once per day so it is
valid on Vercel's Hobby cron tier. Correctness never depends on the schedule firing: ordinary
requests perform bounded request-adjacent draining, clients reconcile authoritative roots after
reconnect, and every recovery operation is idempotent.

For the official Vercel project, configure `CRON_SECRET`; Vercel sends that value in the request
Authorization header. Portable/self-hosted deployments may instead configure
`DENJU_RECOVERY_TOKEN`. If both variables are present they must contain the same secret or the
recovery endpoint fails closed, preventing a cron configuration from silently drifting away from
the server's expected bearer. Do not expose that endpoint through an ordinary
installation/session/automation credential.

## Deployment sequence

1. Build/test the same server container used by self-hosting.
2. Apply PostgreSQL migrations through the migration-owner connection.
3. Prove the configured R2 provider with `denju-server check-object-store`.
4. Stage a registry-only Vercel context with `cargo xtask vercel-context`, deploy a preview, and
   verify health, anonymous reads, authenticated mutations, object transfer,
   recycle/recovery, and two-instance wake behavior.
5. Deploy production, then verify the existing `registry.denju.ashray.xyz` alias, TLS, liveness,
   readiness, capabilities, and representative client reconciliation against the new process.

The GitHub release workflow publishes the six native client binaries, the shared release manifest,
install scripts, npm launcher, and the multi-architecture GHCR `denju-server` image from one tag.
