set default-list

# Fast type-check for one workspace package while iterating.
check package:
    cargo check -p {{ package }}

# Run all tests for one workspace package.
test package:
    cargo test -p {{ package }}

# Run one integration-test binary, e.g. `just test-target denju cli`.
test-target package target:
    cargo test -p {{ package }} --test {{ target }}

# Fast Clippy for one or more packages; dependencies are compiled but not linted.
lint +packages:
    python3 scripts/scoped_verify.py lint {{ packages }}

# Scoped handoff gate. With no args, infer changed packages and reverse dependents.
verify *packages:
    python3 scripts/scoped_verify.py verify {{ packages }}

# Comprehensive repository gate used by CI/release. Deliberately expensive.
full:
    cargo xtask check

# Comprehensive Rust-only workspace gate. Deliberately expensive.
rust-full:
    cargo xtask rust

# Documentation type-check.
docs:
    cargo xtask docs

# Start the pinned PostgreSQL + Garage dependencies and registry process.
dev:
    cargo xtask dev

# Run the docs site locally.
docs-dev:
    bun run docs:dev

# Validate the published npm shim only.
npm-check:
    bun run npm:check
