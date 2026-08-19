---
title: Docs site maintenance
description: Run, edit, validate, and deploy the ZueDocs-powered documentation site embedded in this Go CLI template.
order: 5
category: Reference
summary: Developer notes for maintaining the Astro docs app alongside the Go CLI.
---

## Local development

Install dependencies and start Astro:

```bash
bun install
bun run docs:dev
```

Astro serves the docs locally, usually at `http://localhost:4321`.

## Files to edit

The docs site is intentionally small:

```bash
src/data/docs.ts               # site name, repo URL, nav, categories
src/pages/index.astro          # landing page
src/pages/docs/index.astro     # docs index
src/pages/docs/[...slug].astro # article route
src/content/docs/*.md          # documentation pages
src/styles/global.css          # shared ZueDocs import
```

For most updates, edit markdown in `src/content/docs` first.

## ZueDocs package usage

This site imports the shared docs shell from `zuedocs`:

```astro
import BaseLayout from "zuedocs/layouts/BaseLayout.astro";
import DocsPageLayout from "zuedocs/layouts/DocsPageLayout.astro";
```

Local repos should keep their own docs content and `src/data/docs.ts`, while shared shell behavior belongs in the `zuedocs` package.

## Validate changes

Run:

```bash
bun run docs:check
bun run docs:build
```

Run these commands serially. Do not run Astro check and build concurrently in the same repo.

## Deployment

The docs site builds to static output in `dist`:

```bash
bun run docs:build
```

For Vercel or another static host, use `dist` as the output directory. If that conflicts with CLI release artifacts in your deployment setup, configure the host to run the docs build in a clean build environment.
