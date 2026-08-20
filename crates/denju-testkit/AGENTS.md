# `denju-testkit`

Root `AGENTS.md` applies. This crate is shared deterministic test support only.

## Keep here

- reusable domain/wire fixtures, deterministic builders, fake clocks/IDs, conformance helpers

## Invariants

- production crates never depend on this crate
- do not turn it into a fake implementation that bypasses real e2e paths
- live/provider/e2e evidence belongs under the real test harness, not behind testkit mocks

## Fast check

`cargo test -p denju-testkit`
