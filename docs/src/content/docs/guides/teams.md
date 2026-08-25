---
title: Teams and assigned packs
description: Give an organization shared ownership and keep an approved set of skills synchronized across every team member's computers.
order: 15
category: Use Denju
summary: "Put skills in a pack, assign that pack to a team, and Denju keeps the approved set on everyone's machines."
---

Teams are how an organization shares ownership and keeps a common skill set synchronized. Those skills do not have to be public: team-owned skills and packs can stay private to the organization while Denju distributes them to the people who need them.

Imagine you run a legal organization. You have skills for contract review, legal research, citation checking, drafting client letters, and litigation workflows. You want every lawyer or engineer using agents in the organization to have the same approved skills without sending around folders or maintaining an onboarding checklist.

The Denju pattern is:

1. Put those skills in a pack, for example `@northstar/packs/legal-core`.
2. Assign that pack to the `@northstar` team.
3. Denju keeps the skills from that pack on every team member's computers.
4. Change the pack once; Denju adds, removes, or updates those skills for the team automatically.

A **team member is a person**. A **pack contains skills**. Assigning a pack connects those two concepts.

## Create a team

A team gets a shared Denju name that owns its skills and packs:

```bash
denju team create @northstar
```

The organization's skills and packs live under that name:

```text
@northstar/contract-review
@northstar/legal-research
@northstar/packs/legal-core
```

Invite a person as a normal member or maintainer:

```bash
denju team invite @northstar
denju team invite @northstar --role maintainer
```

Denju prints a one-time join code. The invited person runs:

```bash
denju team join <code>
```

Invite codes expire after 24 hours, are single-use, and can be revoked before use.

## Roles

Teams have three roles:

- **owner** — manages team policy, roles, ownership, and deletion;
- **maintainer** — edits and publishes team-owned skills and packs;
- **member** — uses team content and can submit proposals.

The owner can optionally allow every team member to publish:

```bash
denju team settings @northstar --members-can-publish true
```

There are no separate per-skill permission lists in v1. If a skill or pack is team-private, team membership is what grants access.

## Team skills are private by default

For a team-owned skill, the normal publish command creates a release for the team:

```bash
denju publish @northstar/contract-review
```

That release is **not globally public**. Team members can subscribe to it because their team membership grants access. If the skill is in an assigned pack, there is even less to manage: Denju installs and updates that private team skill across current members automatically, and future members receive it when they join.

Use `--public` only when the organization deliberately wants a team-owned skill to appear in the public registry.

This means a team can use Denju as its private skill distribution layer without also setting up Git cloning, shared folders, or another synchronization service.

## Build the team's skill pack

Create a pack owned by the team:

```bash
denju pack create @northstar/packs/legal-core
```

Add the skills the organization wants everyone to have:

```bash
denju pack add @northstar/packs/legal-core \
  @northstar/contract-review \
  @northstar/legal-research \
  @northstar/citation-check \
  @northstar/client-letter-draft
```

The pack is just that set of skills. You can add or remove skills later, and skills that follow latest will move forward when they publish new releases.

## Assign the pack to the team

This is what turns the pack into organization policy:

```bash
denju team assign @northstar @northstar/packs/legal-core
```

After assignment:

- every current team member receives the skills in the pack;
- a new person joining the team receives those skills as part of joining;
- adding a skill to the pack installs it for the team;
- removing a skill from the pack removes it when nothing else on that machine still needs it;
- a followed skill publishing a new release updates through the pack;
- team members cannot locally unsubscribe the assigned pack while the policy applies.

You maintain the pack. Denju maintains everyone's machines.

Remove the policy with:

```bash
denju team unassign @northstar @northstar/packs/legal-core
```

Removing the assignment removes only the team policy. If somebody also subscribed to one of those skills personally, that personal relationship is preserved and can become active again.

## Maintainer drafts stay private until a team release

A team can own the skills inside its packs, but maintainers do not edit one shared live draft.

Each authorized publisher works in a private working copy. Other team members continue using the latest team release until a maintainer publishes the next one.

That means an unfinished change to `@northstar/contract-review` on one maintainer's machine does not suddenly appear on every lawyer's computer. Publishing is the boundary that updates the **team-only release**, which can then flow through direct team subscriptions or the assigned pack.

If two maintainers edit from the same release, Denju uses its normal merge and conflict rules instead of silently overwriting one person's work.

## Editing a team-required skill

A team member can still experiment locally with a skill supplied by an assigned pack.

If they edit that team-required skill, Denju preserves the local change as a personal fork and restores the team-approved version. The person's experiment and the organization's required skill can coexist without changing team policy.

## Multiple teams can disagree

A person can belong to multiple teams. If two teams assign packs that require different exact revisions of the same skill, neither team silently wins.

Denju pauses only that skill, keeps the last valid version when possible, and shows both team policies in `denju status` with the commands that can resolve the disagreement.

## Move personal skills or packs into the organization

A personal skill or pack can move under the team without losing its identity or history:

```bash
denju transfer @alice/contract-review @northstar
denju transfer @alice/packs/legal-core @northstar
```

Transfer preserves its history, releases, subscriptions, forks, proposals, stars, and references from packs. Existing relationships keep following the same underlying skill or pack.

## Ownership succession

A team always has exactly one owner. The owner cannot simply leave and create an ownerless organization.

Start an ownership transfer:

```bash
denju team transfer-owner @northstar @alice
```

The recipient accepts the one-time code:

```bash
denju team accept-owner <code>
```

The former owner becomes a maintainer. An account that still owns a team cannot be deleted until ownership is transferred or the team itself is deleted.
