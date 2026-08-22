---
title: Editing owned skills
description: How Denju records local edits, synchronizes private workspaces, and protects invalid or concurrent work.
order: 5
category: Start
summary: Edit managed skills normally; Denju turns coherent valid saves into durable private revisions.
---

Imported skills remain ordinary writable directories under Denju's canonical managed tree. Editing through the canonical path or its normal Codex/Claude projection changes the same working generation.

## What happens after a save

The background service treats filesystem notifications as hints. After a short quiescence period Denju scans the complete skill, reuses known hashes for unchanged files, validates the Agent Skills and portable filesystem contract, and creates an immutable private revision only when semantic content actually changed.

The revision is recorded locally before upload. If the registry is offline, the save remains queued and synchronizes when connectivity returns. If namespace storage quota is exhausted, local editing still works and the revision remains queued until capacity is available.

`denju sync` performs the same correctness path without requiring the background service. This is why CLI behavior remains correct when the daemon is stopped.

## Invalid edits stay local

Denju never replaces an invalid working tree with a remote version. If a stable save is missing required frontmatter or violates the portable filesystem rules, that exact local content remains visible and synchronization pauses only that skill. Fix the skill and run:

```bash
denju sync
```

Changing the `name` field directly is treated differently because package identity is explicit. Denju preserves the edit and reports the exact `denju rename @owner/old-name new-name` command rather than silently changing the resource locator.

## Multiple devices

Each private workspace ref advances with compare-and-swap against the generation and parent revision the editing device observed. A successful save becomes visible to another authenticated device on its next reconciliation.

If two devices save from the same parent, Denju never silently lets the last request win. The first device may advance the remote workspace; the second keeps its local bytes and local revision and enters a conflict state.

Denju fetches both preserved revisions and performs a three-way merge on the client. Changes to different files and non-overlapping text edits merge automatically into one deterministic revision with both original heads as parents. If the edits overlap, only that skill pauses; unrelated skills continue synchronizing.

Inspect an unresolved conflict with:

```bash
denju status
```

The status output includes both immutable head IDs and exact commands to compare them. To keep either complete head, run the printed `denju restore @owner/skill <revision>` command. To make a custom resolution, edit the preserved working tree and run `denju sync`. Either path validates the complete result and records a two-parent merge revision, so choosing one side never erases the other head from history.

If another device resolves the conflict first, the next sync adopts that resolved revision only when this device's preserved conflict working tree is still untouched. New local resolution work is never overwritten.

## Team workspaces

A team does not have one shared writable branch. Every owner, maintainer, or publishing-enabled member edits a private workspace ref for that team skill. Another team member can read the latest team release, but cannot inspect another publisher's unpublished workspace.

When two publishers start from the same team release, their private heads may diverge safely. Publishing reconciles only the caller's private head with the latest team release. Non-overlapping changes merge deterministically; overlapping changes preserve both revisions and create conflict state scoped to that publisher. Another maintainer's private ref is never silently fast-forwarded or overwritten because somebody else published.

## Editing enforced team content

An enforced team-pack skill is deliberately different from an ordinary subscription. Editing it must not mutate or remove the team's required revision. Denju captures the edit into a personal fork, then restores the enforced upstream revision and keeps both resources projected with deterministic collision-safe harness names.

The fork remains independent personal work. The team assignment remains authoritative until the team owner unassigns the pack or the user leaves/is removed from that team. For an ordinary non-enforced subscription, editing still performs the normal subscription-to-fork replacement.

## Collision-derived projections

When two installed skills need the same Agent Skills invocation name, Denju exposes deterministic collision-safe derived projections. Those views are independently writable so an edit cannot mutate canonical bytes before Denju validates it.

Denju tracks the last semantic state shared by the canonical and derived views. An edit made only through the derived projection is validated, journaled, written into a fresh managed working generation, and switched atomically. A canonical-only edit regenerates the derived view. If both sides diverge independently, Denju pauses rather than guessing which content should win.

## Repair and recovery

Deleting or renaming a managed canonical skill directory by hand is not interpreted as deleting or renaming the Denju resource. Reconciliation restores the managed root from durable state. Remote materialization and collision-derived writeback both use crash-recoverable journaled switches so an interrupted process cannot expose a partially constructed skill.
