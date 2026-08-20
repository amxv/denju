---
title: Anonymous use
description: Set up Denju, discover public Agent Skills, subscribe, synchronize, and remove managed skills without creating an account.
order: 2
category: Start
summary: Search and install public skills anonymously through verified, managed projections.
---

Denju does not require an account for public discovery or subscriptions. Setup creates an anonymous installation credential, records one registry, and prepares managed Codex and Claude projection roots.

## Set up the machine

```bash
denju setup
```

The official registry origin is `https://registry.denju.ashray.xyz`. A self-hosted or development installation chooses its registry once during setup:

```bash
denju setup --registry http://127.0.0.1:7788
```

## Find a public skill

Search returns bounded public metadata; skill bodies and scripts are not global search-index input.

```bash
denju search "review"
denju show @alice/review
```

Use `--json` on the same commands for the versioned machine-readable result envelope.

## Subscribe

```bash
denju subscribe @alice/review
```

Subscription state is stored by immutable resource ID. Denju downloads the published deterministic snapshot through short-lived object-store authorization, verifies its size, SHA-256, portable manifest, and Merkle root, builds a complete generation off-path, and only then switches the managed skill into view.

The canonical managed skill lives under:

```text
~/.denju/skills/<owner>/<skill>/
```

Denju projects that canonical content into both configured Codex and Claude roots using native links. It never silently falls back to independent copies.

If two installed packages expose the same Agent Skills name, both conflicting resources receive deterministic aliases. The alias is the same in Codex and Claude, remains within the Agent Skills 64-character limit, and matches the projected `SKILL.md` frontmatter name.

## Reconcile current state

```bash
denju sync
```

`sync` is a one-shot reconciliation. It reads the registry's current subscription state, repairs incomplete materialization work, verifies missing content, and settles projections/removals. Correctness does not depend on a warm registry process or a running local daemon.

## Unsubscribe

```bash
denju unsubscribe @alice/review
```

When the final active source disappears, Denju removes the managed canonical path and harness projections. Other unmanaged skills are never touched.
