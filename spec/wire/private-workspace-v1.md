# Denju private workspace synchronization wire contract v1

Owned private skills synchronize immutable Merkle revisions through one compare-and-swap workspace ref. Filesystem notifications are local dirty hints only; PostgreSQL workspace generation and revision remain remote authority.

Private revision endpoints:

- `POST /v1/private-skills/revisions/prepare` — authenticated `skills:write`; validates the expected resource generation and parent revision, reserves one UUIDv7 operation/revision identity, and returns random presigned staging uploads only for objects the namespace has not already proven.
- `POST /v1/private-skills/revisions/commit` — streams/verifies required objects by exact size and SHA-256, reconstructs and validates the declared Agent Skills tree, enforces namespace quota, promotes canonical objects, persists revision ancestry/reachability, and advances the private workspace ref atomically when the expected generation and parent still match.
- `GET /v1/private-skills` — authoritative current private workspace catalog used by other authenticated devices for reconciliation.

The prepare request binds `operation_id`, immutable `resource_id`, `expected_generation`, `expected_parent_revision_id`, the semantic manifest, and an RFC-8785 request hash under the private-revision v1 domain. The registry computes the revision identity from root tree, parent, authenticated author principal, and operation ID. Exact retries replay the same prepared/committed operation; operation-ID reuse with different intent is rejected.

The client records coherent valid saves in SQLite before network execution. Each queued local revision stores its operation ID, parent, expected generation, semantic manifest, and root. Offline or quota-blocked work remains queued and editable. Successful commit advances the local workspace base without rewriting revision identity.

Only namespace-proven blobs skip staging. Physical cross-tenant deduplication never changes that authorization rule. A staged object is not trusted until the registry verifies its bytes; the workspace ref cannot advance before all referenced objects and Merkle metadata are authoritative.

If another device advances the same private workspace first, the stale commit returns `generation_conflict`. The losing local head and working bytes remain preserved and the resource pauses instead of using last-write-wins. Deterministic content merge and user-facing conflict resolution are separate later synchronization behavior; this contract guarantees preservation of both divergent heads.

Local watcher delivery is deliberately non-authoritative. Native events wake a bounded coalescing queue; overflow requests a full verification scan. Periodic scans and polling fallback reconstruct state from the managed filesystem and SQLite index. File size, high-resolution mtime, executable state, and prior blob identity allow unchanged files to reuse hashes; forced verification rehashes content.

Collision-derived harness projections are independently writable materializations rather than hard-linked writable files. A persisted semantic baseline identifies whether canonical or derived content changed. Derived writeback is journaled through `planned -> staged -> verified -> switched -> complete`, validates the canonicalized Agent Skills tree before switching, and recovers idempotently after interruption. Simultaneous divergence of canonical and derived views pauses the resource instead of guessing.
