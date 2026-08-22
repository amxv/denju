---
title: Packs
description: Group exact skill requirements into versioned desired-state sources that subscribers can apply atomically.
order: 9
category: Start
summary: Packs are flat, versioned sets of skill resource IDs with exact resolved revisions and durable follow-latest behavior.
---

A pack is a flat desired-state source for several skills. It stores skill resource IDs and resolution intent, not copied skill bytes. Every pack version freezes the exact resolved RevisionId for every member, so old versions remain reproducible even after skills publish newer releases.

## Create and edit a pack

```bash
denju pack create @alice/packs/core
denju pack add @alice/packs/core @bob/review @carol/testing@v3
denju pack remove @alice/packs/core @bob/review
```

`pack add` and `pack remove` accept multiple skills in one mutation. The whole edit is atomic and, when it changes membership, creates exactly one next pack version. Repeating the same committed operation is idempotent.

Members without `@vN` follow the skill's latest immutable release when the pack is public. `@vN` pins that member to one exact release. Packs are deliberately flat: a pack may contain skills, never another pack.

A private personal pack can also contain private skills the owner is authorized to read, including the owner's current private workspace. Publishing the pack requires every member to be readable by the pack's public audience.

A team-private pack may contain public skills and private skills owned by that same team. A one-person private share is not sufficient because every pack member must be readable by the pack's full audience. Making a team pack public is stricter again: every member must be globally public.

## Publish and subscribe

```bash
denju publish @alice/packs/core
denju subscribe @alice/packs/core
```

Publishing changes pack visibility; it does not copy member content. Subscribing makes the pack one durable desired-state source for the installation or account. Denju downloads and verifies every changed member needed for the new desired state before switching any visible pack-managed skill, then commits the local pack application as one journaled parent operation.

Pack subscriptions always follow the live pack. They do not accept `--version` or `--retain-on-delete`. Exact reproducibility lives in immutable pack history; a live subscription is intentionally a moving desired-state relationship.

## Follow-latest history

When a followed skill creates a **new immutable release**, each dependent pack advances through one exact next pack version for that release event. Generic resource changes, proposal acceptance, private saves, metadata changes, and unchanged republishing do not advance packs.

Release fanout is durable and ordered rather than performed inside the skill-publish transaction. A publish request performs only bounded follow-latest draining; remaining work can resume from PostgreSQL after process termination through the deployment-neutral drain entrypoints. If releases arrive faster than draining, Denju preserves every release event in order rather than coalescing directly to the newest one.

## Degraded members

An authored member stays in the pack when it temporarily cannot be satisfied. `denju show @alice/packs/core` reports the pack as degraded and identifies the member reason, such as `unpublished`, `access_revoked`, or `deleted`. Denju removes only the unavailable pack-managed projection; if the same stable skill resource becomes available again, ordinary sync rematerializes it without editing the pack.

If two subscribed packs require different exact revisions of the same skill, neither silently wins. A first-install conflict exposes no pack-selected revision. If a valid pack-managed revision was already visible, Denju preserves that exact last-known-good projection and `denju status` lists the conflicting pack sources plus explicit `denju unsubscribe ...` resolution commands.

## Team-assigned packs

A team owner can enforce a readable pack for every current and future member:

```bash
denju team assign @acme @acme/packs/core
```

This is a distinct desired-state source from an ordinary pack subscription. A member cannot unsubscribe an enforced assignment. Team policy overrides weaker direct subscriptions and personal-pack requirements for the same immutable skill resource without deleting those weaker relationships, so `denju team unassign ...` or leaving the team can reactivate them automatically.

Assignments from different teams have equal authority. If two enforced packs require incompatible revisions of one resource, Denju pauses only that resource instead of using last-write-wins. Existing valid bytes stay on the last known-good revision; a first install exposes no winner. `denju status` names each team/pack source and gives source-specific unassignment commands.

Editing a skill currently supplied by an enforced pack creates a personal fork but does **not** replace team policy. Denju restores the required team revision and projects both it and the independent personal fork with deterministic collision-safe harness names. Ordinary non-enforced subscription edits keep the usual behavior of replacing the subscription with the fork.

## Rename, unpublish, and delete

```bash
denju rename @alice/packs/core core-tools
denju unpublish @alice/packs/core-tools
denju delete @alice/packs/core-tools
```

Rename preserves the pack's stable resource ID and keeps old-locator redirect behavior. Unpublish makes public-only subscriptions dormant: their pack-managed skills disappear on reconcile, while republishing the same pack reactivates the existing relationship.

Delete tombstones a personal pack and removes its subscription and team-assignment roots; packs do not support retain-on-delete. Recreating the same locator later creates a different resource ID and never reconnects subscriptions or assignments to the deleted pack. Deleting an entire team likewise removes its assigned-pack policy and tombstones its remaining team resources.
