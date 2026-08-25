---
title: CLI reference
description: A compact map of Denju's command surface, global options, resource locators, and the command groups used by people and agents.
order: 40
category: Reference
summary: "One CLI covers discovery, publishing, collaboration, teams, synchronization, diagnostics, and upgrades."
---

Run:

```bash
denju --help
```

for the authoritative command list in the installed version. Denju keeps one CLI surface for humans and agents; there is no separate agent mode.

## Global options

```text
--json          emit one versioned JSON result on stdout
-V, --version   print the Denju build version
-h, --help      print help
```

Running plain `denju` is state-aware guidance. It reports a compact health state and, when useful, one next action.

## Resource locators

Skills:

```text
@owner/skill
@owner/skill@v7
```

Packs:

```text
@owner/packs/name
```

Users and teams:

```text
@alice
@acme
```

Commands that operate on a managed resource use these locators instead of guessing package identity from the current directory.

## Setup and identity

```bash
denju setup [--registry URL]
denju claim @username
denju login @username

denju identity
denju identity update ...
denju identity backup
denju identity recover @username
denju identity delete

denju devices
denju devices revoke <session-id>

denju tokens
denju tokens create --scope <scope> [--expires-in-seconds N]
denju tokens revoke <token-id>
```

## Discovery

```bash
denju search QUERY [--sort relevance|stars] [--following] [--topic TOPIC] [--limit N] [--cursor CURSOR]
denju show LOCATOR [--followers-cursor CURSOR] [--following-cursor CURSOR]
denju top [--topic TOPIC] [--limit N] [--cursor CURSOR]

denju follow @user
denju unfollow @user
denju star @owner/skill
denju unstar @owner/skill
denju topics LOCATOR [TOPIC ...]
denju report LOCATOR --reason REASON
```

## Skills and releases

```bash
denju import PATH [--to @team]
denju publish LOCATOR [--public] [--message TEXT] [--tag TAG ...]
denju rename LOCATOR NEW_NAME
denju unpublish LOCATOR
denju delete LOCATOR [--yes]
denju deprecate LOCATOR [--replacement LOCATOR | --undo]

denju history [LOCATOR]
denju history prune LOCATOR [--yes]
denju diff LOCATOR [REVISION_A] [REVISION_B]
denju restore LOCATOR REVISION
denju export LOCATOR DESTINATION
denju usage
```

## Subscriptions and sharing

```bash
denju subscribe LOCATOR [--version N] [--retain-on-delete]
denju unsubscribe LOCATOR

denju share LOCATOR @user
denju unshare LOCATOR @user
```

`--version` and `--retain-on-delete` are direct-skill subscription options; live pack subscriptions do not use them.

## Forks and proposals

```bash
denju fork @owner/skill
denju fork sync @you/skill
denju fork resolve @upstream/skill --as NAME
denju fork resolve @upstream/skill --merge-into @you/skill
denju fork resolve @upstream/skill --discard

denju propose @you/fork [--message TEXT]
denju proposals
denju proposal show <id>
denju proposal accept <id>
denju proposal reject <id>
denju proposal withdraw <id>
```

## Packs

```bash
denju pack create @owner/packs/name
denju pack add @owner/packs/name SKILL [SKILL ...]
denju pack remove @owner/packs/name SKILL [SKILL ...]
```

Pack visibility, rename, deletion, and subscription use the same top-level `publish`, `rename`, `unpublish`, `delete`, `show`, `subscribe`, and `unsubscribe` commands as skills.

## Teams

```bash
denju team
denju team create @team
denju team show @team
denju team invite @team [--role member|maintainer]
denju team invite-revoke @team <invite-id>
denju team join <code>
denju team role @team @member member|maintainer
denju team remove @team @member
denju team settings @team --members-can-publish true|false
denju team assign @team @owner/packs/name
denju team unassign @team @owner/packs/name
denju team leave @team
denju team transfer-owner @team @member
denju team accept-owner <code>
denju team delete @team

denju transfer LOCATOR @team
```

## Local operation

```bash
denju status
denju sync
denju doctor
denju upgrade
```

Use `status` to inspect resource/synchronization blockers, `sync` to perform one reconciliation, and `doctor` to validate or repair the local installation itself.
