---
title: What is Denju?
description: The simplest mental model for Denju, the problems it solves, and how skills, subscriptions, packs, and teams fit together.
order: 1
category: Start
summary: "Denju is a package registry plus automatic synchronization for Agent Skills."
---

Denju is a registry and synchronization system for [Agent Skills](https://agentskills.io/). The easiest mental model is **npm for discovering and publishing skills, plus automatic filesystem synchronization for keeping the skills you use current**.

You can search for a skill, subscribe to it once, and stop thinking about copying its directory between machines or agent harnesses. When the owner publishes a new release, Denju updates the managed skill automatically. The agent still sees an ordinary `SKILL.md` directory on disk.

## What problem does Denju solve?

Agent Skills are just directories, which is a feature: they are portable, inspectable, and easy to create. But directories alone do not answer a few practical questions:

- Where do I discover skills other people have made?
- How do I install one and know which version I have?
- How do I keep the same skill current on another computer?
- How do I share a private skill with a teammate?
- How do we make sure everyone on a team has the same approved skill set?
- What happens when I edit somebody else's skill locally?

Denju adds those lifecycle and distribution pieces without replacing the Agent Skills format.

## The four concepts to know first

### Skills

A Denju skill is a complete Agent Skill directory with a stable scoped name such as:

```text
@alice/react-performance
@acme/code-review
```

Published releases are immutable and numbered `v1`, `v2`, `v3`, and so on. You can follow the latest release or pin an exact one.

### Subscriptions

A subscription means: **keep this skill present on my machine**.

```bash
denju subscribe @alice/react-performance
```

Denju downloads and verifies the release, keeps a canonical managed copy under `~/.denju`, and exposes it to Codex and Claude Code. A subscription follows new releases unless you pin it.

Subscriptions are intentionally separate from social follows. Following a person affects discovery; subscribing to a skill affects your filesystem.

### Packs

A pack is a versioned set of skill requirements.

```text
@acme/packs/core
  @acme/code-review      latest
  @owner/testing         v3
  @owner/security        latest
```

Subscribe to a pack and Denju keeps the whole set satisfied. Packs are flat—skills only, no packs-inside-packs—and every pack version records the exact skill revisions it resolved to.

### Teams

Teams own skills and packs in a shared namespace such as `@acme`. A team can also **assign** a pack to every member. Assigned packs are policy: current and future members receive the required skills, and Denju keeps them aligned as the pack changes.

That is the main team use case: make the team's preferred skills continuously true instead of writing an onboarding checklist that slowly becomes stale.

## Identity is optional until it is useful

You do not need an account to set up Denju, search the public registry, inspect public skills, or subscribe to them.

Identity enters when you want to do something that needs ownership or access control—publish a skill, synchronize private work across devices, share privately, star resources, or join a team.

```bash
denju claim @alice
```

Denju uses a username, password, and one-time recovery secret. There is no email requirement and no browser step.

## Denju does not replace your agent harness

Denju manages **which Agent Skill directories should exist and which revisions they should contain**. Codex and Claude Code continue to discover and load those skills normally.

On a configured machine, Denju keeps one canonical managed version of each installed resource and projects it into the supported harness roots. If two packages would expose the same Agent Skills name, Denju gives the conflicting local projections deterministic aliases so both can coexist.

## Denju is also self-hostable

The public registry is the default, but Denju is not tied to it. An organization can run the same open-source `denju-server` against its own PostgreSQL database and S3-compatible object store.

The reference Docker Compose stack includes PostgreSQL and Garage. Managed PostgreSQL and S3 services work through the same server contract.

Next: [Install Denju](/docs/start/install) or jump straight to [Self-hosting](/docs/self-host/quickstart).
