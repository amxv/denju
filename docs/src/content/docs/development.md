---
title: Development
description: The small set of commands agents and contributors need to build, verify, and run Denju locally.
order: 2
category: Development
---

From the repository root:

```bash
cargo xtask check
cargo build --workspace
cargo xtask dev
bun run docs:dev
```

Rust is the primary project. The root Bun workspace exists only for the documentation site and the published npm installer shim.

`cargo xtask dev` owns the local dependency and registry lifecycle. It starts the pinned PostgreSQL 18.6 and Garage 2.3.0 services, applies registry migrations, and runs the registry at `http://127.0.0.1:7788`. Re-running it while the registry is already live is safe.

For setup development, use an isolated home and the explicit local registry rather than your real harness roots:

```bash
TEST_HOME="$(mktemp -d)"
HOME="$TEST_HOME" \
  CODEX_HOME=.codex \
  CLAUDE_CONFIG_DIR=.claude \
  DENJU_TEST_FILE_CREDENTIALS=1 \
  DENJU_TEST_SERVICE_INSTALL_ONLY=1 \
  cargo run -p denju -- setup --registry http://127.0.0.1:7788
```
