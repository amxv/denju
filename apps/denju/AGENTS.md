# `apps/denju`

Root `AGENTS.md` applies. This package is process wiring for the public `denju` CLI and hidden per-user daemon entrypoint.

## Keep here

- command parsing, terminal/JSON presentation, process startup, dependency wiring
- daemon process wiring and top-level lifecycle integration

## Keep out

- domain invariants (`denju-core`)
- reconciliation decisions (`denju-sync`)
- SQLite/filesystem/service implementations (`denju-local`)
- HTTP/SSE/S3 protocol execution (`denju-client`)

Command handlers should delegate quickly into owned crate APIs. Do not turn this binary into a product-logic module.

## Fast check

`cargo check -p denju`
