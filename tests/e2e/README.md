# External end-to-end coverage

Docker-backed end-to-end lifecycle coverage is owned by `cargo xtask load` so it is explicit,
repeatable, and not accidentally run by ordinary unit-test invocations. The harness uses real
registry HTTP, PostgreSQL, S3-compatible object storage, ordinary server processes, and an
explicit marked `DENJU_TEST_HOME` for every CLI invocation.

Unit/integration tests under the Rust crates remain deterministic and dependency-free; external
service tests belong here or in the xtask harness, never in shell-script automation.
