---
title: Resource lifecycle
description: Rename, publish, unpublish, deprecate, delete, retain, prune, and inspect storage for owned skills.
order: 11
category: Start
summary: Stable resource IDs keep lifecycle mutations precise even when locators change or are reused.
---

Denju treats a skill's internal resource ID as its identity. A locator such as `@alice/review` is a human-facing name that can change or later be reused by a different resource.

## Rename

```bash
denju rename @alice/review code-review
```

Rename keeps the same resource ID. Denju creates a real revision whose `SKILL.md` declares the new name and moves the local canonical/projection paths through a recoverable journal. If the skill is public, that rename revision becomes the next immutable release; the unused old locator redirects to the renamed resource.

If you edit the `name` field directly first, Denju preserves those working bytes and reports the matching explicit rename command instead of silently changing registry identity. Running that command stages the complete working tree, verifies its object proofs, and makes those exact edits part of the rename revision/release; it does not publish the name change first and upload the rest afterward.

For a team skill, rename rewrites every publisher's current private ref independently so divergent drafts stay private. If the skill has a release, the renamed release is rebuilt from that immutable release snapshot rather than from any maintainer workspace; renaming can never publish somebody's unrelated pending edits.

## Unpublish and republish

```bash
denju unpublish @alice/code-review
denju publish @alice/code-review
```

Unpublish removes public access without deleting history or direct subscription rows. Public-only subscribers remove the local materialization on their next reconcile. The dormant subscription remains tied to the same resource ID, so republishing that resource reactivates it without creating a new relationship.

If no unpublished content changed, republish reactivates the existing latest immutable release instead of fabricating a duplicate version.

## Deprecate

```bash
denju deprecate @alice/code-review --replacement @alice/review-v2
denju deprecate @alice/code-review --undo
```

Deprecation is reversible metadata, not a release. Deprecated skills remain installable, but search demotes them and `search`, `show`, and `subscribe` surface replacement guidance when one is configured.

## Retain a direct subscription after deletion

```bash
denju subscribe @alice/code-review --retain-on-delete
```

Retention is opt-in per direct subscription. If the owner later deletes the resource, a retained subscription freezes on that resource's final immutable release. An ordinary subscription is removed. Retention follows the immutable resource ID, never locator text, so a new skill that later reuses `@alice/code-review` is unrelated.

Security quarantine is stronger than retention. If the retained tombstone release or whole resource is quarantined, Denju preserves the local bytes under `~/.denju/quarantine/` for inspection and removes the active projection instead of continuing to expose the retained copy.

## Delete

```bash
denju delete @alice/code-review
# non-interactive prior authorization
denju delete @alice/code-review --yes
```

Delete tombstones the resource and removes active canonical/projection state. Published history needed for integrity and retained direct subscriptions remains immutable in the registry. Redirects targeting the deleted resource are removed, and its locator becomes available for a new resource with a new resource ID.

Team resource deletion is intentionally blocked until the owner-only team deletion and ownership-succession rules are implemented. Team rename, publish, unpublish, and deprecation already use team publishing authority.

## Transfer into a team

```bash
denju transfer @alice/code-review @acme
denju transfer @alice/packs/core @acme
```

Transfer is an ownership mutation, not a copy. It keeps the same resource ID, name, revisions, releases, visibility, subscriptions, forks, proposals, provenance, stars, and pack references while changing the owning namespace. The old personal locator redirects to the new team locator. A destination name collision or destination storage-quota failure aborts the whole transfer without changing ownership or creating a redirect.

For skills, the transferor's current personal workspace becomes only that user's first private team workspace. Other publishers are seeded from the latest immutable release, never from the transferor's unpublished draft. Existing one-person shares survive as release-only access after the transfer.

## Storage usage and private-history pruning

```bash
denju usage
denju history prune @alice/code-review
denju history prune @alice/code-review --yes
```

`usage` reports logical namespace storage, remaining capacity, prunable private history, and queued local bytes. `history prune` is explicit and only removes eligible unreleased private revisions that are no longer the current private workspace head. Published/protected history is never silently pruned to recover quota.

Pruning is also the recovery path when a new local revision is already queued because storage is full. The prune invalidates stale prepared uploads at the old generation, rebases only the queued revisions' generation expectations, and retries the same queued revision identities after capacity is reclaimed. Your local edit is not rewritten just to recover quota.

Canonical objects that become unreachable are only eligible for physical deletion after the registry's GC grace period and a fresh authoritative reachability check.
