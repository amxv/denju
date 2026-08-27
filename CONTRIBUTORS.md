# Contributor guide

## Prerequisites

- Rust 1.97.1 via `rust-toolchain.toml`
- Bun 1.3.11
- `just` 1.52+ is optional but recommended for the discoverable command menu
- Docker Compose for the pinned PostgreSQL/S3-compatible development dependencies used by integration and load tests
- Node.js 18+ only for the published npm wrapper checks

## Local development

Install the JavaScript workspaces without running the npm package's release-binary postinstall:

```bash
bun install --ignore-scripts
```

Use the smallest useful check while iterating and the affected-package gate before a normal handoff:

```bash
just
just check denju-core
just test denju-core
just lint denju-core
just verify
cargo check -p denju-core
cargo test -p denju-core
just full
```

`just verify` auto-detects changed Rust packages, includes workspace reverse dependents in the compile/lint closure, and avoids a redundant standalone `cargo check` before Clippy. Its lightweight selector is `scripts/scoped_verify.py`, which intentionally has no Rust build startup cost. `cargo xtask check` remains the comprehensive CI/release interface behind `just full`. Just recipes stay thin; do not move build, generation, migration, or environment logic into the Justfile, and do not add a Makefile as a second command authority.

Build all Rust packages with:

```bash
cargo build --workspace
```

## Documentation

The Astro/ZueDocs app is isolated under `docs/`:

```bash
bun run docs:dev
bun run docs:check
bun run docs:build
```

## Release shape

A `vX.Y.Z` tag runs the release workflow. It builds native `denju` binaries for macOS, Linux, and Windows on x64 and arm64 runners, assembles the shared SHA-256/size manifest plus POSIX and PowerShell installers, publishes the multi-architecture `denju-server` container, creates the GitHub Release, and publishes the matching `denju-cli` npm wrapper. Installer, npm, and `denju upgrade` paths consume the same release contract and never compile from source as a fallback.
