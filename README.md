# denju

Denju is an agent-native social registry and near-real-time synchronization system for Agent Skills.

`main` is a clean Rust greenfield scaffold for the new product. The former Go/Agentbox implementation is historical only and is deliberately excluded from the implementation workflow.

## Workspace

```text
apps/                 native client/daemon and registry binaries
crates/               domain, wire, sync, local, client, registry, testkit
xtask/                 canonical developer commands
Justfile               discoverable aliases that delegate to xtask/Cargo/Bun
packages/npm/          thin native-binary npm installer
docs/                  Astro/ZueDocs site
spec/ tests/ fuzz/     protocol/conformance/acceptance harness homes
deploy/                self-hosting/container boundary
```

## Development

```bash
just
just check
cargo xtask check
cargo build --workspace
bun run docs:dev
```

`xtask` is the source of truth for repository automation and CI. `just` is only a convenient command menu; there is intentionally no Makefile command layer.

The authoritative product specification and implementation package live under ignored `tmp/gg/` local agent state during development.
