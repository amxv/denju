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
- `scripts/scoped_verify.py` — zero-build-cost package selection for the fast agent verification loop.
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

## Verification: keep the feedback loop scoped

Use `just` as the agent-facing command menu. The obvious commands are intentionally the cheap/scoped ones; commands containing `full` are the expensive repository-wide gates.

```bash
just                         # show the command menu
just check denju             # fastest type-check for one package
just test denju              # all tests for one package
just test-target denju cli   # one integration-test binary
just lint denju              # fast Clippy, default targets, no dependency linting
just lint denju denju-registry
just verify                  # canonical pre-handoff gate; auto-detect changed packages
just verify denju denju-registry  # explicit scoped pre-handoff gate
just full                    # comprehensive CI/release gate
```

### Iteration order

1. While editing, run the **smallest command that can disprove the current change**. For a source edit this is usually `just check <package>`; for a known integration-test surface use `just test-target <package> <test-target>`.
2. Once the implementation settles, use `just lint <package...>` if Clippy feedback is useful before the final gate. Pass all related packages in **one invocation** rather than running Clippy once per package.
3. Before handoff, run `just verify` once. It finds Rust packages changed relative to `origin/main`, adds their workspace reverse dependents to the compile/Clippy closure, runs one `--all-targets --no-deps` Clippy invocation, then tests the packages actually changed.
4. Use `just full` only when the task genuinely needs the whole repository gate: release work, CI/repository automation changes, broad workspace changes, or when `just verify` escalates automatically because a shared Rust input changed.

### Avoid redundant Cargo modes

- Clippy already type-checks. Do **not** routinely run `cargo check -p foo` immediately before `just lint foo` or `just verify foo` just for confidence.
- Do **not** run `cargo clippy -p a ... && cargo clippy -p b ...`; use `just lint a b` so Cargo sees one graph.
- Do **not** use `--all-targets` during the inner loop unless the changed code is target-specific. `just verify` owns the all-targets scoped gate.
- Do **not** run the full workspace gate after every edit. A change in `apps/denju/` should not compile unrelated registry/server surfaces unless dependency direction requires it.
- Do not `cargo clean` to fix ordinary build issues; it destroys the incremental state that makes subsequent agent loops fast.

For example, a CLI help change plus a small `denju-registry` rename fix should normally look like:

```bash
just test-target denju cli
just check denju-registry
# iterate until behavior is right
just verify
```

That replaces the much more expensive pattern of running all `denju` tests, a registry check, and then separate all-target Clippy commands for each package.

`cargo xtask check` remains the canonical repository-wide CI contract behind `just full`. `just` is the discoverable UX and stays thin. Lightweight package selection lives in `scripts/scoped_verify.py` so the fast path does not have to compile xtask; heavyweight generation, deployment, release, and repository-wide automation stays in `xtask`. Do not add a Makefile as a second command authority. The docs and npm workspaces are intentionally separate from runtime Rust code.

For line-count feedback, `locguard` checks the dirty/changed source tree quickly and `locguard scan` checks the complete eligible repository. It supplements, never replaces, the relevant Cargo/xtask/docs checks.

## Safety

- Keep `tmp/gg/` local and untracked.
- Never commit credentials, local databases, object blobs, or live private fixtures.
- **Tests and live acceptance fixtures must use `DENJU_TEST_HOME` pointing at a dedicated disposable directory containing `.denju-test-home-v1`.** Test mode intentionally ignores inherited `CODEX_HOME` and `CLAUDE_CONFIG_DIR`, forces file credentials, and never starts the real background service. Do not simulate isolation by changing only `HOME`.
- No test/e2e/acceptance run may read, write, migrate, remove, or project into the developer's real harness homes. On this machine the custom homes are `~/.gg/codex/` and `~/.gg/claude/`; the standard homes `~/.codex/`, `~/.claude/`, and `~/.agents/` are equally protected. All harness fixtures must remain beneath `DENJU_TEST_HOME`.
- Preserve the single Rust implementation path on `main`; no Go or Agentbox fallback.
