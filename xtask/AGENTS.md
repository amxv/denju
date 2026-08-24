# `xtask`

Root `AGENTS.md` applies. `xtask` is the canonical repository automation, generation, CI, and development-orchestration surface.

## Invariants

- nontrivial repository automation has one owner here rather than duplicated shell/Just/Make logic
- `Justfile` may expose thin aliases only
- `cargo xtask check` remains the comprehensive deterministic handoff/CI gate
- `cargo xtask dev` owns the repeatable lifecycle around the pinned Compose dependencies and registry process

Keep runtime product logic out of xtask.

## Fast check

`cargo check -p xtask`
