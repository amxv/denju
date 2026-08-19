---
title: Changelog
description: Release notes for denju.
order: 99
category: Distribution
summary: Product changes prepared for the next tagged release.
---

## 0.2.0 - 2026-08-20

- Rename the repository, Go module, and terminal command to `denju`.
- Publish the npm distribution under the available `denju-cli` package name.
- Keep the documentation at `denju.ashray.xyz` with GitHub push-to-deploy.

## 0.1.0 - 2026-08-20

- Initial release under the `agentbox-skill-share` name.
- Package complete agent skill directories into portable `.tar.gz` bundles.
- Create an Agentbox thread, attach the bundle, and grant explicit team access.
- Support package-only output for local inspection and offline handoffs.
- Preserve nested scripts, references, assets, symlinks, and executable modes.
- Refuse unsafe skill names and existing archive destinations.

## Maintainer notes

When adding future entries:

- Keep the newest version at the top.
- Add sections only for version tags that exist.
- Summarize code and product changes from `main` since the previous version tag.
- Include internal code changes when they affect maintainers or release safety.
- Do not include docs-site-only changes such as site styling, docs package bumps, deploy plumbing, footer/layout changes, or documentation navigation changes.
- Rewrite commit subjects into clear release notes instead of pasting raw commit messages.
- If a release contains only tagging/release metadata, write: `Maintenance release. No direct code behavior changes beyond release preparation.`
