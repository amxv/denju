---
title: Repository layout
description: Ownership boundaries for the Rust workspace and distribution surfaces.
order: 3
category: Reference
---

- `apps/denju` — CLI and background-daemon process wiring.
- `apps/denju-server` — hosted registry process wiring.
- `crates/` — domain, wire, sync, local, client, registry, and testkit ownership boundaries.
- `xtask/` — canonical developer and CI command surface.
- `packages/npm/` — thin native-binary installer/launcher only.
- `docs/` — Astro/ZueDocs documentation site.
- `tmp/gg/` — ignored local planning and agent handoff artifacts.
