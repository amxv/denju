---
title: Trust, reports, and quarantine
description: Understand Denju's trust boundary, private moderation reports, operator quarantine, local quarantine copies, and registry isolation.
order: 12
category: Start
summary: Denju verifies structure and content identity but does not sandbox or pre-scan skills; registry quarantine is the exceptional security override for content already published or shared.
---

Denju treats installing or subscribing to an Agent Skill as a trust decision. It verifies the portable filesystem profile, manifest, immutable object hashes, and authorized registry relationships, but **does not scan skills for malware, secrets, or instruction content before publication** and does not sandbox a skill at runtime.

Private content is protected by registry authorization, TLS in transit, and deployment-level encryption at rest. Denju v1 does not provide end-to-end encryption.

## Report a public resource

A claimed user can privately report a public skill:

```bash
denju report @alice/review --reason malicious
```

A report is only a moderation signal. It is not public, does not change ranking or desired state, and does not remove content automatically. Reports are visible through the registry operator surface, not the end-user `denju` CLI.

## Security quarantine

A registry operator may quarantine either an entire resource or one exact immutable skill release. Self-hosted operators use the same authority model.

Quarantine is deliberately different from owner deletion or unpublishing:

- it does not rewrite the immutable release or revision history;
- new access to the quarantined target is blocked immediately;
- affected clients receive the ordinary resource-generation wake hint and reconcile from authoritative state;
- if the quarantined release is currently visible, Denju removes its canonical and harness projections;
- before removal, the existing local generation is copied to `~/.denju/quarantine/<resource-id>/<revision-id>/` for inspection;
- Denju never silently falls back to another release;
- a historical quarantined release does not disturb a different clean release that is currently active;
- an unavailable pack member remains in authored pack history and appears as degraded with reason `quarantined`;
- direct `--retain-on-delete` cannot override quarantine.

A whole-resource quarantine also blocks the owner's live private workspace. If a local edit exists when the security decision arrives, the bytes are preserved in the quarantine tree but are not uploaded through the quarantined authority boundary.

After the operator lifts the relevant quarantine, ordinary `denju sync` reconciles the same stable resource relationship again. A clean remote desired revision rematerializes normally; quarantined local inspection bytes are not silently promoted into registry history.

## Operator boundary

Operator powers live only on `denju-server`. The ordinary client CLI cannot mint or use them.

A self-hosted operator bootstraps or revokes an operator credential with the migration-owner connection available only to that one-shot command. Routine report review and quarantine requests use a separately hashed operator bearer against the protected `/v1/admin` API. Installation, user-session, and automation credentials are rejected by the admin surface.

Operator bearer values are shown once. PostgreSQL stores only their SHA-256 digest, and quarantine mutations are audited and idempotent.

## PostgreSQL isolation

The normal registry process uses separate restricted PostgreSQL login roles:

- `DENJU_DATABASE_URL` — request SQL as `denju_app`;
- `DENJU_DATABASE_WORKER_URL` — durable recovery/background work as `denju_worker`;
- `DENJU_DATABASE_DIRECT_URL` — session-dependent `LISTEN/NOTIFY` as `denju_app`, never a transaction-mode pooler;
- `DENJU_DATABASE_MIGRATION_URL` — schema/operator bootstrap authority supplied only to migration/admin bootstrap/revoke commands, not the ordinary server runtime.

The app and worker roles are non-superuser and `NOBYPASSRLS`, cannot switch to one another or to a bypass role, and authenticate directly rather than connecting as an owner and using `SET ROLE`.

Application authorization remains the primary business-policy boundary. PostgreSQL row-level security adds defense in depth around private resources, maintainer workspaces, team data, private shares, subscriptions, immutable revision snapshots, Merkle/object reachability, social relationships, and moderation data. Actor identity is transaction-local, so a pooled connection cannot retain one user's authority for the next request.

Global content-addressed trees, revisions, canonical blobs, and durable event rows are not table-wide read surfaces for `denju_app`. Where an idempotent write needs PostgreSQL conflict detection, Denju uses narrowly scoped database functions that verify the active actor and exact semantic row instead of granting global object/event enumeration.

## Network and request hardening

Non-loopback registry and S3 origins must use HTTPS and may not embed URL credentials. Registry JSON request bodies are bounded, malformed JSON is rejected without echoing the supplied payload, and presigned object transfers are short-lived. Canonical S3 object keys and content hashes are not authorization capabilities: an authorized registry relationship is required to obtain a valid presigned transfer URL.

The recovery drain endpoint uses a credential distinct from all client/session/automation/operator credentials. Repeated authorized recovery calls are idempotent and ordinary client credentials are rejected.
