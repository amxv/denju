# Denju social discovery wire contract v1

The `/v1` API includes a metadata-only discovery surface. PostgreSQL remains authoritative for profiles, relationships, social counts, resource access, and report records. `resource_search_documents` is derived metadata that can be rebuilt from authoritative rows; it never contains skill bodies, scripts, blobs, snapshot bytes, or object-store locations.

## Read endpoints

- `GET /v1/search` accepts `q`, optional `limit`, opaque `cursor`, `sort=relevance|stars`, `following`, and optional `topic`. It returns one authorization-filtered catalog containing public resources plus private/team resources readable by the authenticated caller.
- `GET /v1/top` is the all-time public-skill ranking by aggregate stars, with optional topic and keyset cursor. Follow relationships do not personalize this ranking.
- `GET /v1/show?locator=...` is universal: `@user` returns a profile, a skill locator returns accessible skill detail, and a pack locator returns accessible pack detail. Profile follower/following pages accept separate opaque cursors.

`CatalogResource` identifies `kind`, stable `resource_id`, current locator, source (`public`, `owned`, `private_share`, `team`, or client-local after CLI merge), visibility, normalized Agent Skills metadata, explicit topics, public star count, deprecation state, public/authorized fork provenance, and pack membership labels where applicable.

The registry search document is limited to locator/name/description/owner, `license`, `compatibility`, discovery topics, provenance, pack-member labels, and star count. Search code must not read or index `SKILL.md` bodies or other skill files.

Default registry ordering is active-before-deprecated, then text relevance, then stars, then stable locator/ID ties. A followed public user adds a modest relevance-only boost. `sort=stars` uses star count before text relevance and receives no follow boost. Cursors bind the sort order and exact keyset position; clients treat them as opaque.

## Profile mutation

`POST /v1/profile` updates the authenticated user's optional bio plus independent `followers_visible` and `following_visible` booleans. A hidden relationship side exposes neither its list nor its count. Bio is at most 500 Unicode scalar values.

The request carries UUIDv7 `operation_id` and an endpoint-domain canonical `request_hash`. Exact retries replay the committed response; conflicting operation reuse is rejected.

## Follow mutation

- `POST /v1/follows` follows one stable user ID.
- `POST /v1/follows/remove` removes that relationship.

The relationship is one-way and idempotent at the domain level. Self-follow is rejected. Follows never create desired-state rows or filesystem work. The CLI may keep anonymous follow intent locally, keyed by stable registry user ID, and convert it to this authenticated mutation after claim/login.

## Stars and ranking

- `POST /v1/stars` stars one stable resource ID.
- `POST /v1/stars/remove` removes the caller's star.

Only active public skills may be starred; packs are rejected. `(user_id, resource_id)` is unique, making repeated star/unstar actions idempotent. Aggregate `resources.star_count` changes transactionally with the relationship and refreshes only derived search metadata.

Unpublishing does not remove `resource_stars`; public reads expose zero/no count for non-public content. Republishing the same stable resource makes its prior aggregate visible again. A new resource that reuses a deleted locator has a new ID and inherits no star rows.

## Discovery topics

`POST /v1/resources/topics` is a generation-CAS metadata mutation for a resource the caller may publish. Topics are normalized lowercase strings containing ASCII letters/numbers with single internal hyphens, maximum 32 bytes each and 12 per resource. Topics do not create a skill revision or release.

## Reports

`POST /v1/reports` accepts a claimed user's private moderation report for a currently public resource. The reason is 1–64 characters. Reports are not exposed through end-user discovery APIs and do not mutate resource visibility or availability. The operator API consumes this table for review and quarantine decisions.

## Derived indexes and lifecycle

Resource metadata changes refresh one `resource_search_documents` row. Fork/upstream and pack-member locator changes refresh only directly dependent derived rows. Account deletion removes follow relationships and the deleting user's stars, updates affected aggregate counts, nulls report attribution, and removes social idempotency rows while preserving immutable resource/revision history according to the existing lifecycle contract.

The CLI merges local owned-draft metadata after the registry response without uploading local-only metadata merely for search. A matching local workspace replaces older remote metadata for the same stable resource ID. Body/script bytes remain outside both the local metadata table and the registry search document.
