---
title: Sync, offline work, and conflicts
description: Understand the background service, one-shot reconciliation, offline queues, multi-device merges, and explicit conflict resolution.
order: 12
category: Use Denju
summary: "Denju synchronizes automatically, remains correct without the daemon, and preserves both sides when concurrent edits really conflict."
---

Most of the time, synchronization should be boring. The background service watches managed skills, receives registry wake hints, and keeps desired state current.

## Force a complete reconciliation

```bash
denju sync
```

`sync` is a one-shot operation. It settles currently known uploads, downloads, projection changes, pack changes, and removals, then exits.

If one resource is blocked by validation, a content conflict, or incompatible desired-state sources, Denju reports that exact resource instead of waiting forever. Unrelated skills can continue synchronizing.

## Work offline

Owned skill edits are recorded locally first. If the registry is offline, valid private revisions remain queued and upload when connectivity returns.

Likewise, a storage-quota problem does not block local editing. `denju usage` shows the namespace limit, current usage, and queued local bytes. Eligible unreleased private history can be pruned explicitly:

```bash
denju history prune @alice/my-skill
```

Denju never silently deletes history just because it is old.

## Invalid working content

If a managed skill becomes structurally invalid—for example, required frontmatter is missing—Denju does not overwrite your local work with a remote version. That skill enters a paused validation state. Fix the working tree and run `denju sync` again.

Changing the `name` field directly is treated as a pending rename because the skill name is package identity. Denju preserves your edit and tells you to run the explicit `denju rename ...` operation.

## Concurrent edits on two devices

Suppose two devices start from the same private revision and both save.

Denju never resolves that race by silently taking the last writer. Both immutable heads are preserved. The client then performs a three-way merge from the common parent.

- Changes to different files merge automatically.
- Non-overlapping edits in the same text file can merge automatically.
- Overlapping edits create an explicit conflict for that skill.

Check the conflict:

```bash
denju status
denju diff @alice/my-skill <head-a> <head-b>
```

To keep one complete head, use the exact `denju restore` command printed by status. To create a custom resolution, edit the preserved working tree and run:

```bash
denju sync
```

The resolved result becomes a new two-parent revision, so neither original head disappears from history.

## Desired-state conflicts are different

A content conflict means two people or devices edited the same skill history.

A desired-state conflict means two equally authoritative sources require different immutable revisions of the same resource—for example, two team assignments from two different teams.

Denju does not hide that disagreement by creating duplicate version aliases. It preserves the last valid visible revision, pauses only that resource, and shows the governing sources plus exact resolution commands in `denju status`.

## Repair the installation itself

Use `doctor` when the problem is the local Denju installation rather than one resource's desired state:

```bash
denju doctor
```

It checks local database health, the background service, stored credentials, harness roots, broken or duplicate Denju projections, registry connectivity, and interrupted local operations.
