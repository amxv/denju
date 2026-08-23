---
title: Social discovery and search
description: Search one authorized metadata catalog, inspect profiles and resources, follow authors, star public skills, rank by stars, and report public resources.
order: 10
category: Start
summary: Discovery combines local drafts, private shares, team content, and public metadata without indexing skill instructions or changing filesystem desired state.
---

Denju discovery is deliberately separate from synchronization policy. Following an author or starring a skill changes discovery metadata only; it never installs, removes, pins, or updates a skill on your machine.

## Search one merged catalog

```bash
denju search "agent performance"
denju search "agent performance" --following
denju search "agent performance" --sort stars
denju search "agent performance" --topic rust
```

The normal search result set merges:

- the current metadata of local owned drafts, including unsynchronized edits;
- private skills shared with the signed-in user;
- team content the user may read;
- public global registry metadata.

Every result says where it came from (`local`, `owned`, `private_share`, `team`, or `public`) and whether the resource is locally private, team-visible, or public. A local owned workspace wins over older registry metadata for the same stable resource ID, so an unsynchronized description edit is immediately discoverable on that device.

Search indexes metadata only: locator/name, Agent Skills `description`, `license`, `compatibility`, explicit Denju discovery topics, owner, public fork provenance, pack membership labels, and public skill star count. **`SKILL.md` body text, scripts, assets, and other skill content are never put into the search document.**

Default ordering is text relevance first and stars second. Authors you follow receive a small relevance boost in the default search only. `--sort stars` makes star count the primary ordering, while `--following` restricts the remote catalog to public resources owned by followed users.

Deprecated resources remain searchable but are demoted below otherwise matching active resources.

Search results are bounded. Use `--limit` (maximum 50) and the opaque cursor returned by the prior result:

```bash
denju search "rust" --limit 20 --cursor <cursor>
denju top --limit 20 --cursor <cursor>
```

Do not parse or synthesize a cursor; pass it back unchanged.

## Universal `show`

`show` is the inspection command for all three public resource shapes:

```bash
denju show @alice
denju show @alice/review
denju show @alice/packs/core
```

For an owned skill on the current machine, `show` reads the current Denju-managed working generation, so an unsynchronized local draft does not get replaced by older registry metadata. Profiles expose bio, public skills, public packs, public forks/provenance, and follower/following information according to that user's privacy settings.

Follower and following lists use bounded keyset pages. If a profile result returns another cursor, continue the appropriate list with:

```bash
denju show @alice --followers-cursor <cursor>
denju show @alice --following-cursor <cursor>
```

## Profiles and privacy

Usernames remain immutable. The signed-in user can change only profile metadata/privacy:

```bash
denju identity update --bio "Agent infrastructure builder"
denju identity update --clear-bio
denju identity update --followers-visible false
denju identity update --following-visible false
```

Follower and following visibility are independent. Hiding either side hides both its list **and its aggregate count** from profile readers.

## Follow authors

```bash
denju follow @alice
denju unfollow @alice
```

Following is one-way. A claimed user stores the relationship in the registry. An anonymous installation can follow too: Denju keeps that stable-user-ID intent only in local SQLite and adopts it into the account when that installation claims or logs into an identity. No follow operation changes local skill presence.

V1 has no activity feed and sends no follow notifications.

## Stars and `top`

Only a claimed user can star a currently public skill:

```bash
denju star @alice/review
denju unstar @alice/review
denju top
denju top --topic rust
```

Star and unstar are idempotent. Aggregate counts are public; Denju does not publish an individual's star history. Packs cannot be starred.

Stars attach to the immutable skill resource ID. Unpublishing hides that resource and its count from search/profiles/rankings without deleting the star relationships. Republishing the same resource restores the count. Deleting the resource and later recreating the same locator creates a different ID with zero inherited stars.

`denju top` is the all-time public skill ranking and is not personalized by your follow graph. An optional discovery topic narrows the ranking.

## Discovery topics

Publish-capable owners/maintainers can attach explicit metadata topics without creating a skill revision or release:

```bash
denju topics @alice/review rust agent-infra
denju topics @alice/review          # clear topics
```

Topics are lowercase letters/numbers with single internal hyphens, at most 32 bytes each, with at most 12 topics on one resource. They are registry metadata; Denju does not infer them by scanning instructions.

## Private reports

A claimed user can send a moderation signal for a public resource:

```bash
denju report @alice/review --reason malicious
```

Reports are private operator data. Submitting one does not change visibility, desired state, ranking, or availability by itself. Operator quarantine is a separate authority action.
