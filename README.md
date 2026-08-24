# denju

Denju is an agent-native social registry and near-real-time synchronization system for Agent Skills.

It gives Codex and Claude Code one managed, versioned skill catalog with anonymous discovery,
automatic synchronization, private workspaces, forks/proposals, packs, teams, and deterministic
conflict handling. The client and registry are native Rust; PostgreSQL and S3-compatible object
storage are the registry authority.

## Install

The easiest cross-platform install is the thin npm wrapper, which downloads and verifies the
matching native release binary:

```bash
npm install -g --allow-scripts=denju-cli denju-cli
denju setup
```

macOS/Linux and Windows standalone installers are also attached to each GitHub Release as
`denju.sh` and `denju.ps1`. All install paths verify the same release manifest and SHA-256 values;
none compiles from source as a fallback.

## Start using Denju

```bash
# No account is required for discovery or subscriptions.
denju search "react performance"
denju show @owner/skill
denju subscribe @owner/skill

# Claim identity only when you want to publish, share, or join a team.
denju claim @alice
denju import ~/.agents/skills/my-skill
denju publish @alice/my-skill

# Inspect or repair local state at any time.
denju status
denju doctor
```

The default hosted registry is `https://registry.denju.ashray.xyz`. A Denju installation is bound
to one registry; self-hosted deployments use the same `denju-server` container and protocol.

## Documentation

Product documentation lives at **https://denju.ashray.xyz** and includes setup, identity,
resources/history, synchronization/conflicts, forks/proposals, packs/teams, discovery, trust and
quarantine, self-hosting, hosted operations, and install/upgrade behavior. The docs site also
publishes raw Markdown routes for agents and tooling.

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
just dev
cargo xtask check
cargo build --workspace
bun run docs:dev
```

`xtask` is the source of truth for repository automation and CI. `just` is only a convenient command menu; there is intentionally no Makefile command layer.

`cargo xtask dev` starts the pinned PostgreSQL/Garage development dependencies, applies registry migrations, and runs the local registry at `http://127.0.0.1:7788`.

The authoritative product specification and implementation package live under ignored `tmp/gg/` local agent state during development.
