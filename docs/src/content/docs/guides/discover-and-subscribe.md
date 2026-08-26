---
title: Discover and subscribe
description: Search the catalog, inspect skills and packs, follow latest or pin a release, and understand what a subscription does locally.
order: 10
category: Use Denju
summary: "Find a skill, make one subscription relationship, and let Denju keep the managed local copy current."
---

The fastest way to get value from Denju is to use somebody else's public skill. No account is required.

## Search the catalog

```bash
denju search "react performance"
denju search "testing" --sort stars
denju search "rust" --topic agent-infra
```

Search combines the things you are allowed to see: public skills and packs, team content, private shares, and your own work when signed in. Search is metadata-only; Denju does not globally index skill instructions, scripts, or assets.

Inspect a skill, pack, or user with the same `show` command:

```bash
denju show @alice/react-performance
denju show @alice/packs/core
denju show @alice
```

## Subscribe to a skill

```bash
denju subscribe @alice/react-performance
```

A direct subscription means: **keep this skill installed on my machine**.

By default the subscription follows the skill's latest published release. When the publisher releases a new version, Denju's background service normally updates the installed skill automatically.

You can ask Denju to check and apply all current changes at any time:

```bash
denju sync
```

## Pin an exact release

Use a pin when you deliberately need one exact published version:

```bash
denju subscribe @alice/react-performance --version 7
```

That subscription stays on `v7` even when `v8` is released. Run the ordinary subscribe command again to return to latest:

```bash
denju subscribe @alice/react-performance
```

## Retain a deleted skill

A direct skill subscription can opt into keeping the final release if the owner deletes the skill:

```bash
denju subscribe @alice/react-performance --retain-on-delete
```

This creates a frozen retained copy only after deletion. Retention applies only to a direct skill subscription, not to skills installed because a pack requires them, and security quarantine always overrides retention.

## Subscribe to a pack

A pack subscription looks the same:

```bash
denju subscribe @alice/packs/core
```

A pack is simply a set of skills. Subscribing to the pack means Denju keeps that set current: skills added to the pack are installed, removed skills are removed when nothing else requires them, and followed skills advance with new releases. Packs do not accept direct `--version` or `--retain-on-delete` options.

## Unsubscribe

```bash
denju unsubscribe @alice/react-performance
denju unsubscribe @alice/packs/core
```

Denju removes a skill only when nothing else still needs it. If you also subscribe to the skill directly, another pack contains it, you own it, or a team-assigned pack requires it, the skill stays installed.

## What gets installed locally?

Denju keeps one managed copy under `~/.denju/skills/...` and projects that copy into the shared `~/.agents/skills` root plus the configured Claude Code skills directory. They are views of the same Denju-managed skill, not separate installations you have to keep in sync yourself.

You normally do not need to care about that layout. The useful contract is simpler: if `denju status` says a skill is active, supported harnesses can discover the same content immediately.
