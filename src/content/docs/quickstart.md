---
title: Quickstart
description: Install dependencies, run the starter CLI, and start the bundled ZueDocs site.
order: 1
category: Start
summary: The fastest path from a fresh clone to a working CLI and local docs site.
---

## Clone and initialize

For a brand-new CLI, create the repository from the directory where the clone
should be created:

```bash
gh repo create acme/pluck \
  --public \
  --template amxv/go-cli-template \
  --clone
```

Then run bootstrap from the new repository root:

```bash
cd pluck
make bootstrap BOOTSTRAP_ARGS='--cli-name pluck --github-owner acme \
  --github-repo pluck --npm-package @acme/pluck --license Apache-2.0'
```

The bootstrap command runs from the repository root and never reclones. Omit
`--public` in the GitHub command for the safe private default. Use public
visibility for the normal anonymous npm installation path.

For an existing clone, run `make bootstrap` with the identity options shown in the
customization guide. No manual `mycli` rename sweep is required.

## Install dependencies

Install Go, Node.js, and Bun, then install JavaScript dependencies:

```bash
bun install
```

The repository uses Go for the CLI, Node for the npm wrapper, and Astro/ZueDocs for the docs site.

## Run the starter CLI

Use the make targets to validate and build the starter command:

```bash
make check
make build
./dist/mycli --help
./dist/mycli hello
```

The sample command is intentionally small so it is easy to replace.

## Start the docs site

Run the embedded documentation site locally:

```bash
bun run docs:dev
```

Astro usually serves the site at `http://localhost:4321`.

## First customization pass

Replace the starter command behavior in:

```bash
internal/app/app.go
internal/app/app_test.go
```

Keep the docs open while you edit so the quickstart, command reference, and release notes stay aligned with the actual CLI.
