---
title: Performance and resilience
description: Run Denju's reproducible load, stateless-host, property, and observability checks.
order: 3
category: Development
summary: Measure normal-use latency, multi-instance recovery, fanout, object storage, and bounded property coverage.
---

## Run the deterministic repository contracts

`cargo xtask check` remains Denju's canonical CI and handoff gate. It now verifies the checked
conformance fixture checksums, fixture-test coverage, SQLx offline-metadata policy, and the single
xtask/Just automation authority before running Rust, npm, and documentation checks.

```bash
cargo xtask contracts
cargo xtask check
```

If an intentionally reviewed conformance vector changes, regenerate only its checksum manifest
with `cargo xtask contracts --update` and review the resulting diff. Denju currently uses SQLx's
runtime query APIs, so ordinary compilation does not require a live database or `.sqlx/query-*.json`
files; introducing compile-time query macros must also introduce checked offline metadata.

## Exercise the property corpus

The bounded property/fuzz corpus covers portable paths, semantic IDs and trees, deterministic and
malformed snapshots, merge/reconciliation invariants, and canonical request hashing.

```bash
cargo xtask fuzz
```

The default extended run uses 4096 cases per property. `DENJU_PROPTEST_CASES` can select another
bounded count while investigating a failure. Minimized reproducible regressions belong beside the
property that found them.

## Run the stateless load harness

`cargo xtask load` is an explicit non-CI integration/load command. It starts the pinned PostgreSQL
18.6 and Garage 2.3.0 dependencies, creates a fresh Phase-17 database, builds release binaries,
seeds a public catalog, and runs ordinary `denju-server` processes.

```bash
cargo xtask load
```

The harness records normal-use CLI state/search latency, registry search/show p95, warm and cold
server behavior, concurrent and horizontal traffic, reconcile scaling, a 1000-subscriber publish,
follow-latest pack fanout, 500-member team reads, concurrent object-store probes, and daemon memory
and watcher behavior. It also forces Garage failure/restart, SSE disconnect/reconnect, PostgreSQL
LISTEN disconnect/reconnect, SIGTERM, arbitrary process death, a complete scale-to-zero gap, and
authenticated outbox recovery.

The current accepted machine report lives under `tests/load/reports/`. A run also writes its latest
machine-local JSON report under ignored `tmp/gg/denju-rust-greenfield/` for comparison. CLI reports
retain both p50 and p95; the under-50ms local normal-use criterion is evaluated at p50, while the
registry's under-200ms criterion is explicitly p95. Online publish propagation must remain below
two seconds.

Every CLI fixture uses a marked `DENJU_TEST_HOME` and deliberately poisons inherited Codex/Claude
environment paths. The harness must never inspect, project into, or clean real user harness roots.

## Inspect server health and metrics

The registry exposes separate liveness, readiness, and operational-metrics endpoints:

```text
/health/live
/health/ready
/health/metrics
```

Operational metrics are bounded process counters plus authoritative outbox lag. They cover HTTP
requests/5xx/latency buckets, active and total SSE connections, dirty-set overflow, PostgreSQL and
object-store errors, object-transfer bytes, reconcile known/changed roots, outbox drains/events,
and PostgreSQL wake-listener state. Metrics and structured JSON tracing never include request
bodies, bearer tokens, presigned URLs, passwords, recovery secrets, or private skill content.

The local daemon writes its own atomic health snapshot to
`~/.denju/run/daemon.metrics.json` (or the equivalent path under `DENJU_TEST_HOME`). It reports
watcher mode, iterations, full-hash fallback scans, capture/sync error counts, and the latest local
capture/remote-sync durations. These values are diagnostics only; correctness continues to come
from SQLite/PostgreSQL/object storage and authoritative reconciliation.
