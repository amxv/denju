# `denju-core`

Root `AGENTS.md` applies. This is Denju's pure domain and semantic-identity boundary.

## Invariants

- no Tokio, filesystem, database, network, process, environment, or platform service I/O
- semantic hashes never depend on serde/wire/database encodings
- portable path and Agent Skills validation have one canonical implementation
- deterministic merge primitives live here; conflict persistence/execution does not

Prefer owned domain values that make invalid states unconstructible. Do not add generic utility abstractions.

## Fast checks

- `cargo test -p denju-core`
- `cargo clippy -p denju-core --all-targets -- -D warnings`
