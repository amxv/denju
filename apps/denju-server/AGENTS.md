# `apps/denju-server`

Root `AGENTS.md` applies. This package is the registry process edge.

## Keep here

- Axum/Hyper transport wiring, process configuration, health/readiness
- migration/admin command entrypoints and dependency assembly

## Keep out

- registry business rules, authorization policy, SQL/S3 ownership (`denju-registry`)
- wire DTO ownership (`denju-wire`)
- client-side merge/reconciliation

Handlers should translate transport concerns and delegate immediately to typed registry use cases.

## Fast check

`cargo check -p denju-server`
