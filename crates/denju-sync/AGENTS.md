# `denju-sync`

Root `AGENTS.md` applies. This crate owns deterministic synchronization and desired-state decisions.

## Invariants

- no filesystem, SQLite, network, process, clock, or async runtime I/O
- model reconciliation as deterministic state/input -> actions
- one resolved active revision per immutable skill resource ID
- never implement silent last-write-wins or hide source conflicts with duplicate versions

Execution of returned actions belongs at the local/client edges.

## Fast check

`cargo test -p denju-sync`
