---
title: Packs
description: Group skills into one reusable set and let Denju keep that set current when skills are added, removed, or updated.
order: 14
category: Use Denju
summary: "A pack is a set of skills: subscribe once and Denju keeps the whole set current."
---

A pack is simply **a set of skills**.

Use a pack when several skills belong together and you want to manage them as one thing. Instead of telling someone to install five skills one by one, give them one pack to subscribe to.

For example, a legal organization might keep a pack like this:

```text
@northstar/packs/legal-core
  @northstar/contract-review       latest
  @northstar/legal-research        latest
  @northstar/citation-check        v3
  @northstar/client-letter-draft   latest
```

Subscribe to `@northstar/packs/legal-core` and Denju keeps those skills installed. If the pack later adds a litigation skill, removes the client-letter skill, or moves one of its skills to a newer release, Denju updates the subscribed computers automatically.

That is the whole mental model: **the pack says which skills should be there; Denju keeps that set true.**

## Create a pack

Create an empty pack:

```bash
denju pack create @alice/packs/core
```

Add one or more skills:

```bash
denju pack add @alice/packs/core @owner/review @owner/testing @owner/security
```

Remove skills:

```bash
denju pack remove @alice/packs/core @owner/security
```

Each change creates the next immutable version of the pack. Adding or removing several skills in one command is atomic: either the whole change succeeds or none of it does.

Packs are deliberately flat. A pack contains skills, not other packs.

## How a pack stays current

Each skill in a pack can either follow its latest release or stay pinned to an exact release.

Add a skill normally to follow latest:

```bash
denju pack add @alice/packs/core @owner/testing
```

Pin it to an exact release when you need that stability:

```bash
denju pack add @alice/packs/core @owner/testing@v3
```

If `@owner/testing` follows latest and its owner publishes a new release, Denju advances the pack to a new version that resolves to that release. If it is pinned to `v3`, it stays on `v3`.

Every pack version records the exact revision of every skill in the set. That makes old pack versions reproducible even when the live pack has moved on.

## Subscribe to a pack

Publish a personal pack if other people should be able to find it:

```bash
denju publish @alice/packs/core
```

Then subscribe:

```bash
denju subscribe @alice/packs/core
```

From then on, Denju keeps the skills in that pack current on the subscriber's machine.

If the pack changes:

- a newly added skill is installed;
- a removed skill is removed when nothing else requires it;
- a followed skill moves to its new release;
- a pinned skill stays pinned.

Before changing what the agent can see, Denju verifies the complete new pack state and applies it transactionally. A failed download or interrupted process does not leave half of the pack updated.

A live pack subscription follows the pack itself, so it does not use the direct-skill `--version` or `--retain-on-delete` options.

## Personal, team, and public packs

A personal pack can start private and use skills its owner is allowed to read.

A pack owned by a team can contain public skills and private skills available to the whole team. It cannot depend on a personal skill that only one maintainer can read, because the rest of the team would not be able to satisfy the pack.

Making a pack public requires every skill in it to be public.

## When a skill becomes unavailable

Denju does not silently edit the pack if one of its skills is deleted, unpublished, access-revoked, or quarantined.

Instead, the pack stays intact and is reported as **degraded**, with the unavailable skill and reason shown explicitly.

If access to that same skill returns, Denju can install it again automatically. Otherwise, edit the pack and remove or replace that skill.

## When two packs disagree

A machine can subscribe to more than one pack. If two packs require different exact revisions of the same skill, Denju does not silently choose one.

It keeps the last valid version visible when possible and reports the conflicting packs in `denju status` so you can decide which requirement to change.

For organization-wide policy, a team can assign a pack to all of its people. See [Teams and assigned packs](/docs/guides/teams).
