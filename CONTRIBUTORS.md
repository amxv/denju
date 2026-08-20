# Contributor guide

## Prerequisites

- Rust 1.97.1 via `rust-toolchain.toml`
- Bun 1.3.11
- Node.js 18+ only for the published npm wrapper checks

## Local development

Install the JavaScript workspaces without running the npm package's release-binary postinstall:

```bash
bun install --ignore-scripts
```

Use crate-scoped checks while iterating and the root gate before handoff:

```bash
cargo check -p denju-core
cargo test -p denju-core
cargo xtask check
```

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

A `vX.Y.Z` tag runs the release workflow. It builds native `denju` binaries for macOS, Linux, and Windows on x64 and arm64 runners, publishes a SHA-256 manifest with the GitHub Release, and then publishes the matching `denju-cli` npm wrapper. The npm installer verifies the downloaded binary against that manifest and never compiles from source as a fallback.

The former Go/Agentbox implementation is preserved on `legacy/go-agentbox-v0.2.0`; do not use it as an implementation template for `main`.
