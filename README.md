# denju

**The right Agent Skills, on every machine.**

> **denju · 伝授** — Japanese for passing on or transmitting knowledge or a skill.

Denju is a registry and synchronization system for [Agent Skills](https://agentskills.io/).
Find a skill, subscribe once, and Denju keeps the right version available to Codex and Claude Code automatically—across your computers or across an entire team.

Agent Skills are ordinary directories. That simplicity is useful, but copied folders drift: one machine has the new version, another still has the old one, and nobody remembers which copy should win. Denju replaces that manual copying with subscriptions.

## Get started

Install the native Denju CLI through the npm wrapper:

```bash
npm install -g --allow-scripts=denju-cli denju-cli
denju setup
```

Then find and subscribe to a skill:

```bash
denju search "react performance"
denju show @owner/skill
denju subscribe @owner/skill
```

That is enough to start. Public search and subscriptions do not require an account.

When the publisher releases a new version, Denju keeps the subscribed skill current. The skill still appears to Codex and Claude Code as an ordinary Agent Skill on disk.

## Packs are sets of skills

A pack groups skills that belong together:

```text
@northstar/packs/legal-core
  @northstar/contract-review
  @northstar/legal-research
  @northstar/citation-check
  @northstar/client-letter-draft
```

Subscribe to the pack once and Denju keeps that whole set current. Add a skill to the pack and it appears. Remove one and Denju removes it when nothing else needs it. Publish a new release of a followed skill and the pack moves forward automatically.

```bash
denju subscribe @northstar/packs/legal-core
```

## Keep a team on the same skills

Imagine a legal organization that wants every person using agents to have the same approved contract-review, research, citation, and drafting skills.

Create a team pack and assign it to the team:

```bash
denju team assign @northstar @northstar/packs/legal-core
```

Denju keeps the skills in that pack on every current team member's computers, and new team members receive them when they join. Change the pack once and everyone stays current.

Those team-owned skills can stay private to the organization. A normal team publish creates a team-only release; public visibility is a separate opt-in.

A **team member is a person**. A **pack contains skills**. Assigning a pack is how an organization turns a useful set of skills into team policy.

## Keep your own skills private—or publish them

Claim an identity only when you need ownership, publishing, private sharing, or teams:

```bash
denju claim @alice
```

Bring an existing Agent Skill into Denju:

```bash
denju import ~/.agents/skills/my-skill
```

Imported skills start private. Edit them normally; Denju keeps revision history and synchronizes valid private saves across your signed-in devices automatically. You can stop there and use Denju purely as private multi-device skill sync.

You can also share a private skill with another Denju user without making it public:

```bash
denju share @alice/my-skill @bob
```

Bob decides whether to subscribe. If he does, his private subscription follows your valid saves while the skill stays out of the public catalog.

Publish only when you want the wider registry to use it:

```bash
denju publish @alice/my-skill
```

Publishing creates immutable numbered releases such as `v1`, `v2`, and `v3`.

If you edit somebody else's subscribed skill, Denju preserves your change as a fork instead of silently changing upstream. You can later sync from upstream or propose your changes back.

## A small product model

Denju is built around four ideas:

- **Skill** — an ordinary Agent Skill directory with published releases.
- **Subscription** — keep this skill installed and current, or pin an exact release.
- **Pack** — a set of skills that should stay together.
- **Team** — shared ownership plus assigned packs for keeping an organization aligned.

Denju does not require Git branches, semantic versioning, a new skill format, or a separate runtime API for agents.

## Self-hosting

The official registry is the default, but Denju is open source and self-hostable.

Each release publishes the same production `denju-server` image at `ghcr.io/amxv/denju-server:vX.Y.Z`. Run it with PostgreSQL and S3-compatible object storage, then set up clients against your registry:

```bash
denju setup --registry https://denju.example.com
```

Self-hosting always runs that published image. The reference Docker Compose stack simply bundles PostgreSQL and Garage around it; if you already have PostgreSQL and S3-compatible storage, point the same image at those services.

## Documentation

Read the docs at **https://denju.ashray.xyz**.

Start with:

- [What is Denju?](https://denju.ashray.xyz/docs/start/what-is-denju)
- [Install and set up](https://denju.ashray.xyz/docs/start/install)
- [Packs](https://denju.ashray.xyz/docs/guides/packs)
- [Teams and assigned packs](https://denju.ashray.xyz/docs/guides/teams)
- [Self-hosting](https://denju.ashray.xyz/docs/self-host/quickstart)
- [Architecture](https://denju.ashray.xyz/docs/architecture/overview)

## Development

Denju is a Rust workspace. `just` is the discoverable developer/agent command menu. Scoped verification uses a zero-build-cost selector; `cargo xtask` owns the heavier repository-wide automation behind CI, release, generation, and development services.

```bash
just
just check denju
just test-target denju cli
just verify
just full
just dev
cargo xtask check
cargo build --workspace
bun run docs:dev
```

Use `just check` / `test-target` while iterating, `just verify` before a normal handoff, and reserve `just full` / `cargo xtask check` for the comprehensive CI/release gate.

Repository shape:

```text
apps/          client/daemon and registry binaries
crates/        core, wire, sync, local, client, registry, testkit
docs/          Astro/ZueDocs documentation site
deploy/        development and self-host deployment files
packages/npm/  thin native-binary npm installer
spec/          stable formats and protocol fixtures
xtask/         developer and CI automation
```

See [`AGENTS.md`](./AGENTS.md) and the contributing docs before changing the implementation.
