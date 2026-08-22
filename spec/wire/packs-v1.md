# Denju packs wire contract v1

Packs are flat versioned desired-state resources. Authored membership stores skill `ResourceId` plus optional exact release pin; every immutable pack revision stores the exact resolved skill release/revision IDs needed to reproduce that version.

## Resource and mutation endpoints

- `POST /v1/packs` creates an owned private pack at immutable pack version 1 with no members.
- `GET /v1/packs?locator=...` returns the currently visible pack plus every authored member, its exact current resolved revision, optional release pin, and any unavailability reason.
- `POST /v1/packs/add` atomically adds or changes one or more skill member intents. A changed request creates exactly one next immutable pack version.
- `POST /v1/packs/remove` atomically removes one or more members. A changed request creates exactly one next immutable pack version.
- `POST /v1/packs/publish` makes an owned pack public after validating that every member is readable by the full public audience. Visibility-only publication does not fabricate a new pack version.
- `POST /v1/packs/rename`, `/unpublish`, and `/delete` apply stable-resource lifecycle semantics. Rename preserves `ResourceId`; unpublish preserves dormant subscription roots; delete removes pack subscription roots and frees the locator for an unrelated new resource.

Every authored mutation uses UUIDv7 operation identity, an endpoint-domain canonical request hash, expected resource generation, and a committed response replay. Reusing an operation ID with different intent is rejected.

Packs are skills-only and cannot nest. A public pack may contain only public skills. A private personal pack may contain any skill the owner can currently read. A missing `release_version` means follow latest; a positive `release_version` means an exact immutable release pin.

## Immutable pack revisions

`pack_revisions` assigns one monotonically increasing version for each authored membership change or semantic followed-release advancement. `pack_revision_members` records, for every member in that version:

- stable skill `ResourceId`;
- authored optional pinned release version;
- exact resolved release version when resolution uses a release;
- exact resolved immutable RevisionId;
- deterministic member order.

Historical pack versions therefore never require recomputing `latest`. Skill private-history pruning must retain any revision referenced by immutable pack history.

## Subscription endpoints

- `POST /v1/pack-subscriptions` subscribes the authenticated account or anonymous installation to one live pack resource.
- `POST /v1/pack-subscriptions/remove` removes that desired-state root.
- `GET /v1/pack-subscriptions` returns every currently active readable subscribed pack with the exact desired member snapshots needed for local application.

Pack subscriptions neither pin the pack version nor support retain-on-delete. Unpublishing a public pack omits it from public-only catalogs without deleting the durable relationship; republishing the same stable pack reactivates it. Deleting a pack removes the subscription roots so later locator reuse cannot reconnect them.

Pack-subscription operation rows persist the exact committed pack response. An exact retry therefore returns the originally observed pack generation/version even if the live pack has advanced since the first request.

## Ordered follow-latest advancement

Only durable authority events whose semantic kind is `skill_release_published` can advance follow-latest pack members. Generic `resource_changed` events never do.

Each release event contains the released skill resource, exact release version, and exact immutable RevisionId. The pack drain processes release events in authority-event order and creates at most one pack revision per `(pack, release event)`. It verifies the event against immutable `skill_releases` authority before advancing the pack.

Pack/skill advisory locks serialize authored mutations with release-driven advancement. Before an authored pack mutation commits, it catches that pack up through already-pending release events; if catch-up changed the pack generation, the authored request returns a generation conflict and must retry from the new exact state. This prevents an authored edit from skipping a committed release event.

Skill publication itself does not loop over all dependent packs. After the publish transaction commits, it may run a fixed-size bounded drain. Remaining events stay durable in PostgreSQL and can be processed by:

- the ordinary bounded request-adjacent drain;
- `denju-server drain-packs --limit N`;
- authenticated `POST /v1/internal/packs/drain` using the separately configured recovery bearer.

The recovery endpoint is deployment-neutral and contains no provider-specific scheduler assumptions. Repeating a completed drain is idempotent.

## Degraded and conflicting desired state

Pack member unavailability uses a stable wire vocabulary: `deleted`, `unpublished`, `access_revoked`, and `quarantined`. The authored member and immutable history remain intact while its current desired snapshot is unavailable.

Local pack application stages and verifies every changed desired member before switching any pack-managed canonical pointer. One SQLite parent journal records the previous and next exact revisions for the complete touched set. If a switch fails, Denju restores the previous pointers; startup/sync recovery performs the same rollback for an interrupted parent journal before beginning new work.

Multiple packs are equal-authority desired sources. If they require different exact revisions of one skill, no pack silently wins. A first-install conflict projects none of those competing revisions; an already-valid pack-managed projection remains frozen on its last exact revision while the conflict is recorded. `denju status` identifies the source packs and concrete unsubscribe resolution commands.
