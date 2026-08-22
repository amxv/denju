---
title: Identity and devices
description: Claim an optional Denju identity, log in on another device, recover access, and manage revocable credentials.
order: 3
category: Start
summary: Optional password identity builds on anonymous setup without rewriting local authorship or subscriptions.
---

Public discovery and subscriptions do not require an account. Run `denju setup`, use Denju anonymously, and claim an identity later only when you want account-wide state and authenticated write capabilities.

## Claim and login

```bash
denju claim @username
denju login @username
```

Both commands read passwords through hidden terminal input. Passwords are never command arguments. Claim shows a recovery secret exactly once; store it somewhere separate from the machine.

Claim preserves the setup-created installation author principal. Existing anonymous subscriptions move to account-wide desired state, and anonymous local forks are promoted into the claimed namespace without rewriting their immutable revision IDs. A second device can run setup, log in, and reconcile the same direct subscriptions.

Machine-readable `--json` mode never reads passwords or recovery secrets from stdin. Interactive-only identity commands return the stable `interactive_required` error instead.

## Recovery and backup

```bash
denju identity backup
denju identity recover @username
```

`identity backup` verifies the password, replaces the recovery secret, invalidates the previous secret, and displays the replacement once. Recovery consumes the current recovery secret, sets a new password, and rotates the recovery secret again. Denju never retrieves an existing recovery secret from the registry.

## Devices and automation tokens

```bash
denju devices
denju devices revoke <session-id>

denju tokens
denju tokens create --scope publish:username --expires-in-seconds 3600
denju tokens revoke <token-id>
```

Device sessions and automation tokens are opaque 256-bit bearer credentials. PostgreSQL stores only hashes; the CLI stores only the current revocable session token, preferring the operating-system credential store and using an owner-only file fallback where configured. Token secrets are displayed exactly once.

Revocation is authoritative on the next request. Token listing exposes IDs, scopes, and expiry metadata, never bearer values.

## Delete an account

`denju identity delete` requires explicit confirmation and hidden password input. It tombstones personally owned skills using the same resource-delete semantics, revokes device/install/automation credentials, clears managed local desired state, preserves historical authorship under a deleted-user principal, and releases the username. Non-owner team memberships and that user's private team workspaces are removed as part of deletion. An account that owns a team cannot be deleted until team ownership succession has been completed; Denju refuses to create an ownerless team.

A deleted username can be claimed again, but the new account receives new internal user, namespace, and author-principal IDs and inherits no subscriptions or attribution.
