---
title: Identity, devices, and recovery
description: When you need a Denju identity, how claim and login work, and how to manage recovery, devices, and automation tokens.
order: 4
category: Start
summary: "Stay anonymous for public use; claim a username only when ownership or private account state becomes useful."
---

Denju is anonymous-first. Public search, `show`, subscriptions, synchronization, and local subscription edits do not require an account.

Claim an identity when you want account-bound capabilities such as keeping your own private skills synchronized across devices, publishing, private sharing, teams, stars, or automation credentials.

## Claim a username

```bash
denju claim @alice
```

Denju prompts for a password using hidden terminal input. The registry stores an Argon2id password hash, not the password itself.

Claim also shows a recovery secret exactly once. Save it somewhere separate from the machine. Denju cannot retrieve an old recovery secret later.

Usernames and team names come from the same registry-wide pool, are lowercase, and cannot be renamed in v1.

## Log in on another machine

Run setup first, then log in:

```bash
denju setup
denju login @alice
```

After login, Denju brings your account state onto the new machine: direct subscriptions, personal packs, **your owned private skills**, and team-assigned packs. Your latest valid private changes can therefore follow you onto the new device without a separate file-sync setup.

The same principle applies to team content you are entitled to use. Team-private skills remain non-public, but subscriptions and assigned packs can keep those team releases current across the devices of signed-in team members.

Anonymous state created on the installation before claim/login is adopted where appropriate. Existing anonymous direct subscriptions and local fork history do not need to be recreated.

## Rotate your recovery secret

```bash
denju identity backup
```

After verifying the password, Denju creates a replacement recovery secret and invalidates the previous one. The new value is shown once.

If you forget the password:

```bash
denju identity recover @alice
```

Recovery consumes the current recovery secret, lets you set a new password, and rotates the recovery secret again.

## Manage devices

```bash
denju devices
denju devices revoke <session-id>
```

Device sessions are revocable opaque credentials. Revocation takes effect on the next authenticated request.

## Automation tokens

Agents and CI should use scoped, expiring automation tokens instead of receiving a human password:

```bash
denju tokens create --scope publish:alice --expires-in-seconds 3600
denju tokens
denju tokens revoke <token-id>
```

The token secret is shown once. Token listings show metadata such as ID, scope, and expiry—not the bearer value.

For strict automation, add `--json` to non-interactive commands. Commands that require a human password or recovery secret refuse to open an interactive prompt in JSON mode and return a machine-readable error instead.

## Delete an account

```bash
denju identity delete
```

Deletion requires explicit confirmation and password input. It revokes credentials, removes your subscriptions and other account relationships, deletes personally owned skills and packs using their normal rules, and preserves historical authorship without keeping the account active.

An account that owns a team cannot be deleted until ownership is transferred or the team is deleted. A username released by deletion may later be claimed by someone else, but the new account does not inherit the old account's subscriptions, skills, packs, stars, or attribution.
