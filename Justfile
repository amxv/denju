set default-list

# Full repository handoff gate. Implementation lives in xtask.
check:
    cargo xtask check

# Rust-only repository gate.
rust:
    cargo xtask rust

# Documentation type-check.
docs:
    cargo xtask docs

# Start the pinned PostgreSQL + Garage dependencies and registry process.
dev:
    cargo xtask dev

# Fast compile check for one workspace package.
check-crate crate:
    cargo check -p {{ crate }}

# Tests for one workspace package.
test crate:
    cargo test -p {{ crate }}

# Clippy for one workspace package.
clippy crate:
    cargo clippy -p {{ crate }} --all-targets -- -D warnings

# Run the docs site locally.
docs-dev:
    bun run docs:dev

# Validate the published npm shim only.
npm-check:
    bun run npm:check
