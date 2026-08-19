# Contributor guide

## Prerequisites

- Go 1.26+
- Node.js 18+
- Bun 1+
- Agentbox CLI for live sharing tests

## Local development

```bash
bun install --frozen-lockfile
make check
make build
./dist/denju --help
```

Use `--package-only` for local behavior checks that should not create an
Agentbox thread:

```bash
./dist/denju \
  --package-only \
  --archive /tmp/example-skills.tar.gz \
  agentbox
```

Unit tests use temporary skill trees and a fake Agentbox command runner. Do not
add live network or account mutations to the default test suite.

## Documentation

```bash
bun run docs:check
bun run docs:build
```

Run these commands serially and keep command examples synchronized with the CLI.

## Release process

1. Run `make check`, `bun run docs:check`, and `bun run docs:build`.
2. Update `src/content/docs/changelog.md` and push `main`.
3. Ensure the repository has an `NPM_TOKEN` Actions secret.
4. Push `vX.Y.Z` or run `make release-tag VERSION=X.Y.Z`.

GitHub Actions builds native binaries, publishes the GitHub release, and then
publishes `denju-cli` to npm.
