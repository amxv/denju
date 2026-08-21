# Resource lifecycle v1

Personal skill lifecycle is keyed by immutable `ResourceId`. Locator text is presentation/routing state and must never be used to preserve or inherit relationships.

## Mutations

Authenticated owner mutations use UUIDv7 operation IDs, expected resource generation, and a request hash that binds every semantic field.

- `POST /v1/skills/rename` — changes the active locator while preserving `ResourceId`; creates a name-matching revision and, for an already-public resource, the next immutable release. When the user already edited `SKILL.md` to the requested name, the request may bind a prepared private-revision operation whose verified object bytes are consumed by the rename transaction.
- `POST /v1/skills/unpublish` — removes public visibility and advances generation without deleting releases or direct subscription rows.
- `POST /v1/skills/delete` — tombstones the resource, records its final release when present, removes redirects targeting it, and frees the locator for unrelated reuse.
- `POST /v1/skills/deprecate` — sets or clears reversible deprecation metadata and an optional replacement `ResourceId`.
- `POST /v1/skills/history/prune` — removes only eligible unreleased private revisions that are not the current private workspace head.
- `GET /v1/usage` — returns namespace logical storage/quota and prunable private-history estimates.

All lifecycle responses return the authoritative resource generation after the mutation. Replaying an operation ID with a different request hash is rejected.

## Rename and redirects

Rename authority is committed in PostgreSQL before local path migration. Local canonical/projection switching is SQLite-journaled and recoverable. A public old locator may redirect to the renamed active resource while that locator remains unused. Direct active lookup wins, and deleting the target removes redirects to it.

Pending direct-name edits use the ordinary private-revision prepare/upload surface only as staging proof. `RenameSkillRequest.prepared_revision_operation_id` is covered by the rename request hash. The registry verifies that preparation belongs to the same user/resource/generation/current parent, reconstructs the old-name working tree from staged or already-reachable canonical bytes, rewrites only `SKILL.md`'s name to the requested locator, and commits that complete tree as the rename revision. The prepared operation is consumed in the same PostgreSQL transaction; no intermediate private revision or ref is exposed.

A later resource created at a reused locator has a new `ResourceId`; old subscriptions, retention choices, pins, and history never transfer by locator.

## Unpublish and dormant subscriptions

Unpublish is O(1) in subscriber count. Existing direct subscription rows stay keyed to the same `ResourceId`, but the public resource is omitted from desired materialization until it is public again. Republish of the same resource reactivates those rows. No per-subscriber grant/event rows are created.

## Delete retention

`SubscriptionMutationRequest.retain_on_delete` is explicit and defaults to false. If a subscribed resource is tombstoned:

- `false`: it disappears from desired materialization on reconcile;
- `true`: it resolves to the tombstone's final immutable release and is marked `retained_after_delete`.

Retention is represented by the existing direct subscription row plus the resource tombstone/final release. It is not a copied grant and cannot attach to a recreated locator.

## Deprecation

`PublicSkill.deprecation` is optional common metadata so search, show, subscription reconciliation, and publish responses cannot silently disagree. When present it may contain both replacement resource ID and current replacement locator. Search orders non-deprecated matches before deprecated matches.

## Prune and canonical GC

History prune transactionally removes only eligible private revision/reachability rows and reports logical bytes reclaimed. Newly unreachable canonical blobs become delayed GC candidates. Physical canonical deletion requires the configured grace interval to pass and a fresh zero-reachability check inside the GC transaction.

Because prune advances resource generation without changing the current workspace revision, it also invalidates every still-prepared private revision for that resource in the same transaction. Clients with quota-blocked queued revisions shift only their expected generations by the authoritative delta and retry the same operation/revision identities; stale staged rows are removed and their non-authoritative staging objects are deleted best-effort after commit.

Deterministic snapshot archives are derived objects, but they may not be deleted while any current workspace, immutable release, or retained revision references their object key.

## Account deletion interaction

Account deletion applies ordinary tombstone semantics to every personally owned resource before removing the user's namespace. Tombstones retain the deleted owner slug and immutable author principals are converted to deleted-user attribution. The username can then be claimed by a new namespace without inheriting the prior account's resources or relationships.
