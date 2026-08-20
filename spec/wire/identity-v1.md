# Denju identity and credential wire contract v1

`denju-wire` is the Rust source of truth for the current identity DTOs. Human passwords and recovery secrets are accepted only by the identity endpoints that require them and are never bearer/session storage values.

Identity/session endpoints:

- `POST /v1/identity/claim` — installation bearer; creates a user and session while adopting eligible anonymous state.
- `POST /v1/identity/login` — installation bearer; links that installation author principal to an existing user and creates a session.
- `POST /v1/identity/recover` — installation bearer; verifies and rotates the recovery secret, replaces the password, and creates a session.
- `POST /v1/identity/backup` — user session; verifies the password and rotates the recovery secret.
- `GET /v1/identity` — user session or active scoped automation bearer.
- `GET /v1/devices` / `POST /v1/devices/revoke` — user session only.
- `GET /v1/tokens` / `POST /v1/tokens` / `POST /v1/tokens/revoke` — user session only.
- `POST /v1/account/delete` — user session plus password; current implementation accepts resource-free accounts only.

Passwords use Argon2id PHC hashes. Recovery, installation, session, and automation bearer secrets are 256-bit random values; PostgreSQL stores only their hashes. Recovery and automation secrets are shown once and cannot be retrieved later.

The setup-created installation `AuthorPrincipalId` is immutable. Claim/login associates it with the user for attribution rather than changing revision transcripts. Claimed-user authorship uses a distinct user author principal.

Every identity mutation carries a UUIDv7 `operation_id` and an endpoint-domain-separated RFC-8785 request hash over its canonical non-secret intent. The committed outcome is recorded transactionally. Exact retries replay that outcome, including after recovery rotation, device revocation, or account deletion makes the original credential unusable for ordinary requests. Human-secret portions are additionally bound by an Argon2id operation-secret verifier, so changing only a password/recovery input conflicts without persisting a fast password fingerprint.

Automation-token list responses expose only token ID, scopes, creation time, and expiry. Bearer values are never returned by list/read endpoints.
