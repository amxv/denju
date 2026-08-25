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

A **team member is a person**. A **pack contains skills**. Assigning a pack is how an organization turns a useful set of skills into team policy.

## Publish your own skills

Claim an identity only when you need ownership, publishing, private sharing, or teams:

```bash
denju claim @alice
```

Bring an existing Agent Skill into Denju and publish it:

```bash
denju import ~/.agents/skills/my-skill
denju publish @alice/my-skill
```

Imported skills start private. Edit them normally; Denju keeps revision history and can synchronize private work across your authenticated devices. Publishing creates immutable numbered releases such as `v1`, `v2`, and `v3`.

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

Run the same `denju-server` with PostgreSQL and S3-compatible object storage, then set up clients against your registry:

```bash
denju setup --registry https://denju.example.com
```

The repository includes a Docker Compose deployment with PostgreSQL and Garage, or you can use managed PostgreSQL and S3-compatible services.

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

Denju is a Rust workspace. `cargo xtask` is the repository automation and CI authority; `just` provides discoverable shortcuts.

```bash
just
just check
just dev
cargo xtask check
cargo build --workspace
bun run docs:dev
```

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
