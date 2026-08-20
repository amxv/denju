# denju

Denju is an agent-native social registry and near-real-time synchronization system for Agent Skills.

`main` is a clean Rust greenfield scaffold for the new product. The former Go/Agentbox implementation is preserved on the `legacy/go-agentbox-v0.2.0` branch and is not an architectural constraint.

## Workspace

```text
apps/                 native client/daemon and registry binaries
crates/               domain, wire, sync, local, client, registry, testkit
xtask/                 canonical developer commands
packages/npm/          thin native-binary npm installer
docs/                  Astro/ZueDocs site
spec/ tests/ fuzz/     protocol/conformance/acceptance harness homes
deploy/                self-hosting/container boundary
```

## Development

```bash
cargo xtask check
cargo build --workspace
bun run docs:dev
```

The authoritative product specification and implementation package live under ignored `tmp/gg/` local agent state during development.
