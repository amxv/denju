---
title: Quickstart
description: A short end-to-end tour from anonymous discovery to private multi-device skill sync, optional publishing, and reusable packs.
order: 3
category: Start
summary: "Search and subscribe anonymously, then bring in a private skill and let Denju keep it current across your devices."
---

This quickstart shows the most common Denju journey. You can stop after the first section if all you want is to use other people's public skills.

## 1. Search and subscribe anonymously

```bash
denju search "code review"
denju show @owner/review
denju subscribe @owner/review
```

That subscription now keeps the skill installed on this machine. Denju follows the skill's latest published release by default.

Check what Denju is managing:

```bash
denju status
```

## 2. Pin a release when you need stability

Most subscriptions should follow latest. When you deliberately need an exact immutable version:

```bash
denju subscribe @owner/review --version 3
```

Subscribe again without `--version` to return to following latest:

```bash
denju subscribe @owner/review
```

## 3. Claim an identity when you want private account sync

```bash
denju claim @alice
```

Denju prompts for a password and displays a recovery secret once. Save that recovery secret somewhere separate from the machine.

Your existing anonymous subscriptions become account state. Your identity also gives Denju somewhere to attach owned private skills, so those skills can follow you to your other signed-in devices.

## 4. Import one of your existing skills

Suppose you already have:

```text
~/.agents/skills/my-skill/
  SKILL.md
  scripts/
  references/
```

Import it:

```bash
denju import ~/.agents/skills/my-skill
```

Import moves the skill under Denju management rather than making a second loose copy. Denju validates and stores the skill first; only after the managed version is ready for your agent harnesses does it remove the original discovery path.

The imported skill starts private at `@alice/my-skill`.

## 5. Let the private skill follow you

Open the managed skill through its Denju path or the normal skill location your agent sees and edit files as usual. Denju records valid saved changes in private history and synchronizes them to your other signed-in devices.

That is already a complete Denju workflow. You can keep `@alice/my-skill` private forever and use Denju purely to keep your own Agent Skills synchronized across machines. There is no separate private-sync command or configuration.

Inspect history:

```bash
denju history @alice/my-skill
denju diff @alice/my-skill
```

## 6. Publish only when you want other people to use it

```bash
denju publish @alice/my-skill --message "Initial release"
```

Publishing is optional. The first public personal release is `v1`. Later publishes create `v2`, `v3`, and so on. Published history is immutable; fix a bad release by publishing a new one.

## 7. Put related skills in a pack

```bash
denju pack create @alice/packs/core
denju pack add @alice/packs/core @alice/my-skill @owner/review
denju publish @alice/packs/core
```

Someone else can now subscribe to the pack:

```bash
denju subscribe @alice/packs/core
```

That one pack subscription now keeps the whole set of skills current. Each immutable pack version records the exact skill revisions used at that point in time.

## Where next?

- [Discover and subscribe](/docs/guides/discover-and-subscribe) for public consumption and pins.
- [Private skills and publishing](/docs/guides/publish-and-edit) for private multi-device work and optional releases.
- [Packs](/docs/guides/packs) for reusable skill sets.
- [Teams](/docs/guides/teams) for organization-owned skills and assigned packs.
