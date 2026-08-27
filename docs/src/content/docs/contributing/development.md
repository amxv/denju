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

## Automated hosted-registry deployment

A tagged release always builds and publishes the multi-architecture `denju-server` image. Hosted Vercel deployment is optional: if no Vercel credentials are configured, the release workflow skips that deployment path so forks are not coupled to the upstream project.

To make a fork deploy its registry from the exact release image, configure these GitHub Actions **secrets**:

```text
VERCEL_TOKEN
VERCEL_ORG_ID
VERCEL_PROJECT_ID
DENJU_DATABASE_MIGRATION_URL
```

Also configure this repository **variable** with the public HTTPS origin of that registry:

```text
VERCEL_REGISTRY_ORIGIN=https://denju.example.com
```

`DENJU_DATABASE_MIGRATION_URL` is the privileged direct PostgreSQL owner connection used only by release automation. Do not add it to the long-lived Vercel runtime environment; the deployed server should continue using the restricted app, worker, and direct-session roles documented under [Self-host configuration](/docs/self-host/configuration).

With those settings present, release automation publishes the exact tagged image, verifies it can be pulled anonymously, applies migrations with that image, deploys Vercel from a tiny `FROM <exact-tag>` context, confirms the public origin points at the newly created deployment, and only then publishes the GitHub/npm release.

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
cargo xtask docs        # fast docs-specific tests; no Astro check/build
bun run docs:check      # Astro diagnostics + docs tests
bun run docs:build      # production site build
bun run docs:dev        # local docs server
```

`cargo xtask docs` is intentionally the fast iteration command. The comprehensive repository gate still runs Astro diagnostics and the production build. The docs site remains under `docs/` and preserves both normal HTML pages and raw `.md` routes for agents and tools.
