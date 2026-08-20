# `denju-registry`

Root `AGENTS.md` applies. This crate owns hosted Denju authority and persistence.

## Keep here

- typed registry use cases and application authorization
- PostgreSQL queries/migrations, RLS policy ownership, idempotent operations
- S3-compatible object-store adapter, logical quota/reachability, search, outbox/workers

## Invariants

- `migrations/` is the sole hosted PostgreSQL migration source
- PostgreSQL/current refs remain authoritative; outbox/fanout/search caches are derived
- cross-tenant physical dedup never becomes an object-existence oracle
- R2 and Garage pass one provider-conformance boundary; do not fork domain behavior by provider

Axum/Hyper request handling belongs in `apps/denju-server`.

## Fast check

`cargo test -p denju-registry`
