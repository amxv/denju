# Denju Docs Agent Notes

`docs/` is a ZueDocs consumer, not a standalone design system. Keep product documentation local here and keep the shared presentation shell in the `zuedocs` package.

## Authoring contract

Every file under `src/content/docs/` must provide:

- `title`
- `description` — the page promise used in page metadata and docs cards
- `summary` — the shorter description used where compact navigation needs it
- `order`
- `category` — one of the values in `src/data/docs.ts`

Structure articles with useful `##` headings. ZueDocs derives the right-side table of contents from those headings, so a wall of unsectioned prose produces a visibly broken article layout.

## Shared shell contract

Preserve the standard ZueDocs consumer routes:

- `/docs` uses the grouped card index and page actions.
- `/docs/<slug>` uses the Docs Map sidebar, `DocsPageLayout`, page actions, and heading-derived table of contents.
- raw Markdown remains available at `/docs.md` and `/docs/<slug>.md`.

Do not replace those routes with hand-written minimal `<main>` wrappers or copy shared ZueDocs layouts/components into Denju. If the shared shell itself needs a new capability, change `zuedocs` and consume a released package version.

Keep branding, navigation, accents, and footer content in `src/data/docs.ts`. Avoid transient implementation-status copy in shared metadata; documentation pages should describe behavior that exists on the current branch.

## Validation and deployment

Run Astro validation serially:

```bash
bun run docs:check
bun run docs:build
```

Use `bun run docs:dev` for local preview.

The Vercel documentation project intentionally builds only when `docs/` changes. `docs/scripts/should-build.mjs` owns that Git diff decision and fails open when Vercel's comparison SHAs cannot be proven. Its regression tests are part of `docs:check`.

Vercel installs only the `@denju/docs` workspace. Do not broaden the docs install back to the whole Bun workspace: doing so runs the published `denju-cli` postinstall during documentation builds.
