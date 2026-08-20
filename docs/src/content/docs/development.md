---
title: Development
description: The small set of commands agents and contributors need at the scaffold stage.
order: 2
category: Development
---

From the repository root:

```bash
cargo xtask check
cargo build --workspace
bun run docs:dev
```

Rust is the primary project. The root Bun workspace exists only for the documentation site and the published npm installer shim.
