---
title: Discover and subscribe
description: Search the catalog, inspect resources, follow latest or pin a release, and understand what a subscription does locally.
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

Search combines the content you are allowed to know about: public resources, team resources, private shares, and your own local/owned state when authenticated. Search is metadata-only; Denju does not globally index skill instructions, scripts, or assets.

Inspect any visible resource with the universal `show` command:

```bash
denju show @alice/react-performance
denju show @alice/packs/core
denju show @alice
```

## Subscribe to a skill

```bash
denju subscribe @alice/react-performance
```

A direct subscription means: **this resource should remain installed on my machine**.

By default the subscription follows the skill's latest immutable public release. When the publisher releases a new version, Denju's background service normally reconciles it automatically.

You can force reconciliation at any time:

```bash
denju sync
```

## Pin an exact release

Use a pin when you deliberately need one immutable version:

```bash
denju subscribe @alice/react-performance --version 7
```

That subscription stays on `v7` even when `v8` is released. Run the ordinary subscribe command again to return to latest:

```bash
denju subscribe @alice/react-performance
```

## Retain a deleted skill

A direct subscription can opt into keeping the final release if the owner deletes the resource:

```bash
denju subscribe @alice/react-performance --retain-on-delete
```

This creates a frozen retained copy only after deletion. It does not apply to pack members, and security quarantine always overrides retention.

## Subscribe to a pack

A pack subscription looks the same:

```bash
denju subscribe @alice/packs/core
```

The difference is what the relationship means. The pack itself is the desired-state source, and Denju installs or removes its skill members as the pack changes. Packs do not accept direct `--version` or `--retain-on-delete` options.

## Unsubscribe

```bash
denju unsubscribe @alice/react-performance
denju unsubscribe @alice/packs/core
```

Denju removes a skill only when the final active source requiring it disappears. If the same resource is still required by another direct subscription, pack, owned workspace, or team assignment, it stays present.

## What gets installed locally?

Denju keeps a canonical managed version under `~/.denju/skills/...` and exposes it to the configured Codex and Claude Code skill roots. Those harness-facing paths are managed projections, not separate independent installations.

You normally do not need to care about that layout. The useful contract is simpler: if `denju status` says a skill is active, supported harnesses can discover the same content immediately.
