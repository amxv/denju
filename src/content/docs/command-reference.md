---
title: Command reference
description: A compact map of the starter command and the files that usually change as the CLI grows.
order: 4
category: Reference
summary: Starter commands, make targets, and docs commands in one scannable page.
---

## Starter command

```bash
mycli --help
mycli hello
mycli hello <name>
mycli --version
```

Replace these commands with behavior specific to your project.

## Go development

```bash
make fmt
make test
make vet
make lint
make check
make build
make build-all
make install-local
make clean
```

## Project bootstrap and release

```bash
make bootstrap BOOTSTRAP_ARGS='--cli-name pluck --github-owner acme --github-repo pluck'
make check
bun run docs:check
bun run docs:build
# edit src/content/docs/changelog.md, then commit and push it
make release-tag VERSION=0.2.0
```

The bootstrap target updates the repeated project identity fields from the
repository root. Releases are triggered by pushing a `v*` tag after the
changelog has been edited and committed. GitHub Actions handles the release
assets and npm publication.

## Docs development

```bash
bun run docs:dev
bun run docs:check
bun run docs:build
bun run docs:preview
```

## Main files

```bash
cmd/mycli/main.go             # CLI entrypoint
internal/app/app.go           # command parser and handlers
internal/app/app_test.go      # starter tests
internal/buildinfo/buildinfo.go # version plumbing
bin/mycli.js                  # npm executable shim
scripts/postinstall.js        # release binary installer
.github/workflows/release.yml # release workflow
src/content/docs/*.md         # docs content
src/data/docs.ts              # docs site config
project.config.json           # canonical project identity
```
