---
title: Resource lifecycle and history
description: Rename, unpublish, deprecate, delete, retain, transfer, export, inspect usage, and prune eligible private history without losing stable identity.
order: 17
category: Use Denju
summary: "Resource identity survives locator changes; lifecycle operations are explicit and published history remains immutable."
---

A Denju skill or pack has a stable internal resource identity. A locator such as `@alice/review` is its human-facing name, not the identity itself.

That distinction lets Denju rename or transfer a resource without disconnecting its existing relationships.

## Rename

```bash
denju rename @alice/review code-review
```

The resource keeps its history, subscriptions, shares, stars, fork provenance, and pack references. The old locator redirects while unused.

For a released skill, rename creates the next immutable release so the published `SKILL.md` name and package locator remain consistent.

## Unpublish

```bash
denju unpublish @alice/code-review
```

Unpublish removes global visibility without deleting the resource or immutable versions. Public-only subscribers lose access on reconciliation, but their subscription relationship stays attached to the same stable ID and can reactivate if that resource is republished.

A team resource returns to team-only visibility.

## Deprecate

```bash
denju deprecate @alice/code-review --replacement @alice/review-v2
denju deprecate @alice/code-review --undo
```

Deprecation is reversible metadata. Existing subscribers continue updating, while discovery surfaces demote the old resource and show the replacement.

## Delete

```bash
denju delete @alice/code-review
```

Deletion tombstones the resource and removes active managed state. Published history remains immutable where required for integrity, policy, or retained subscriptions.

The old locator can later be reused, but a newly created resource receives a different stable identity and inherits none of the old relationships.

## Retain the final release after deletion

Configure this **before** the resource is deleted:

```bash
denju subscribe @alice/code-review --retain-on-delete
```

That direct subscription can keep the deleted resource's final immutable release frozen locally. Pack members cannot use retention, and security quarantine overrides it.

## Transfer into a team

```bash
denju transfer @alice/code-review @acme
```

Transfer changes the owning namespace without cloning the resource. Existing subscriptions and history continue following the same stable identity.

## Export an unmanaged copy

```bash
denju export @alice/code-review ./copy
denju export @alice/code-review@v7 ./v7-copy
```

Export is the escape hatch to an ordinary directory. Denju does not need a separate clone primitive: subscribe is a managed follower, fork is an independent Denju resource, export is an unmanaged copy.

## Usage and private-history pruning

```bash
denju usage
denju history prune @alice/code-review
```

The registry reports its own namespace limits; clients do not hard-code hosted-service quotas. Pruning is explicit and limited to eligible unreleased private-save history. Published, pinned, proposal, fork, conflict, and other protected revisions are not silently removed.
