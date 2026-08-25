---
title: Discovery, profiles, follows, and stars
description: Use Denju's metadata catalog and social signals without confusing discovery relationships with installed skill state.
order: 16
category: Use Denju
summary: "Search the catalog, follow authors, star public skills, and browse rankings without changing which skills are installed."
---

Denju's discovery layer helps you find useful skills and authors. It is deliberately separate from synchronization.

Following a person does **not** install their skills. Starring a skill does **not** subscribe to it.

## Search one authorized catalog

```bash
denju search "agent performance"
denju search "agent performance" --following
denju search "agent performance" --sort stars
denju search "agent performance" --topic rust
```

Results can include:

- your current local owned drafts;
- private skills shared with you;
- team content you are allowed to read;
- public registry metadata.

Search indexes metadata only: locator, name, Agent Skills description, compatibility/license, topics, owner, public fork provenance, pack membership labels, and public star count. Skill instructions, scripts, and assets are not full-text indexed.

## Inspect users, skills, and packs

```bash
denju show @alice
denju show @alice/review
denju show @alice/packs/core
```

Profiles can expose a bio, public skills/packs/forks, and follower/following information according to that user's privacy settings.

## Follow an author

```bash
denju follow @alice
denju unfollow @alice
```

Following gives that author a modest relevance boost in default search and enables `--following` filtering.

An anonymous installation can follow too; Denju stores that intent locally and adopts it if the installation later claims or logs into an identity.

## Star public skills

```bash
denju star @alice/review
denju unstar @alice/review
denju top
denju top --topic rust
```

Only claimed users can star, and only public skills can be starred. Packs are not starred in v1.

Stars belong to the underlying skill, not just its current name. Unpublishing hides the skill and its count from public discovery but preserves its stars; republishing the same skill restores them. Deleting it and creating a new skill with the same name does not inherit the old stars.

## Discovery topics

Owners and maintainers can attach explicit topics without creating a skill revision:

```bash
denju topics @alice/review rust agent-infra
```

Denju does not infer topics by scanning the skill body.

## Report a public skill

```bash
denju report @alice/review --reason malicious
```

A report is private moderation input. It does not automatically change ranking, availability, or what is installed on anyone's machine. Registry operators can investigate and apply quarantine separately.
