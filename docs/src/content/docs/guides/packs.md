---
title: Packs
description: Build reusable, reproducible skill sets that follow latest releases or pin exact versions and can later become team policy.
order: 14
category: Use Denju
summary: "A pack is a flat versioned desired-state set: one subscription can keep several exact skill requirements current together."
---

A pack is a versioned set of skill requirements. It does not copy skill content and it is not a recursive dependency graph.

Use packs when you want to say: **these skills belong together**.

## Create a pack

```bash
denju pack create @alice/packs/core
```

Add several skills atomically:

```bash
denju pack add @alice/packs/core @owner/review @owner/testing @owner/security
```

Pin one member by using its release locator:

```bash
denju pack add @alice/packs/core @owner/testing@v3
```

Remove members:

```bash
denju pack remove @alice/packs/core @owner/security
```

A changed add/remove operation creates one next immutable pack version. Repeating an already-satisfied operation is a no-op.

## Follow latest versus pin

A member written as `@owner/skill` follows that skill's new immutable releases.

A member written as `@owner/skill@v3` stays on exactly `v3`.

Every immutable pack version records the exact resolved revision of every member, even for follow-latest entries. Historical pack versions therefore remain reproducible.

## Publish and subscribe

```bash
denju publish @alice/packs/core
denju subscribe @alice/packs/core
```

A live pack subscription follows the pack as it changes. Before switching visible state, Denju downloads and verifies the changed members required for the complete new pack state.

Packs do not support `--retain-on-delete`. If a pack stops requiring a skill and no other source requires it, Denju removes that managed skill.

## Private and team packs

A private personal pack can contain skills the owner is allowed to read.

A team-private pack must be readable by the **whole team**, not only one maintainer. That means it can use public skills and team-readable skills, but not a personal resource shared privately with only one member.

Making a pack public requires every member to be public.

## Degraded packs

Denju does not silently rewrite a pack if one authored member becomes unavailable. The pack becomes degraded and reports why—for example `unpublished`, `deleted`, `access_revoked`, or `quarantined`.

If access to the same stable resource returns, ordinary synchronization can materialize it again. Otherwise, a maintainer edits the pack to remove or replace the unavailable member.

## Pack conflicts

If two ordinary packs require different exact revisions of the same skill, neither silently wins. Denju preserves the last valid visible revision when one exists and reports the conflicting pack sources in `denju status`.

Team-assigned packs add a stronger policy layer. See [Teams](/docs/guides/teams).
