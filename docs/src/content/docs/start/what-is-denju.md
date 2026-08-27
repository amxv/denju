---
title: What is Denju?
description: The simplest mental model for Denju, the problems it solves, and how skills, subscriptions, packs, and teams fit together.
order: 1
category: Start
summary: "Denju discovers Agent Skills and keeps public subscriptions, private skills, packs, and team skills current across machines."
---

Denju is a registry and synchronization system for [Agent Skills](https://agentskills.io/). The easiest mental model is **npm-style discovery and publishing, plus automatic synchronization for the skills you actually use—including private ones**.

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

## Private sync is a core use case

You do **not** have to publish a skill publicly to get value from Denju.

Import one of your own skills and it starts private. Valid saves synchronize automatically to your other signed-in Denju devices, so the same private skill can stay current on your laptop, workstation, or another machine without Git, Dropbox, rsync, or a second setup step.

You can also share a personally owned private skill with one specific Denju user without making it public:

```bash
denju share @alice/my-skill @bob
```

Sharing grants Bob private read and subscription access and prints the exact `denju subscribe ...` command for him to run. It does not auto-install anything. If Bob subscribes, his copy follows Alice's valid saved changes while the skill remains absent from the public catalog.

Teams get the same benefit without making their skills public. A normal team publish creates a **team-only release**. Team members can subscribe to it directly, or the team can put private team skills in an assigned pack so Denju keeps that private skill set current for current and future members automatically.

One important boundary: a maintainer's unfinished edits stay private to that maintainer. Teammates receive the next team-only release when it is published; `--public` is a separate opt-in for making that team skill globally visible.

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

A pack is a set of skills you want kept together.

```text
@acme/packs/core
  @acme/code-review      latest
  @owner/testing         v3
  @owner/security        latest
```

Subscribe to a pack and Denju keeps that whole set of skills installed and current. Add a skill, remove one, or publish a new release of a followed skill and subscribed machines update automatically. Packs are flat—skills only, no packs-inside-packs—and every pack version records the exact skill revisions it resolved to.

### Teams

Teams own skills and packs under a shared name such as `@acme`. A team can **assign** a pack to its people. The pack contains the skills; team members are the people receiving them.

For example, a legal organization can keep its contract-review, research, citation, and drafting skills private to the team, put them in one `legal-core` pack, and assign that pack to the team. Current and future team members receive those private skills, and Denju keeps everyone aligned whenever the pack changes.

That is the main team use case: make the organization's approved skill set continuously true instead of maintaining an onboarding checklist that slowly becomes stale.

## Identity is optional until it is useful

You do not need an account to set up Denju, search the public registry, inspect public skills, or subscribe to them.

Identity enters when you want to do something that needs ownership or access control—publish a skill, synchronize private work across devices, share privately, star skills, or join a team.

```bash
denju claim @alice
```

Denju uses a username, password, and one-time recovery secret. There is no email requirement and no browser step.

## Denju does not replace your agent harness

Denju manages **which Agent Skill directories should exist and which revisions they should contain**. Codex and Claude Code continue to discover and load those skills normally.

On a configured machine, Denju keeps one managed copy of each installed skill and makes it available to the supported harnesses. If two installed skills would use the same Agent Skills name, Denju gives them deterministic local aliases so both can coexist.

## Denju is also self-hostable

The public registry is the default, but Denju is not tied to it. An organization can run the same open-source `denju-server` against its own PostgreSQL database and S3-compatible object store.

Self-hosting always runs the published Denju server image. The reference Docker Compose stack simply bundles PostgreSQL and Garage around it; managed PostgreSQL and S3 services use the same server image and contract.

Next: [Install Denju](/docs/start/install) or jump straight to [Self-hosting](/docs/self-host/quickstart).
