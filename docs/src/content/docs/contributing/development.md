---
title: Development workflow
description: Build, test, run the local registry, and exercise Denju safely without touching real user Codex, Claude Code, Agent Skills, or Denju state.
order: 50
category: Contributing
summary: "cargo xtask is the repository authority; marked test homes are mandatory for process-level Denju fixtures."
---

Rust is the product implementation. The root JavaScript workspace exists only for the docs site and the thin npm native-binary installer.

## Canonical repository checks

```bash
cargo xtask check
```

This is the broad handoff/CI gate. It covers repository contracts, formatting, strict Clippy, workspace tests, npm checks, Astro diagnostics, and the production docs build.

Useful focused commands include:

```bash
cargo test -p denju-core
cargo clippy -p denju-local --all-targets -- -D warnings
cargo xtask contracts
cargo xtask fuzz
```

`Justfile` is only a discoverable alias layer. It should not become a second implementation of build, migration, release, or environment logic.

## Run the development registry

```bash
cargo xtask dev
```

This owns the pinned local PostgreSQL + Garage lifecycle, applies migrations, and runs the registry on the development loopback origin.

## Never test against real harness roots

Process-level CLI, daemon, and acceptance fixtures must use a marked isolated Denju test home. A temporary `HOME` by itself is **not sufficient** because the developer shell may already contain an absolute custom `CLAUDE_CONFIG_DIR` value, and real shared Agent Skills roots must never be touched by tests.

Create a disposable marked directory:

```bash
TEST_HOME="$(mktemp -d)"
touch "$TEST_HOME/.denju-test-home-v1"
```

Then run Denju with the absolute test home:

```bash
DENJU_TEST_HOME="$TEST_HOME" \
  cargo run -p denju -- setup --registry http://127.0.0.1:7788
```

In test-home mode Denju ignores inherited harness-root overrides, keeps simulated Codex/Claude state below the marked home, uses file-backed test credentials, and does not start the real per-user background service.

Tests must never use, scan as simulated state, write, migrate, link into, or clean a developer's real:

```text
~/.agents/
~/.codex/
~/.claude/
custom CODEX_HOME
custom CLAUDE_CONFIG_DIR
```

## Performance and stateless lifecycle harness

```bash
cargo xtask load
```

The load harness is an explicit integration/performance command rather than part of ordinary CI. It exercises local CLI latency, registry latency, fanout, long-disconnected reconcile, multi-instance wakeups, SSE/listener loss, process death, scale-to-zero, object-store recovery, and query plans.

## Documentation

```bash
bun run docs:check
bun run docs:build
bun run docs:dev
```

The docs site remains under `docs/` and preserves both normal HTML pages and raw `.md` routes for agents and tools.
