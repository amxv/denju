# `denju-client`

Root `AGENTS.md` applies. This crate executes remote registry/data-plane operations.

## Keep here

- pooled HTTPS JSON client, SSE connection/reconnect, auth header/session handling
- presigned S3 transfer execution and transfer retry/concurrency mechanics

## Invariants

- local SQLite/desired-state authority does not live here
- object bytes are verified against Denju semantic identity before becoming trusted local state
- provider-specific S3 behavior stays behind the generic transfer boundary

## Fast check

`cargo test -p denju-client`
