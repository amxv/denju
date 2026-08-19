---
title: Release and npm publishing
description: Understand the tag-driven GitHub release workflow and the npm wrapper package contract.
order: 3
category: Distribution
summary: How native binaries, GitHub releases, and npm publishing fit together.
---

## Release trigger

The release workflow runs when a `v*` tag is pushed. Prepare the changelog and
tag from the repository root:

```bash
make check
bun run docs:check
bun run docs:build
# edit src/content/docs/changelog.md
git add src/content/docs/changelog.md
git commit -m "docs: update changelog for v0.2.0"
git push origin HEAD
make release-tag VERSION=0.2.0
```

The changelog is intentionally edited and reviewed by the agent or maintainer.
`make release-tag` only creates and pushes the tag. GitHub Actions performs the
release work after the tag arrives.

## What the workflow does

The GitHub Actions workflow:

1. runs Go and Node quality checks
2. builds native binaries for supported OS/architecture targets
3. uploads binaries to a GitHub Release
4. publishes the npm package with the version from the tag

The npm package itself stays small. It contains the JavaScript shim, postinstall script, Go source, and project files needed for fallback local builds.

## Binary asset names

Release assets use this shape:

```bash
<binary>_<goos>_<goarch>[.exe]
```

Examples:

```bash
mycli_darwin_arm64
mycli_linux_amd64
mycli_windows_amd64.exe
```

Do not change this naming convention unless you update the workflow and `scripts/postinstall.js` together.

## npm install behavior

After npm install, `scripts/postinstall.js` tries to download the right native binary from GitHub Releases.

If the download cannot be completed, it falls back to building locally with Go when possible.

## Required secret

Publishing to npm requires this GitHub secret:

```bash
NPM_TOKEN
```

Make sure the package name in `package.json` is available and publishable before tagging a release.

The default package license is Apache-2.0 and is kept in sync with `LICENSE` and
the bootstrap metadata. Run `make bootstrap BOOTSTRAP_ARGS='--license MIT'` when a project
needs a different supported license and has the optional `license-generator` CLI
installed.

## Repository visibility

The normal npm and release path assumes a public GitHub repository so postinstall
can download release assets anonymously. If a project explicitly chooses a
private repository with anonymous npm installation expectations, it must arrange
authenticated or bundled assets in the shipping workflow.
