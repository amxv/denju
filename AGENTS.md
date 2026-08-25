# AGENTS.md

Denju is an actively maintained Rust product. Treat the current repository as the source of truth for implementation, behavior, tests, documentation, and operational contracts.

The former Go/Agentbox implementation exists only as historical Git data on `legacy/go-agentbox-v0.2.0`. Do not use it for implementation guidance or behavioral parity unless the user explicitly asks for historical investigation.

Old implementation plans and progress artifacts under `tmp/gg/` are historical context, not instructions for normal maintenance work. Future changes should be driven by the current codebase and the task at hand rather than old phase boundaries.

## Start here

1. Inspect the current branch and working tree before changing anything. Preserve concurrent work you do not own.
2. Explore the current repository areas relevant to the task: source, tests, docs, fixtures, deployment code, and nearby contracts. Follow behavior across crate/app boundaries when necessary instead of stopping at the first matching file.
3. Read the nearest `AGENTS.md` if a subtree adds one in the future.
4. Use current tests, public CLI/API behavior, wire formats, migrations, and documentation to understand existing contracts before changing them.
5. Prefer the smallest coherent change that fits the existing architecture. If behavior is unclear, investigate the implementation and tests first rather than inferring it from historical plans.

## Repository map

- `apps/denju/` — CLI + background daemon process wiring.
- `apps/denju-server/` — registry process wiring.
- `crates/denju-core/` — pure IDs, paths, Merkle/revision/merge domain logic.
- `crates/denju-wire/` — versioned JSON, CLI structured output, API/SSE contracts.
- `crates/denju-sync/` — deterministic synchronization state machine, no I/O.
- `crates/denju-local/` — SQLite, filesystem generations, watchers, projections, OS services.
- `crates/denju-client/` — HTTPS/SSE/auth/object-transfer client.
- `crates/denju-registry/` — registry use cases, PostgreSQL, S3, search, outbox.
- `crates/denju-testkit/` — shared deterministic fixtures only.
- `xtask/` — canonical developer/CI commands.
- `Justfile` — thin discoverable aliases only; recipes delegate to Cargo/xtask/Bun and contain no build logic.
- `packages/npm/` — thin binary installer/launcher; never a source-build fallback.
- `docs/` — Astro/ZueDocs site.
- `deploy/` — development, container, and self-hosting deployment surfaces.

Keep dependencies one-way toward `denju-core`; binaries wire product logic rather than owning it. Do not introduce generic utility crates or traits without a real ownership or I/O boundary.

## Working style

- Explore before editing. Search for callers, tests, wire types, migrations, and docs that describe the behavior you are changing.
- Keep public terminology user-facing. Internal Rust/database vocabulary should not leak into docs or CLI copy unless users genuinely need it.
- Preserve stable IDs, wire compatibility, migration safety, and deterministic synchronization semantics unless the task explicitly changes those contracts.
- Prefer existing repository patterns over introducing parallel abstractions or second command paths.
- Keep large files under control; use `locguard` while iterating when useful.

## Verification

Use the narrowest useful check while iterating, then run the scoped full check appropriate to the change before handoff:

```bash
just
just check-crate denju-core
just test denju-core
cargo check -p <crate>
cargo test -p <crate>
cargo clippy -p <crate> --all-targets -- -D warnings
cargo xtask check
```

`cargo xtask check` is the canonical repository-wide gate and CI contract. `just` is only the low-friction command menu; never duplicate Rust build/generation/dev logic in the Justfile. Do not add a Makefile as a second command authority. The docs and npm workspaces are intentionally separate from runtime Rust code.

For line-count feedback, `locguard` checks the dirty/changed source tree quickly and `locguard scan` checks the complete eligible repository. It supplements, never replaces, the relevant Cargo/xtask/docs checks.

## Safety

- Keep `tmp/gg/` local and untracked.
- Never commit credentials, local databases, object blobs, or live private fixtures.
- **Tests and live acceptance fixtures must use `DENJU_TEST_HOME` pointing at a dedicated disposable directory containing `.denju-test-home-v1`.** Test mode intentionally ignores inherited `CODEX_HOME` and `CLAUDE_CONFIG_DIR`, forces file credentials, and never starts the real background service. Do not simulate isolation by changing only `HOME`.
- No test/e2e/acceptance run may read, write, migrate, remove, or project into the developer's real harness homes. On this machine the custom homes are `~/.gg/codex/` and `~/.gg/claude/`; the standard homes `~/.codex/`, `~/.claude/`, and `~/.agents/` are equally protected. All harness fixtures must remain beneath `DENJU_TEST_HOME`.
- Preserve the single Rust implementation path on `main`; no Go or Agentbox fallback.
