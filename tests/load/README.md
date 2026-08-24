# Phase 17 load and stateless lifecycle harness

`cargo xtask load` is the canonical non-CI performance/resilience harness. It uses the pinned
PostgreSQL 18.6 + Garage 2.3.0 development services, creates a fresh `denju_phase17_load`
database, seeds a real public catalog, and starts ordinary release `denju-server` processes.

The harness measures registry search/show p95, horizontal two-instance traffic, isolated release
CLI state/search normal-use latency (p50 target with p95 retained in the report), and reconcile
scaling at 25/100/200 watched roots. It also exercises
SIGTERM, arbitrary process death, a scale-to-zero gap with a committed pending outbox event,
authenticated recovery draining, cross-instance PostgreSQL wake delivery, forced listener
disconnect/reconnect, and SSE connection loss/reconnect.

All CLI work uses an explicit marked `DENJU_TEST_HOME`. Inherited `CODEX_HOME` and
`CLAUDE_CONFIG_DIR` values are deliberately poisoned; test projection must remain beneath that
isolated home and must never touch real Codex, Claude, or `~/.agents` roots.

The latest machine-local JSON report is written under ignored
`tmp/gg/denju-rust-greenfield/phase17-load-report-latest.json`. Environment-specific accepted
results are copied into `tests/load/reports/` after review. The checked report records both p50
and p95 for CLI process latency, even though the product's under-50ms normal-use criterion is not
a p95 contract; registry show/search keep the explicit under-200ms p95 contract from the plan.
