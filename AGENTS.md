# AGENTS.md

Guidance for coding agents working in `go-cli-template`.

## Purpose

This repo is a generic starter for Go command-line tools distributed through npm.

The sample command is `mycli`. Replace it with your actual CLI name and behavior.

## Architecture

- `cmd/mycli/main.go`: process entrypoint, error handling, exits non-zero on failure.
- `internal/app/app.go`: command parser + handlers.
- `internal/app/app_test.go`: starter tests.
- `bin/mycli.js`: npm shim that invokes packaged native binary.
- `scripts/postinstall.js`: downloads release binary on install, falls back to `go build`.
- `scripts/setup.js`: initializes command, module, repository, npm, docs, and license identity.
- `project.config.json`: canonical repeated project identity fields.
- `.github/workflows/release.yml`: tag-driven release pipeline.
- `src/`: ZueDocs-powered Astro documentation site for future CLIs.

## Local commands

Use `make` targets:

- `make fmt`
- `make test`
- `make vet`
- `make lint`
- `make check`
- `make build`
- `make build-all`
- `make install-local`
- `make bootstrap BOOTSTRAP_ARGS='...'`

Direct commands:

- `go test ./...`
- `go vet ./...`
- `npm run lint`
- `bun run docs:check`
- `bun run docs:build`

## How to customize safely

1. Run `make bootstrap` to rename the CLI command consistently.
The bootstrap script updates the command path, npm wrapper, package metadata, Go
module, Makefile, workflow, postinstall, docs config, and license metadata.

2. Keep binary naming convention unchanged unless you also update postinstall/workflow:
- release assets: `<binary>_<goos>_<goarch>[.exe]`
- npm-installed binary path: `bin/<binary>-bin` (or `.exe` on Windows)

3. If adding dependencies, commit `go.sum` and optionally enable Go cache in workflow.

4. Keep help output expressive and command-local (`<command> --help` should explain examples).

5. Update the bundled docs site when command behavior changes:
- `src/data/docs.ts` for site name, repo URL, nav, footer sections, and categories
- `src/pages/index.astro` for landing-page product copy
- `src/content/docs/*.md` for guides and command reference

6. ZueDocs consumer pattern:
- import shared layouts from `zuedocs/layouts/*`
- import shared behavior/styles from `zuedocs/docsEnhancements` and `zuedocs/styles.css`
- keep repo-specific content/config local instead of forking shared shell files

## Release contract

Release pipeline triggers on `v*` tags and expects:

- `NPM_TOKEN` GitHub secret present before the first tag-driven release.
- npm package name in `package.json` is publishable under your account/org.
- repository URL matches the release origin used by `scripts/postinstall.js`.
- public GitHub visibility is the normal anonymous npm install path.

## Guardrails

- Prefer additive changes; do not break the release asset naming contract unintentionally.
- If you change release artifacts or CLI binary name, update both workflow and postinstall script in the same PR.
- Run docs validation serially (`bun run docs:check` then `bun run docs:build`); do not run Astro check/build concurrently in the same repo.

## Changelog and Release Skill

This template includes:

- `src/content/docs/changelog.md` as a placeholder changelog page for generated projects.
- `.agents/skills/release/SKILL.md` as a release checklist that updates the changelog before tagging.

When customizing the template for a real CLI, keep both files and update the
placeholder changelog manually before each release. Run the checks and docs
validation, commit and push the changelog, then push a `v*` tag. GitHub Actions
builds the release assets and publishes npm.
