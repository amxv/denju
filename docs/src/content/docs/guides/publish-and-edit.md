---
title: Publish and edit skills
description: Import an existing skill, work privately across devices, inspect history, publish releases, and move skills into teams.
order: 11
category: Use Denju
summary: "Bring a skill under Denju management, edit it normally, and publish only the revisions you want consumers to receive."
---

Publishing starts with an existing Agent Skill directory. Denju does not require a new package format; `SKILL.md` remains the agent-facing entry point.

## Import a skill

You must be signed in because import creates a skill owned by you or your team:

```bash
denju import ~/.agents/skills/my-skill
```

The directory name and `name` in `SKILL.md` must already match. Denju validates the complete skill before committing anything.

Import is intentionally a transfer into Denju management. Denju verifies and stores the skill, creates the managed local copy and harness links, then removes the original discovery path only after the Denju-managed copy is ready. If the registry is unavailable or validation fails, the source stays untouched.

The imported skill starts private under your Denju name, for example `@alice/my-skill`.

## Edit the managed skill normally

The managed skill remains an ordinary writable directory. Edit it through its Denju-managed path or through the copy your agent harness sees.

Denju groups a coherent save, validates the complete skill, and creates a private immutable revision when the semantic content changed. Valid revisions synchronize to your other authenticated devices. Invalid working content stays on the editing machine and pauses only that skill until you fix it.

Check state with:

```bash
denju status
```

## Inspect history

```bash
denju history @alice/my-skill
denju diff @alice/my-skill
denju diff @alice/my-skill v1 v3
```

Restore does not rewrite history. It takes an older revision and makes a new private revision from it:

```bash
denju restore @alice/my-skill <revision>
```

Export creates an ordinary unmanaged copy:

```bash
denju export @alice/my-skill ./my-skill-copy
denju export @alice/my-skill@v3 ./my-skill-v3
```

## Publish a release

```bash
denju publish @alice/my-skill
denju publish @alice/my-skill --message "Improve review workflow"
```

The current private head becomes the next immutable release: `v1`, `v2`, `v3`, and so on. Consumers see releases, not every private save you made while working.

Published history is append-only. Correct a bad release by publishing a newer one rather than rewriting the old release.

## Team-owned skills

Import directly into a team where you have publishing permission:

```bash
denju import ./my-skill --to @acme
```

Or move an existing personal skill into the team without losing its history or subscribers:

```bash
denju transfer @alice/my-skill @acme
```

Each authorized team publisher has a private workspace. Team members consume immutable team releases; one maintainer's unfinished draft is not a shared live branch.

Publish a team-only release:

```bash
denju publish @acme/my-skill
```

Make the team-owned skill globally public:

```bash
denju publish @acme/my-skill --public
```

Once a team-owned skill is public, later releases remain public until `denju unpublish` returns it to team-only visibility.
