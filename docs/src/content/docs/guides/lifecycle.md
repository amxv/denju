---
title: Manage skills and packs
description: Rename, unpublish, deprecate, delete, transfer, export, and clean up private history without surprising existing subscribers.
order: 17
category: Use Denju
summary: "Rename or move skills and packs without breaking the people already using them."
---

Once you publish a skill or pack, other people may depend on it. Denju is designed so ordinary maintenance does not accidentally break those relationships.

The useful rule is: **Denju follows the skill or pack itself, not just the text of its current name.**

That is why you can rename or move something into a team and existing subscriptions keep working. You usually do not need to think about Denju's internal IDs to use this safely.

## Rename a skill or pack

```bash
denju rename @alice/review code-review
```

The same skill is now available as `@alice/code-review`. Its history, subscribers, shares, stars, forks, proposals, and references from packs stay attached.

The old name continues to point to the renamed skill while that name remains unused. If you deliberately create something new with the old name later, the new thing is separate and does not inherit the old relationships.

For a published skill, rename also creates the next release so the package name and `SKILL.md` name continue to match.

The same command works for packs:

```bash
denju rename @alice/packs/core legal-core
```

## Make something private again

```bash
denju unpublish @alice/code-review
```

Unpublish removes public access without deleting the skill or its release history.

People whose only access was through the public registry lose the installed skill on their next sync. Their subscription is still remembered against that same skill, so publishing it again can reactivate the relationship.

For a team-owned skill or pack, unpublishing removes global public access but the team can continue using it privately.

## Mark an old skill as replaced

When a public skill still works but you want new users to move elsewhere, deprecate it:

```bash
denju deprecate @alice/code-review --replacement @alice/review-v2
```

Existing subscribers keep receiving updates. Search and `show` make it clear that the skill is deprecated and point to the replacement.

Undo the deprecation with:

```bash
denju deprecate @alice/code-review --undo
```

Deprecation does not publish a new skill release by itself.

## Delete a skill or pack

```bash
denju delete @alice/code-review
```

Delete removes the skill or pack from active use. Denju removes managed copies for people who no longer have a reason to keep it, while preserving published history on the registry where it is needed for integrity and existing retention rules.

The old name becomes available again. If you later create a new `@alice/code-review`, Denju treats it as a new skill: old subscriptions, stars, shares, and history do not jump to the replacement just because the text of the name matches.

## Keep a deleted skill on purpose

A **direct skill subscription** can opt into retaining the final published version if the owner later deletes the skill:

```bash
denju subscribe @alice/code-review --retain-on-delete
```

Configure this before deletion.

This applies only to a direct skill subscription. If the skill was installed only because a pack contained it, pack changes and deletion follow the pack normally. Security quarantine also overrides retention.

## Move a personal skill or pack into a team

```bash
denju transfer @alice/code-review @acme
denju transfer @alice/packs/core @acme
```

Transfer is a move, not a clone. The skill or pack keeps its history and existing relationships while ownership changes from your personal account to the team.

For example, someone already subscribed to `@alice/code-review` continues following the same underlying skill after it becomes `@acme/code-review`.

Transfer is useful when a personal skill grows into organization-owned infrastructure.

## Export an ordinary folder

Export creates an unmanaged copy outside Denju:

```bash
denju export @alice/code-review ./copy
denju export @alice/code-review@v7 ./v7-copy
```

Use this when you want plain files rather than an ongoing Denju relationship.

A helpful distinction is:

- **subscribe** — keep following a Denju skill;
- **fork** — make an independent Denju skill based on another one;
- **export** — make an ordinary directory that Denju no longer manages.

## Check storage and clean up private history

```bash
denju usage
denju history prune @alice/code-review
```

`denju usage` shows the storage policy advertised by your registry, how much you are using, and any local work waiting to upload.

`history prune` removes only private save history that is safe to discard and always requires explicit approval. Published releases, pinned versions, proposal history, forks, and conflict history are not silently deleted just because they are old.
