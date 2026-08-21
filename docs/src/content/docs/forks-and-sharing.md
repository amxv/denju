---
title: Forks and private sharing
description: Protect subscription edits with forks, synchronize upstream explicitly, resolve name collisions, and share private skills safely.
order: 6
category: Start
summary: Subscription edits become independent forks; private sharing grants read and subscription access without transferring ownership.
---

Denju treats subscribed skills as managed upstream state. Editing a subscription never writes into the upstream resource or silently loses the local change. Instead, Denju turns the edit into an independent fork.

## Automatic forks

For a claimed user, editing a subscribed skill automatically creates a private fork in that user's namespace, records immutable upstream provenance, replaces the ordinary upstream subscription with the fork, and rewires canonical and harness projections to the fork.

An anonymous subscriber receives the same protection locally. The fork has durable local revision history and upstream provenance but no registry resource ID. It remains on that device until an identity is claimed; claim uploads the existing history using the same revision IDs.

Forks do not follow upstream automatically. When upstream advances, `denju status` reports `upstream_ahead` and prints the explicit synchronization command:

```bash
denju fork sync @you/skill
```

`fork sync` performs the same deterministic three-way merge used for concurrent private workspace edits. Independent changes merge into a new fork revision; overlapping changes pause for explicit conflict resolution rather than overwriting either side. The fork's original creation provenance never changes even as its synchronization base advances.

## Explicit forks

Create a fork without first subscribing or editing:

```bash
denju fork @owner/skill
```

The new fork starts from the exact accessible upstream revision and remains independent thereafter. Private fork provenance is visible only to users who can inspect that fork. When a fork is public, its upstream provenance is public as well.

## Resolve an occupied fork name

Automatic forks use the upstream skill name. If `@you/skill` already exists, Denju does not invent a suffix or discard either resource. Only that skill pauses in `name_conflict` until one of these explicit choices is made:

```bash
denju fork resolve @upstream/skill --as new-name
denju fork resolve @upstream/skill --merge-into @you/skill
denju fork resolve @upstream/skill --discard
```

`--as` preserves the automatic fork under the chosen free name. `--merge-into` three-way merges the local fork into the named owned skill. `--discard` removes only the local automatic fork and restores the upstream subscription. No path silently invents a resource name.

## Share a private skill

Owners can grant another user private read and subscription access without transferring ownership:

```bash
denju share @you/skill @alice
denju unshare @you/skill @alice
```

`share` does not install anything for the recipient and does not create an inbox. It prints the exact `denju subscribe @you/skill` command to send them. While authorized, the private skill appears in their normal `denju search` and `denju show` results.

A subscription to a shared private skill follows every coherent valid private save, not only published releases. Revoking the share removes the recipient's managed upstream copy as soon as no public, pack, team, or other access source requires it. A personal fork the recipient already created is independent and survives revocation.
