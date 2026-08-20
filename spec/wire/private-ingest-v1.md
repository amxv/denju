# Denju private skill ingest wire contract v1

`denju import PATH` is an authenticated authority operation. The source directory is validated and snapshotted locally first; the registry does not receive an unverified archive as authority.

Private ingest endpoints:

- `POST /v1/private-skills/imports/prepare` — user session or automation bearer with `skills:write`; reserves the stable resource/revision identity for one UUIDv7 operation and returns presigned random staging uploads only for blobs that this namespace has not already proven.
- `POST /v1/private-skills/imports/commit` — verifies every required staged object by exact size and SHA-256, reconstructs the declared Merkle tree and deterministic snapshot, enforces logical namespace quota, promotes canonical content, and atomically commits the private resource/workspace plus the idempotent outcome.
- `GET /v1/private-skills` — user session or automation bearer with `skills:read`; returns the account namespace's owned private workspaces plus short-lived authorized snapshot downloads.

The prepare request carries `operation_id`, `expected_generation`, skill name, semantic manifest, deterministic snapshot checksum/size, and an RFC-8785 request hash under the private-import v1 domain. Initial imports require `expected_generation=0`. Exact operation retries return the original reserved/committed identity; reusing an operation ID with different canonical request content returns `operation_conflict`.

Blob existence is namespace-private. A namespace's first reference to a blob always receives its own random staging authorization even if the same SHA-256 already exists physically for another tenant. Only after the server streams and verifies that namespace's staged bytes may physical content be deduplicated. `namespace_blob_reachability` charges unique logical bytes independently per namespace.

The registry stores canonical file bytes under content-addressed S3-compatible keys only after verification. Merkle trees, revisions, resource reachability, the private workspace ref, quota reachability, and the committed mutation outcome are PostgreSQL authority. Hash possession is never download authorization.

The client journals import locally. The original source remains untouched through validation, staging, remote commit, managed generation materialization, and harness projection. Source removal is the final successful local switch. Retrying an interrupted import resumes the same UUIDv7 operation from durable staged/verified/switched state rather than creating a second resource.

Current provider conformance uses one generic S3-compatible adapter for Garage and Cloudflare R2. The probe covers presigned PUT, SDK read/write, presigned GET, immutable-write retry, and idempotent delete. R2 live evidence requires deployment credentials and is a release-readiness requirement when those credentials are available.
