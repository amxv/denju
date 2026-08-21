# Denju proposals wire contract v1

Proposals are private moving references from one fork resource to that fork's immutable upstream resource. They do not create Git branches, copied review content, comments, or publication authority.

## Endpoints

- `POST /v1/proposals` — proposer-only creation from an owned fork. The request binds UUIDv7 operation ID, source `ResourceId`, source generation, optional short message, and canonical request hash. One open proposal per source/target pair is allowed.
- `GET /v1/proposals` — lists only proposals where the authenticated user is the proposer or current target maintainer/owner.
- `GET /v1/proposals/{id}` — returns the same visibility-scoped detail; unauthorized proposal IDs return `not_found` rather than revealing existence.
- `POST /v1/proposals/{id}/accept` — target maintainer/owner only. The request binds the proposal generation, exact proposed revision ID, and current target resource generation inspected by the maintainer.
- `POST /v1/proposals/{id}/reject` — target maintainer/owner only.
- `POST /v1/proposals/{id}/withdraw` — proposer only.

Every mutation is idempotent by actor, UUIDv7 operation ID, endpoint-domain request hash, and committed response. Reusing an operation ID with different intent is rejected.

## Moving-head semantics

While open, proposal detail derives `proposed_revision_id` from the source fork's current private workspace. A later coherent fork revision therefore moves the proposal without rewriting or copying revision history. The target side is resolved through the fork's immutable upstream `ResourceId`, not locator text.

The proposal is `open` while the fork's recorded synchronization base matches the proposal-visible upstream head. If upstream advances, detail reports `needs_sync`. The client performs ordinary `fork sync`: clean changes create a two-parent fork revision and the proposal automatically points at that new head. A true overlap remains `needs_sync` and returns control to the proposer for explicit fork conflict resolution.

Fork-sync conflict resolution is client-local state, not a registry merge operation. Denju records the exact conflicted fork head, upstream head, synchronization base, and conflicting paths. A retry cannot treat unrelated edits as resolution: every recorded conflicting path must have changed from the originally conflicted fork revision. The deterministic merge is then rerun with the edited fork version chosen only for those explicitly resolved paths, while unrelated clean upstream changes still merge normally. Success commits an ordinary verified two-parent fork revision with `ForkSyncIntent` and advances the fork's synchronization base.

## Terminal proposal state

Accept, reject, and withdraw freeze the proposal's exact source revision ID and source generation in the terminal row so later fork edits do not rewrite historical proposal outcome. Terminal proposal revision IDs are protected from private-history pruning.

Acceptance is an explicit compare-and-swap action on the target private workspace. It attaches the **exact proposed revision ID** to the target resource after verifying the maintainer still sees the same proposal generation and target resource generation. The accepted revision's immutable ancestry is not rewritten to include unrelated unpublished maintainer draft revisions; those earlier revisions remain immutable history. If the proposal or target advances after inspection, the accept request fails and must be retried from fresh detail.

Acceptance associates the accepted revision's existing Merkle graph and snapshot with the target resource, applies target-namespace logical blob reachability/quota accounting, advances only the target private workspace ref/generation, and emits the ordinary authority event/outbox wake. It never creates a public release. Publication remains a separate `denju publish` mutation.

Reject and withdraw change proposal state only. They do not mutate either fork or upstream workspace. The contributor's fork remains independent after every terminal proposal outcome.
