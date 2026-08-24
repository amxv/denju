# Denju public discovery and direct-subscription wire contract v1

The Rust DTOs in `denju-wire` are authoritative. This document records the public-subscription compatibility boundary alongside the checked `spec/wire/openapi-v1.json` artifact, which enumerates the complete current `/v1` method/path/authentication surface. Rust client and server code share the DTOs directly rather than generating types from OpenAPI.

Public reads:

- `GET /v1/capabilities`
- `GET /v1/search?q=<query>&limit=<1..50>&cursor=<optional>`
- `GET /v1/skills/show?locator=@owner/name`

Direct-subscription authority:

- `POST /v1/installations`
- `GET /v1/subscriptions`
- `POST /v1/subscriptions`
- `POST /v1/subscriptions/remove`

Before identity claim/login, subscription endpoints authenticate with the opaque installation bearer credential created by setup and persist installation-scoped desired state. After claim/login, a revocable user session bearer addresses account-wide desired state; anonymous direct subscriptions from that installation are adopted transactionally by the account. Automation credentials do not manage direct subscriptions. A returned subscribed skill contains current public metadata, the semantic manifest, and a short-lived snapshot download authorization. Possession of a blob or snapshot hash alone is not an authorization mechanism.

Subscription mutations carry a UUIDv7 `operation_id`, `resource_id`, `expected_generation`, and endpoint-domain-separated RFC-8785 request hash. Exact retries return the committed outcome; conflicting operation reuse or stale generations fail without changing the subscription.

Search continuation is opaque and keyset-based. Clients must not infer cursor internals or use offset pagination.

Published snapshot bytes are deterministic `tar.zst`. Clients verify the advertised snapshot byte length and SHA-256 before extraction, validate every archive entry against the portable manifest, recompute file blob IDs and the root Merkle tree, and expose only a fully verified generation.
