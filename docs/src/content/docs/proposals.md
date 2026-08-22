---
title: Proposals
description: Send a fork's moving private head back to its upstream maintainer without creating review threads or publishing releases.
order: 8
category: Start
summary: Proposals are private moving references from a fork to its upstream; acceptance updates private history only.
---

A proposal is a private reference from one of your forks back to that fork's upstream skill. It is intentionally smaller than a pull request: there are no comments, review threads, expiry rules, or hidden Git branches.

## Open a proposal

```bash
denju propose @you/skill
denju propose @you/skill --message "Add the new workflow"
```

The source must still be a fork of the target and must keep the same skill name as upstream. The optional message is short context for the maintainer, not a discussion thread.

Only the proposer and the current upstream maintainer can list or inspect the proposal:

```bash
denju proposals
denju proposal show <id>
```

Unrelated users cannot enumerate or fetch its ID or contents.

## The proposal follows the fork

An open proposal always points at the fork's current immutable head. Saving another valid revision to the fork moves the proposal automatically; Denju does not create a second proposal or rewrite the old revision.

If upstream advances, the proposal uses the same explicit fork synchronization boundary as normal fork work. A clean three-way merge moves the fork and therefore the proposal to the merged head. A true overlap stays with the proposer for explicit resolution instead of asking the registry to invent a merge.

Normal `denju sync` and the background service attempt clean proposal-side fork synchronization for forks owned by the current identity. `denju proposals` reports `needs_sync` when the proposal cannot currently follow upstream cleanly, and the recovery command remains:

```bash
denju fork sync @you/skill
```

## Accept, reject, or withdraw

The upstream maintainer can accept or reject an open proposal:

```bash
denju proposal accept <id>
denju proposal reject <id>
```

The proposer can withdraw it while it is still open:

```bash
denju proposal withdraw <id>
```

Acceptance is compare-and-swap bound to the exact proposal head and upstream generation the maintainer inspected. It attaches that exact existing RevisionId to the maintainer's private workspace. If either side advances after inspection, acceptance fails and the maintainer must inspect the proposal again. Earlier unpublished maintainer revisions remain immutable history, but accepting is an explicit choice to make the proposal head the current private workspace rather than silently merging it with unrelated draft work.

Accepting a proposal never publishes a release. Existing public subscribers keep seeing the previous immutable release until the maintainer separately runs `denju publish`. The contributor's fork remains an independent fork after acceptance; Denju neither deletes nor reattaches it.
