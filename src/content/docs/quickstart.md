---
title: Quickstart
description: Install both CLIs, authenticate Agentbox, and share your first complete skill bundle.
order: 1
category: Start
summary: From npm install to a team-visible Agentbox thread in a few commands.
---

## Install

Install the bundle CLI and the Agentbox CLI:

```bash
npm install -g denju-cli
npm install -g @amxv/agentbox
```

Verify both commands:

```bash
denju --version
agentbox --version
```

## Authenticate Agentbox

```bash
agentbox login
agentbox doctor
```

An existing configured Agentbox profile also works.

## Share installed skills

Skill names resolve under `~/.agents/skills` by default:

```bash
denju \
  --team ama \
  --title "Reusable agent skills" \
  agentbox dogfood frontend-design
```

The command validates all three skills, creates one archive, opens a new thread,
attaches the archive, and grants the `ama` team access. It prints the stable
thread ID when the workflow succeeds.

## Build an archive locally

Use package-only mode to inspect a handoff without creating a thread:

```bash
denju \
  --package-only \
  --archive ./agent-skills.tar.gz \
  agentbox dogfood

tar -tzf ./agent-skills.tar.gz
```

Existing output files are not overwritten.
