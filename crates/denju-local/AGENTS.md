# `denju-local`

Root `AGENTS.md` applies. This crate owns end-user-machine environmental state and I/O.

## Keep here

- SQLite and local migrations, CAS metadata, generations and operation journal
- watcher/polling integration, projections, credential-store adapter, OS service adapters
- crash recovery and local filesystem repair

## Invariants

- secrets never live in SQLite or logs
- CLI correctness cannot require daemon IPC
- materialization is journaled and serialized per resource, not with a process-wide lock
- native links/junctions are required; never silently copy as fallback

## Fast check

`cargo test -p denju-local`
