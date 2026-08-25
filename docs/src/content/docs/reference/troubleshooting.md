---
title: Troubleshooting
description: Diagnose setup, registry, synchronization, validation, quota, conflict, projection, and upgrade problems with the smallest useful Denju commands.
order: 42
category: Reference
summary: "Start with status for resource blockers and doctor for installation problems; Denju preserves local work instead of repairing destructively."
---

Two commands answer different questions:

```bash
denju status
denju doctor
```

Use **status** when a skill, pack, or desired-state source is blocked. Use **doctor** when the Denju installation, background service, credential store, registry connection, or harness projections may be unhealthy.

## “Setup required”

Run:

```bash
denju setup
```

If you intend to use a self-hosted registry, include it on the initial setup command:

```bash
denju setup --registry https://denju.example.com
```

An existing installation is intentionally bound to one registry in v1.

## Registry unavailable

Check network reachability and the registry's readiness endpoint. Your owned valid local edits remain queued; Denju does not discard them because the server is offline.

When connectivity returns:

```bash
denju sync
```

## A skill is paused after editing

Run:

```bash
denju status
```

Common reasons:

- the working tree is temporarily invalid;
- `SKILL.md` name was edited directly and needs explicit `denju rename`;
- two devices created overlapping edits;
- a fork needs explicit upstream synchronization/resolution;
- two desired-state sources require incompatible revisions.

Denju preserves the local working content in these cases rather than overwriting it with remote state.

## A managed skill disappeared

Check its sources:

```bash
denju status
```

A direct subscription may have been removed, a pack may no longer require the resource, team membership/assignment may have changed, access may have been revoked, or the resource may have been unpublished/deleted/quarantined.

If no active source requires a skill, removal is expected behavior.

## A harness does not see a managed skill

Run:

```bash
denju doctor
```

Doctor checks the recorded Codex and Claude Code roots, broken Denju-managed links, stale/duplicate projections, interrupted migrations, and service state. It repairs Denju-owned projection state without treating unrelated user skills as disposable.

If your `CODEX_HOME` or `CLAUDE_CONFIG_DIR` changed, invoking Denju should migrate its managed projection after validating the new location.

## Storage quota exceeded

Inspect usage:

```bash
denju usage
```

Local editing can continue. Eligible unreleased private history can be pruned explicitly:

```bash
denju history prune @you/skill
```

Published and otherwise protected history is not silently deleted.

## A subscribed skill changed after I edited it

Editing an ordinary upstream subscription should create a fork rather than mutate the upstream resource.

Inspect:

```bash
denju status
```

If upstream has advanced, synchronize deliberately:

```bash
denju fork sync @you/skill
```

## Team policy is fighting my personal version

Team enforcement is stronger than personal desired state but does not erase the personal relationship.

`denju status` shows which source currently governs the skill. If two teams require incompatible revisions, neither wins silently; resolve the governing team assignment instead.

## Upgrade failed

Denju's upgrade path stages and verifies the new executable, updates/restarts the background service, and runs a health probe. A failed health check rolls the installation back to the previous binary/package version.

After any interrupted or unusual upgrade state:

```bash
denju doctor
```

## Need machine-readable diagnostics?

Add `--json`:

```bash
denju --json status
denju --json doctor
```

See [Automation and JSON output](/docs/reference/automation-json) for stable envelope and error-code behavior.
