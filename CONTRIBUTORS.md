# Contributor guide

## Prerequisites

- Rust 1.97.1 via `rust-toolchain.toml`
- Bun 1.3.11
- `just` 1.52+ is optional but recommended for the discoverable command menu
- Docker Compose for the pinned PostgreSQL/S3-compatible development dependencies used by integration phases
- Node.js 18+ only for the published npm wrapper checks

## Local development

Install the JavaScript workspaces without running the npm package's release-binary postinstall:

```bash
bun install --ignore-scripts
```

Use crate-scoped checks while iterating and the root gate before handoff:

```bash
just
just check-crate denju-core
just test denju-core
cargo check -p denju-core
cargo test -p denju-core
cargo xtask check
```

`cargo xtask ...` remains the canonical automation/CI interface. Just recipes are thin aliases only; do not move build, generation, migration, or environment logic into the Justfile, and do not add a Makefile as a second command authority.

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

The former Go/Agentbox implementation is historical only. Implementation work must not inspect its branch/source for parity, examples, tests, or architecture. Work from current `main`, the product specification, and the current workstream plan.
