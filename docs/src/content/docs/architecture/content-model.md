---
title: Merkle content and revisions
description: How Denju gives skill bytes deterministic identities, reuses unchanged content, builds immutable revisions, and derives efficient snapshots.
order: 31
category: Architecture
summary: "Files become blobs, directories become Merkle trees, and revisions point at one root tree—so unchanged content is shared instead of retransferred."
---

Denju needs two properties that ordinary directory copying does not provide:

1. a precise identity for **exactly which skill content** a revision contains;
2. an efficient way to know which bytes another device or registry is missing.

It gets both from a small Merkle content model.

## Blob: one file's bytes

A file's content identity is its SHA-256 digest:

```text
BlobId = SHA256(raw file bytes)
```

Change one byte and the BlobId changes. Keep the file identical and every revision/device can refer to the same BlobId.

File timestamps, owners, groups, and other machine-local metadata do not participate in identity. Executable state does, because it is meaningful portable skill behavior.

## Tree: one directory level

A tree contains sorted typed entries for one directory:

```text
tree
├── SKILL.md  -> blob A
├── scripts   -> tree B
└── refs      -> tree C
```

The tree identity hashes the names, entry types, executable state where relevant, and child identities in deterministic order.

If only `scripts/check.py` changes:

- that file gets a new BlobId;
- the `scripts/` tree gets a new TreeId;
- ancestor trees up to the root get new TreeIds;
- every untouched file blob and unaffected subtree keeps its existing identity.

That is the core efficiency win. A revision does not need a brand-new copy of the entire directory just because one small file changed.

## Revision: one immutable skill state

A revision points at one root tree plus its parent revision(s) and author/operation identity.

```text
revision
  root:    TreeId
  parents: [RevisionId, ...]
  author:  stable principal
```

Normal saves have one parent. A clean merge or explicit conflict resolution can have two parents.

Revisions are immutable. Restore creates a new revision that points back through history; it does not move history backward or rewrite an old object.

## Releases are named immutable revisions

A release such as `v7` is a permanent public/team-facing reference to one exact revision.

`latest` is the mutable pointer that advances from release to release:

```text
v6 -> revision 6
v7 -> revision 7
latest -> v7
```

A subscriber following latest only needs to learn that `latest` changed. A subscriber pinned to `v6` continues referencing the old immutable revision.

## Why missing-object negotiation is cheap

When uploading a new revision, the client already knows the BlobIds and tree identities it wants to reference. The registry can determine which content this namespace has not yet proved.

Only missing file blobs need to be uploaded. Unchanged content is reused by identity.

The same principle helps downloads: a device with a local content-addressed cache does not need to download blobs it already has and can verify.

## Physical deduplication does not become an access leak

Content-addressing often creates a subtle multi-tenant problem: if the server simply says “I already have hash X,” one tenant can probe whether another tenant uploaded specific bytes.

Denju avoids making global physical existence an authorization signal. A namespace's first reference to a blob must still prove those bytes through its authorized staging flow, even when another namespace already caused identical bytes to exist physically. After verification, the physical duplicate can be discarded while logical reachability and quota accounting remain namespace-specific.

## Snapshots are transport caches, not identity

For a cold install, requesting thousands of tiny objects separately would be wasteful. Denju therefore derives deterministic compressed release snapshots for efficient transfer.

The snapshot is a transport optimization. The authoritative identity remains the Merkle manifest and immutable objects. A client verifies the downloaded snapshot back against that identity before exposing it.

This split gives Denju both sides of the trade-off: fine-grained deduplication for changes, efficient bundled transfer for cold installs.
