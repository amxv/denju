---
title: Customize the CLI
description: Rename the starter command, update project identity, and replace the starter command handlers safely.
order: 2
category: Development
summary: The main checklist for turning mycli into a real Go command-line tool.
---

## Initialize identity

Run one bootstrap command before adding real behavior:

```bash
make bootstrap BOOTSTRAP_ARGS='--cli-name pluck \
  --binary-name pluck \
  --go-module github.com/acme/pluck \
  --github-owner acme --github-repo pluck \
  --npm-package @acme/pluck \
  --description "A fast file picker" \
  --homepage https://pluck.dev \
  --canonical-url https://pluck.dev \
  --license Apache-2.0'
```

The script updates `cmd`, the npm wrapper, `package.json`, `go.mod`, `Makefile`,
the release workflow, postinstall metadata, and `src/data/docs.ts` together. The
CLI name and release binary name may be kept equal, or set independently.

## Update package identity

The bootstrap command updates package metadata before publishing:

```bash
package.json name
package.json description
package.json repository
package.json homepage
package.json bugs
package.json keywords
go.mod module path
```

The postinstall script reads repository/package metadata to find release binaries, so stale package metadata can break npm installs.

## Replace command logic

The starter app logic lives in:

```bash
internal/app/app.go
internal/app/app_test.go
```

Keep parsing and handlers small at first. Add command-specific help text as command groups grow.

## Keep version plumbing

`internal/buildinfo` exposes the version used by `--version`. Release builds inject the version with Go linker flags.

For local development, the Makefile reads the version from `package.json` and passes it into the build.

## Validate after changes

Run the full local check before pushing:

```bash
make check
bun run docs:check
bun run docs:build
```

Run the docs commands serially. Astro can be sensitive to concurrent check/build work in the same repo.

The default license is Apache-2.0 and needs no extra CLI. Pass `--license MIT`
(or another value from `license-generator --list`) inside `BOOTSTRAP_ARGS` to
regenerate `LICENSE` and synchronize the metadata when the optional
`license-generator` CLI is installed. Without it, bootstrap continues with
Apache-2.0 and prints a warning.
