---
title: Private skills and publishing
description: Import an existing skill, keep it private across your devices, inspect history, optionally publish releases, and move skills into teams.
order: 11
category: Use Denju
summary: "Imported skills start private and sync across your signed-in devices; public publishing is an optional next step."
---

You can use this entire workflow without ever publishing publicly. Denju does not require a new package format; `SKILL.md` remains the agent-facing entry point.

## Import a skill

You must be signed in because import creates a skill owned by you or your team:

```bash
denju import ~/.agents/skills/my-skill
```

The directory name and `name` in `SKILL.md` must already match. Denju validates the complete skill before committing anything.

Import is intentionally a transfer into Denju management. Denju verifies and stores the skill, creates the managed local copy and harness links, then removes the original discovery path only after the Denju-managed copy is ready. If the registry is unavailable or validation fails, the source stays untouched.

The imported skill starts private under your Denju name, for example `@alice/my-skill`.

## Private sync works before you publish

Once the skill belongs to your account, Denju treats private synchronization as normal behavior—not as a separate feature you have to enable.

- valid saved changes become private history;
- those revisions synchronize to your other signed-in Denju devices;
- a newly signed-in device can receive your owned private skills along with your other account state;
- the skill remains absent from the public catalog until you explicitly publish it.

If all you want is one private skill kept current between a laptop and workstation, you can stop here. `denju publish` is not required.

You can also share that private skill with another Denju user without publishing it:

```bash
denju share @alice/my-skill @bob
```

This grants Bob private read/subscription access and prints the subscription command he can run. Sharing does not install the skill automatically and does not make it discoverable to everyone. If Bob subscribes, his copy follows your valid saved changes rather than waiting for public releases.

## Edit the managed skill normally

The managed skill remains an ordinary writable directory. Edit it through its Denju-managed path or through the copy your agent harness sees.

Denju waits for a complete save, validates the whole skill, and records a new private version when the meaningful content changed. Valid changes synchronize to your other signed-in devices. Invalid working content stays on the editing machine and pauses only that skill until you fix it.

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

## Publish a public release when you choose

```bash
denju publish @alice/my-skill
denju publish @alice/my-skill --message "Improve review workflow"
```

For a personally owned skill, publishing makes the current private head public as the next immutable release: `v1`, `v2`, `v3`, and so on. Consumers see releases, not every private save you made while working.

Published history is append-only. Correct a bad release by publishing a newer one rather than rewriting the old release.

## Team-owned skills can stay private to the team

Import directly into a team where you have publishing permission:

```bash
denju import ./my-skill --to @acme
```

Or move an existing personal skill into the team without losing its history or subscribers:

```bash
denju transfer @alice/my-skill @acme
```

Each authorized team publisher has a private working copy. Team members use released team versions; one maintainer's unfinished draft is not a shared live document.

Publish a team-only release:

```bash
denju publish @acme/my-skill
```

That command does **not** make the skill public. It creates the next release for the team. Authorized team members can subscribe to it, and an assigned team pack can keep that private team skill installed and current across the team's devices automatically.

This makes Denju useful as private team distribution even when the organization never wants its skills to appear in the public registry.

Make the team-owned skill globally public only when you want to:

```bash
denju publish @acme/my-skill --public
```

Once a team-owned skill is public, later releases remain public until `denju unpublish` returns it to team-only visibility.
