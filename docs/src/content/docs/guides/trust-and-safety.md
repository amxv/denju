---
title: Trust and safety
description: What Denju verifies, what it deliberately does not sandbox or scan, and how reports, quarantine, and private access boundaries work.
order: 18
category: Use Denju
summary: "Denju verifies identity, authorization, structure, and content integrity; subscribing is still a trust decision about the skill itself."
---

Denju can prove that you received the exact authorized revision it intended to deliver. It does not prove that the skill's instructions are good or safe to run.

## What Denju verifies

For managed content Denju verifies things such as:

- whether you are allowed to access the skill or pack;
- portable Agent Skills structure;
- exact content hashes and revision identity;
- downloaded snapshot size and SHA-256;
- safe in-root paths and supported symlinks;
- a complete verified local copy before making an update visible.

## What Denju does not do

Denju v1 does **not** pre-scan public releases for malware, secrets, or instruction content and does not sandbox a skill at runtime.

Subscribing is therefore a trust decision about the publisher and the skill. Automatic updates continue that trust until you unsubscribe or pin an exact release.

Private content is protected by authenticated authorization and normal deployment TLS/encryption-at-rest boundaries. Denju does not provide end-to-end encryption in v1.

## Reports

A claimed user can send a private moderation signal:

```bash
denju report @owner/skill --reason malicious
```

Reports are operator input only. They do not automatically remove a skill or pack.

## Security quarantine

A registry operator can quarantine one exact skill release or an entire skill.

If a quarantined release is currently active, Denju removes it from the harness-visible managed state and preserves the existing local bytes under Denju's quarantine area for inspection. It never silently falls back to another version.

Quarantining an old historical release does not disturb a different clean release that is active now.

For packs, the quarantined skill remains part of the pack's authored history, but the pack becomes degraded with reason `quarantined`.

Retention after deletion cannot override security quarantine.

## Private sharing and team boundaries

Personal private sharing grants read/subscription access to one user; it does not grant write access.

Teams use membership and team roles rather than per-skill ACLs. Team maintainers each work in private working copies, and team members use released team versions rather than somebody else's unpublished draft.

Self-hosted registry operators receive the same quarantine and authorization model. Operator credentials live on the server side and are separate from normal Denju installation, session, and automation credentials.
