---
title: Forks, private sharing, and proposals
description: Edit subscribed skills safely, synchronize a fork explicitly, share private work, and propose changes back upstream.
order: 13
category: Use Denju
summary: "Local edits become independent forks instead of mutating upstream; proposals send a fork head back without inventing Git branches."
---

A subscribed skill is somebody else's managed resource. If you edit it, Denju protects both your work and the upstream relationship by turning the edit into a fork.

## Automatic forks

For a signed-in user, editing a subscription creates a private fork in your namespace and replaces the ordinary upstream subscription with that fork.

For an anonymous user, Denju creates a device-local fork with revision history and upstream provenance. If you claim an identity later, that existing history can be promoted without rewriting its revision IDs.

Forks do **not** follow upstream automatically.

When upstream advances:

```bash
denju status
denju fork sync @you/skill
```

`fork sync` uses the same three-way merge rules as private workspace synchronization. Clean changes merge. Conflicting paths remain for explicit resolution.

## Create a fork explicitly

```bash
denju fork @owner/skill
```

The new resource records immutable provenance back to the upstream resource and starting revision.

## Handle a fork-name collision

If the natural personal name is already occupied, Denju does not invent a suffix. It pauses that skill and asks you to choose:

```bash
denju fork resolve @owner/skill --as new-name
denju fork resolve @owner/skill --merge-into @you/existing-skill
denju fork resolve @owner/skill --discard
```

## Share a private skill

```bash
denju share @you/skill @alice
denju unshare @you/skill @alice
```

Sharing grants read and subscription access. It does not auto-install the skill for the recipient and does not create an inbox. Send the recipient the `denju subscribe ...` command printed by `share`.

A private subscription follows coherent private saves live, not only public releases. If access is revoked, Denju removes the upstream managed copy when no other source requires it. A fork the recipient already created remains theirs.

## Propose a fork upstream

```bash
denju propose @you/skill --message "Add the new workflow"
```

A proposal is intentionally smaller than a pull request. It is a private moving reference from your fork to its upstream maintainer—no comments, threaded review, or Git branch abstraction.

List and inspect proposals:

```bash
denju proposals
denju proposal show <id>
```

As you continue editing the fork, the open proposal follows the fork's current head. If upstream advances cleanly, normal fork synchronization can update the proposal. Real conflicts return to you for explicit resolution.

The maintainer can accept or reject:

```bash
denju proposal accept <id>
denju proposal reject <id>
```

The proposer can withdraw:

```bash
denju proposal withdraw <id>
```

Acceptance applies the exact proposal revision to the maintainer's private workspace. It does **not** publish a release automatically. The maintainer publishes separately after deciding the accepted change is ready for consumers.
