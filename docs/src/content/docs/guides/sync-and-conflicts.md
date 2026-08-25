---
title: Sync, offline work, and conflicts
description: Understand automatic sync, one-shot sync, offline edits, multi-device merges, team-policy disagreements, and conflict resolution.
order: 12
category: Use Denju
summary: "Denju synchronizes automatically, remains correct without the daemon, and preserves both sides when concurrent edits really conflict."
---

Most of the time, synchronization should be boring. The background service watches your managed skills and keeps the versions you should have installed current.

## Force a complete sync

```bash
denju sync
```

`sync` is a one-shot check of everything Denju currently knows about. It uploads pending edits, downloads updates, applies pack changes, repairs managed harness links, removes skills that are no longer needed, then exits.

If one skill is blocked by validation, an edit conflict, or two policies asking for different versions, Denju reports that skill instead of waiting forever. Unrelated skills can continue synchronizing.

## Work offline

Owned skill edits are recorded locally first. If the registry is offline, valid private revisions remain queued and upload when connectivity returns.

Likewise, a storage-quota problem does not block local editing. `denju usage` shows the applicable storage limit, current usage, and queued local bytes. Eligible unreleased private history can be pruned explicitly:

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

## When two install policies disagree

A content conflict means two people or devices edited the same skill history.

A policy conflict means two equally strong rules ask Denju to install different exact versions of the same skill—for example, two teams assign packs that pin that skill differently.

Denju does not guess which team should win. It keeps the last valid version visible when possible, pauses only that skill, and shows both policies plus exact resolution commands in `denju status`.

## Repair the installation itself

Use `doctor` when the problem is the Denju installation itself rather than one skill or policy:

```bash
denju doctor
```

It checks local database health, the background service, stored credentials, Codex/Claude skill locations, broken or duplicate Denju links, registry connectivity, and interrupted local work.
