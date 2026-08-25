---
title: Local storage and projections
description: How SQLite, canonical managed skills, verified generations, native links, watchers, and collision aliases turn registry state into ordinary Agent Skill directories.
order: 33
category: Architecture
summary: "The local filesystem is a verified materialized view backed by durable SQLite state and immutable content."
---

The product contract users care about is simple: if Denju says a skill is active, Codex and Claude Code can discover that content as an ordinary Agent Skill.

Underneath, Denju deliberately separates durable local state from the harness-visible filesystem view.

## Canonical managed tree

Denju owns a canonical tree under `~/.denju`:

```text
~/.denju/
  state.db
  objects/
  generations/
  skills/
    <owner>/
      <skill>/
```

`state.db` is SQLite and records the installation, desired sources, materialized revisions, operations, conflicts, projection assignments, and queues.

Local immutable blobs are cached by content identity.

## Generations make remote updates atomic

Denju does not download a new release directly over the currently visible directory.

Instead it:

1. downloads missing content;
2. verifies hashes and the complete manifest;
3. constructs a new generation off-path;
4. verifies the completed generation;
5. switches the logical canonical skill to that generation;
6. marks the operation complete in the local journal.

If the process dies in the middle, recovery resumes or rolls back from the journal. An agent never needs to observe a directory that contains half the old release and half the new one.

## Native harness projections

Supported harness roots receive native filesystem projections of the canonical managed skill.

Denju chooses one active Codex projection root and one Claude Code root based on the configured environment. It does not scatter the same managed resource into multiple Codex roots and hope discovery deduplicates it.

If `CODEX_HOME` or `CLAUDE_CONFIG_DIR` changes, Denju treats that as a managed migration: build and validate the new projection first, then remove only the old Denju-managed links.

## Name collisions

Denju resources are scoped (`@alice/review`), while Agent Skills invocation names are not.

If `@alice/review` and `@bob/review` are both installed, the plain local name `review` is ambiguous. Denju assigns deterministic collision-safe aliases to the conflicting projections and uses the same alias in Codex and Claude Code on that device.

The projected directory name and the projected `SKILL.md` name continue matching the Agent Skills specification. Canonical resource identity is unchanged.

## Watchers are hints too

The background service uses native filesystem notifications where possible so local edits feel immediate. But editor save patterns, overflow, network filesystems, and process restarts make watcher events unreliable as authority.

Denju therefore maintains a SQLite-backed file index and can rescan the affected tree—or the complete skill when needed. Periodic verification and polling fallback keep correctness independent of one OS event stream.

## Writable projections and edit protection

Managed owned skills remain writable. A coherent valid save becomes a private revision.

If the user edits a subscribed upstream resource, Denju changes the **relationship** before it lets that edit become upstream state: the edit becomes a fork.

Collision-derived projections use a generated view so their synthetic local name does not rewrite the canonical package identity. Writeback is journaled and validated before a fresh canonical working generation becomes active.
