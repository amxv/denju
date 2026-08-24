# Denju operator and quarantine wire contract v1

The registry exposes a dedicated operator authority surface without adding operator powers to the end-user `denju` CLI. PostgreSQL/current refs remain authoritative; quarantine uses the same resource generation, durable authority-event, outbox, SSE wake, and reconciliation model as other authority changes.

## Operator credentials

`denju-server admin bootstrap --name <name>` is a local/self-host operator command that requires the separately supplied migration-owner database URL. It creates a random operator bearer, prints it once, and stores only a 32-byte SHA-256 digest in PostgreSQL. `denju-server admin revoke <operator-id>` uses the same privileged one-shot database boundary and makes the bearer fail its next authorization check.

The ordinary server runtime does not require or receive the migration-owner credential. Runtime `/v1/admin` requests authenticate an operator bearer only through the narrow database authentication function. Installation, session, automation, and recovery credentials are not operator credentials.

## Admin HTTP endpoints

The protected operator API is:

- `GET /v1/admin/reports?limit=&cursor=` — bounded newest-first private moderation reports with opaque keyset continuation;
- `GET /v1/admin/resources/resolve?locator=` — resolve an active locator or stable resource UUID to operator mutation metadata;
- `POST /v1/admin/quarantine` — quarantine an entire resource or one exact skill release;
- `POST /v1/admin/unquarantine` — lift that exact quarantine scope.

Quarantine mutations use `AdminQuarantineRequest`:

```json
{
  "operation_id": "<uuidv7>",
  "resource_id": "<stable-resource-uuid>",
  "expected_generation": 42,
  "release_version": 7,
  "reason": "malicious",
  "request_hash": "<sha256>"
}
```

`release_version` omitted means whole-resource scope. Exact release scope is valid only for skills. `reason` is required for quarantine and is 1–500 Unicode scalar values. Unquarantine sends an empty reason.

The request hash is SHA-256 over an endpoint-specific v1 domain plus RFC-8785 canonical JSON of `operation_id`, `resource_id`, `expected_generation`, optional `release_version`, and `reason`. Quarantine and unquarantine use distinct hash domains. The `(operator_id, operation_id)` outcome is stored atomically with the domain mutation; exact retries replay and conflicting operation reuse fails.

A changed quarantine bumps the resource generation and commits a `resource_quarantined` or `resource_unquarantined` authority event plus outbox wake. A semantic no-op is audited but does not fabricate a new generation.

## Quarantine reconciliation

`SyncReconcileResponse`, `SubscriptionCatalog`, and `PrivateSkillCatalog` may include `quarantined: QuarantinedResource[]` entries:

```json
{
  "resource_id": "<stable-resource-uuid>",
  "locator": "@owner/skill",
  "release_version": 7,
  "reason": "malicious"
}
```

`release_version` is omitted for whole-resource quarantine. `revision_id` is optional by design. Content RLS may already hide the quarantined release/workspace; an existing authorized client uses its local relationship plus the quarantine scope to decide whether its currently materialized generation must be preserved and removed. The server never needs to reveal a quarantined manifest, snapshot location, blob identity, or revision solely to deliver the security tombstone.

A quarantined relationship is not also reported as an ordinary removal. This distinction lets the client preserve the affected local generation under `~/.denju/quarantine/<resource-id>/<revision-id>/` before journaled projection removal. Exact historical-release quarantine leaves a different active release untouched. No version fallback occurs.

Pack member resolution uses the stable unavailable reason `quarantined`; authored pack membership and immutable pack revisions remain unchanged.

Retain-on-delete grants read authority only to the deleted resource's exact tombstone release/revision. The same resource/exact-release quarantine check is applied first, so retention cannot expose a quarantined tombstone.

## Database isolation contract

The security migrations establish separate direct login roles for ordinary request SQL (`denju_app`) and durable worker/recovery SQL (`denju_worker`). Both are `NOSUPERUSER`, `NOCREATEROLE`, and `NOBYPASSRLS`; runtime startup rejects role switching/bypass-capable connection identities. Actor user/installation context is stored with transaction-local PostgreSQL settings only.

RLS is defense in depth for private/team/resource/object relationships. Security-definer helpers do not become generic read APIs. They expose only narrowly required capabilities such as exact bearer-hash authentication, exact semantic object persistence, pack-specific pending release-event catch-up for a pack the actor may manage, and tombstone-release reads for an actor with a retained direct subscription.

Canonical blob keys, Merkle IDs, event IDs, and guessed resource/revision UUIDs do not grant access by possession.

## Request and recovery boundaries

The Axum server caps normal JSON bodies at 2 MiB. Non-loopback public/S3 origins must be HTTPS and URL userinfo is rejected. Published/private snapshot GET presigns expire after 300 seconds; staging PUT presigns expire after 600 seconds. Provider signature validation rejects tampered URLs.

`POST /v1/internal/packs/drain` is protected by the dedicated `DENJU_RECOVERY_TOKEN`, not by an end-user, automation, or operator bearer. Duplicate authorized drain calls are safe and idempotent because durable release events/current pack state remain authoritative.
