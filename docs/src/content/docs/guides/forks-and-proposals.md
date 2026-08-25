---
title: Forks, private sharing, and proposals
description: Edit subscribed skills safely, synchronize a fork explicitly, share private work, and propose changes back upstream.
order: 13
category: Use Denju
summary: "Local edits become independent forks instead of mutating upstream; proposals send a fork head back without inventing Git branches."
---

A subscribed skill still belongs to its publisher. If you edit your local copy, Denju protects both your work and the publisher's version by turning your edit into a fork.

## Automatic forks

For a signed-in user, editing a subscription creates a private fork under your Denju name and replaces the ordinary upstream subscription with that fork.

For an anonymous user, Denju creates a device-local fork with revision history and upstream provenance. If you claim an identity later, that existing history can be promoted without rewriting its revision IDs.

Forks do **not** follow upstream automatically.

When upstream advances:

```bash
denju status
denju fork sync @you/skill
```

`fork sync` uses the same three-way merge rules as Denju's private skill synchronization. Clean changes merge. Conflicting paths remain for explicit resolution.

## Create a fork explicitly

```bash
denju fork @owner/skill
```

The fork remembers which upstream skill and revision it started from, even after you change it further.

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

Sharing is for a **personally owned private skill**. It grants one specific Denju user read and subscription access without making the skill public.

It does not auto-install the skill for the recipient and does not create an inbox. `denju share` prints the exact `denju subscribe ...` command you can send them.

Once the recipient subscribes, their copy follows your valid saved changes, not only public releases. This makes private sharing useful for a collaborator who should stay current with your working skill without exposing it to the public registry.

If access is revoked, Denju removes the upstream managed copy when nothing else still needs it. A fork the recipient already created remains theirs.

Team-owned skills use team membership instead of new per-skill share grants. Publish a team-only release and use direct team subscriptions or assigned packs for private team distribution.

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

Acceptance applies the proposed version to the maintainer's private working copy. It does **not** publish a release automatically. The maintainer publishes separately after deciding the accepted change is ready for consumers.
