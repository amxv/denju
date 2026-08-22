---
title: Teams
description: Create shared namespaces, enforce approved packs, publish through private maintainer refs, and manage team ownership safely.
order: 7
category: Start
summary: Shared ownership, enforced pack policy, and explicit succession without shared live drafts.
---

Teams use the same global namespace as users. A name such as `@acme` can belong to exactly one user or one team, and team-owned skills and packs use ordinary locators such as `@acme/review` and `@acme/packs/core`.

## Create and join

```bash
denju team create @acme
denju team invite @acme --role maintainer
# send the printed one-time command to the recipient
denju team join <invite-code>

denju team
denju team show @acme
```

Invite codes are bearer secrets. They expire after 24 hours, are single-use, and can be revoked before use with `denju team invite-revoke @acme <invite-id>`.

## Roles and publishing

Teams have exactly three roles: `owner`, `maintainer`, and `member`. There is exactly one owner. Owners can invite maintainers or members, change roles, remove non-owner members, and change team settings. Maintainers can invite members. Members read team content by default.

Owners can opt ordinary members into publishing:

```bash
denju team settings @acme --members-can-publish true
```

When enabled, members use the same private-workspace publishing model as maintainers. This is a team-wide policy; Denju does not add per-skill team ACLs.

Owner-only membership changes use:

```bash
denju team role @acme @alice maintainer
denju team remove @acme @alice
```

The owner cannot leave the team or delete their account while they still own it. Ownership succession is an explicit two-person handoff:

```bash
denju team transfer-owner @acme @alice
# the recipient runs the printed one-time command
denju team accept-owner <transfer-code>
```

The nominated existing member becomes owner only after accepting the code; the former owner becomes a maintainer. Creating the transfer does not change authority by itself.

Non-owners can leave with `denju team leave @acme`. Removing or leaving a team immediately withdraws team-assigned pack requirements and private team subscriptions on reconciliation; unrelated public direct subscriptions and personal forks remain.

Deleting the entire team is owner-only, always requires hidden password input, and applies normal tombstone semantics to all remaining team resources:

```bash
denju team delete @acme
```

The namespace becomes reusable only after deletion. Recreating `@acme` creates a new internal team ID and inherits no old memberships, assignments, resources, or other relationships.

## Enforced packs

Owners can make a pack required for every current and future team member:

```bash
denju team assign @acme @acme/packs/core
denju team unassign @acme @acme/packs/core
```

An assigned pack may be team-owned or public. Assignment is team policy, not a subscription performed on behalf of each member: members cannot unsubscribe it locally, and joining the team applies the requirement on the same reconciliation path as existing members.

Team policy is stronger than personal desired-state sources. If the same skill is also present through a direct subscription or personal pack, Denju keeps that weaker relationship intact but suppresses it while the enforced requirement exists. Removing the assignment or leaving the team automatically reactivates the personal source when it is still valid.

Two teams are equally authoritative. If they require different exact revisions of the same skill, Denju does not choose a winner. It preserves the last valid visible revision, or exposes no revision on a first-install conflict, while `denju status` identifies the conflicting team/pack sources and the exact `denju team unassign ...` recovery commands. Unrelated skills continue synchronizing.

## Private maintainer refs

Team ownership never means a shared live draft. Each authorized publisher gets a private workspace ref for each team skill they edit. Their local tree can diverge from every other maintainer without exposing those bytes to the team.

The latest immutable team release is the shared read surface. Publishing reconciles the caller's known private head with that release. Compatible concurrent changes merge deterministically; conflicting edits preserve both immutable heads and pause only that publisher's workspace for explicit resolution.

An unpublished first import is visible only to the importer. Another publisher gets a workspace only after a team release exists, and that workspace is seeded from the release rather than from somebody else's draft.

## Publish privately or publicly

```bash
denju publish @acme/review
denju publish @acme/review --public
```

The default team release is team-private. Current team members can discover and subscribe to it, while non-members cannot. `--public` makes the resource globally public. Public visibility is sticky across later publishes until an explicit `denju unpublish` returns it to team-private access.

Search follows the same boundary: a publisher sees their own private draft, another team member sees the latest team release, and outsiders see only globally public releases.

## Import and transfer

Create directly in the team:

```bash
denju import ./review --to @acme
denju pack create @acme/packs/core
```

Or move an existing personal resource without changing its identity:

```bash
denju transfer @alice/review @acme
denju transfer @alice/packs/core @acme
```

Transfer preserves the stable resource ID and all relationships attached to it. Old locators redirect to the team locator. Destination collisions and storage-quota failures are atomic: neither can leave a partially transferred resource.

For a transferred skill, the transferor keeps their current content as their private team workspace. Existing subscribers, forks, proposals, pack references, and private-share relationships continue by resource ID. A pre-existing private share becomes release-only after transfer, so it can never expose future maintainer drafts.
