# SQLx offline metadata

Denju currently uses SQLx's runtime `query`, `query_as`, and `query_scalar` APIs rather than the
compile-time query macros. Those APIs do not generate `.sqlx/query-*.json` metadata.

`cargo xtask contracts` enforces this boundary: introducing a compile-time SQLx query macro must
also introduce checked-in offline metadata, while stale metadata is rejected when no macro needs
it. This keeps ordinary compilation and `cargo xtask check` independent of a live database.
