---
title: Teams and enforced packs
description: Create a shared namespace, manage roles and private maintainer workspaces, and keep every member on the same approved skill set.
order: 15
category: Use Denju
summary: "Teams provide shared ownership; assigned packs turn a curated skill set into continuously enforced desired state."
---

Teams give an organization a shared Denju namespace such as `@acme`. Team-owned skills and packs use ordinary resource locators:

```text
@acme/review
@acme/packs/core
```

## Create a team

```bash
denju team create @acme
```

Invite a member or maintainer:

```bash
denju team invite @acme
denju team invite @acme --role maintainer
```

Denju prints a one-time join code. The recipient runs:

```bash
denju team join <code>
```

Invite codes expire after 24 hours, are single-use, and can be revoked before use.

## Roles

Teams have exactly three roles:

- **owner** — manages team policy, roles, ownership, and deletion;
- **maintainer** — edits and publishes team skills and packs;
- **member** — consumes team content and can submit proposals.

The owner can optionally let all members publish:

```bash
denju team settings @acme --members-can-publish true
```

Denju deliberately avoids per-skill team ACL complexity in v1.

## Team skill work is private until release

Each authorized publisher edits a private workspace for a team skill. There is no shared live draft.

When somebody publishes, the latest immutable team release becomes the shared read surface. If two maintainers started from the same release and both publish, Denju uses the ordinary merge/conflict rules instead of overwriting one person's work.

## Assign a pack to everyone

This is the main team-policy feature:

```bash
denju team assign @acme @acme/packs/core
```

Every current member receives the pack. Future members receive it when they join. Updates to the pack reconcile for the team automatically.

A member cannot unsubscribe an enforced assignment locally.

Team policy is stronger than personal sources for the same resource, but Denju does not delete the user's weaker direct subscription or personal-pack requirement. It suppresses that requirement while policy applies, then reactivates it automatically if the policy disappears.

Remove the assignment:

```bash
denju team unassign @acme @acme/packs/core
```

## Editing an enforced skill

If a member edits a skill supplied by team policy, Denju does not let that local edit displace the enforced revision. The edit becomes a personal fork, and Denju restores the required team version. Both can coexist using collision-safe local names when necessary.

## Two teams can disagree

Assignments from different teams have equal authority. If two teams require incompatible revisions of the same stable skill, Denju does not invent a winner. Only that skill pauses; `denju status` names both sources and the exact commands that can resolve the disagreement.

## Move a personal resource into a team

```bash
denju transfer @alice/review @acme
denju transfer @alice/packs/core @acme
```

Transfer preserves the resource's stable identity and its history, releases, subscriptions, provenance, proposals, stars, and pack references. The old locator redirects while it remains unused.

## Ownership succession

The current owner cannot simply leave and create an ownerless team. Transfer ownership explicitly:

```bash
denju team transfer-owner @acme @alice
```

The recipient accepts the printed code:

```bash
denju team accept-owner <code>
```

The old owner becomes a maintainer. An account that still owns a team cannot be deleted.
