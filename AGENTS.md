# AGENTS.md

Guidance for coding agents working in `agentbox-skill-share`.

## Purpose

This repository ships a Go CLI that packages complete directories from an agent
skills root and shares the archive through the local Agentbox CLI. It is
distributed as native GitHub release binaries through an npm wrapper.

## Architecture

- `cmd/agentbox-skill-share/main.go`: process entrypoint and exit handling.
- `internal/app/`: flags, validation, help, and workflow orchestration.
- `internal/bundle/`: deterministic skill validation and `.tar.gz` creation.
- `internal/agentbox/`: JSON-mode Agentbox CLI adapter.
- `internal/buildinfo/`: build-time version plumbing.
- `bin/agentbox-skill-share.js`: npm shim for the packaged native binary.
- `scripts/postinstall.js`: downloads a release binary, with a local Go fallback.
- `.github/workflows/release.yml`: tag-driven binary and npm release pipeline.
- `src/`: Astro/ZueDocs documentation site.

## Local commands

```bash
make fmt
make test
make vet
make lint
make check
make build
bun run docs:check
bun run docs:build
```

Run the two docs commands serially.

## Product guardrails

- Validate every requested skill before creating an Agentbox thread.
- Archive full skill directories. Do not omit scripts, references, assets, or
  executable mode bits.
- Keep archive paths rooted at plain skill names and reject traversal names.
- Refuse to overwrite an existing archive.
- Keep team sharing explicit. Do not publish a thread publicly from this CLI.
- Keep the Agentbox adapter on `--json` because its response is parsed by code.
- Preserve the create, attach, then share order so incomplete threads are not
  exposed to a team.

## Release contract

Release assets are named `<binary>_<goos>_<goarch>[.exe]`. If that convention
changes, update the workflow and `scripts/postinstall.js` together.

The tag workflow expects `NPM_TOKEN` before the first release. Update
`src/content/docs/changelog.md`, run all checks, push `main`, and only then push
a `v*` tag.
