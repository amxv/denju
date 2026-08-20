# `denju-wire`

Root `AGENTS.md` applies. This crate owns versioned external data shapes, not domain authority.

## Keep here

- `/v1` JSON request/response DTOs, stable machine error values, SSE hint shapes
- versioned CLI `--json` envelopes and generated-contract source models

## Invariants

- depends toward `denju-core`; never owns semantic object identity
- additive compatible evolution inside `/v1`; breaking semantics require a new major path
- transport serialization must never become a Merkle hash transcript

## Fast check

`cargo test -p denju-wire`
