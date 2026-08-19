---
title: Command reference
description: Flags, defaults, output behavior, and examples for denju.
order: 3
category: Reference
summary: A compact reference for live shares and package-only bundles.
---

## Syntax

```bash
denju [options] <skill> [<skill> ...]
denju --version
```

At least one skill name is required.

## Options

| Option | Behavior |
| --- | --- |
| `--skills-dir <path>` | Skill root. Defaults to `~/.agents/skills`. |
| `--team <slug>` | Agentbox team slug or ID. Required for a live share. |
| `--title <title>` | New thread title. Defaults to a count-based title. |
| `--archive <path>` | Retain the generated archive at this path. |
| `--package-only` | Skip every Agentbox command and only build the archive. |
| `--help` | Print command help. |
| `--version` | Print the CLI version. |

## Live share

```bash
denju \
  --team engineering \
  --title "Frontend skill set" \
  frontend-design react-best-practices shadcn
```

Successful output includes the new `thr_...` ID and the team slug.

## Alternate skill root

```bash
denju \
  --skills-dir ./skills \
  --package-only \
  --archive ./skills.tar.gz \
  custom-skill
```

## Failure behavior

The command exits nonzero when a skill is missing, a name is unsafe, the output
already exists, Agentbox returns invalid JSON, attachment upload fails, or team
sharing fails. If attachment or sharing fails after thread creation, the error
includes the thread ID for recovery.
