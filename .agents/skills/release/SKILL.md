---
name: release
description: Prepare a Go CLI changelog and push a tag for the GitHub Actions release workflow.
allowed-tools: Bash, Read, Write, Edit
---

# Release a Go CLI from this template

Use this skill when cutting a release for a project generated from
`go-cli-template`. The release is intentionally controlled by the repository:
edit the changelog, commit it, and push a `v*` tag. GitHub Actions builds the
native binaries, creates the GitHub Release, and publishes npm.

## Preconditions

- You are on `main` and synced with `origin/main`.
- The working tree is clean except intentional release edits.
- The package name in `package.json` is publishable.
- The `NPM_TOKEN` GitHub secret is configured.
- The release workflow exists at `.github/workflows/release.yml`.

## Changelog first

Before tagging, manually update `src/content/docs/changelog.md`:

- prepend a section for the exact version being released
- keep the newest version at the top
- summarize code and product changes since the previous version tag
- rewrite technical commit subjects into clear release notes
- omit docs-site-only changes such as styling, layout, and navigation changes
- if the release contains only tag/version plumbing, write: `Maintenance release. No direct code behavior changes beyond release preparation.`

Do not use a script or release target to generate the changelog. The release
notes are part of the normal project work and should be reviewed with the
other changes.

## Validate and tag

Run the checks serially from the repository root:

```bash
make check
bun run docs:check
bun run docs:build
```

Commit and push the changelog, then push the tag:

```bash
git add src/content/docs/changelog.md
git commit -m "docs: update changelog for v${VERSION}"
git push origin HEAD
make release-tag VERSION=${VERSION}
```

`make release-tag` only creates and pushes `v${VERSION}`. Use the equivalent
Git commands when you need different tag or remote control.

## Verify

```bash
gh run list --limit 5 --json databaseId,displayTitle,headBranch,headSha,status,conclusion,url,workflowName
gh run watch <run-id>
gh release view "v${VERSION}" --json tagName,name,url,assets
npm view "$(node -p "require('./package.json').name")" version
```

Report the tag, workflow URL, release URL, asset names, and published npm
version. The template does not sync local keychains, validate release secrets,
or orchestrate the workflow from a local `ship` command.
