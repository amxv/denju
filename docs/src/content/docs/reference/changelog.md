---
title: Changelog
description: User-facing Denju release history.
order: 43
category: Reference
summary: New CLI capabilities, synchronization behavior, projection changes, upgrades, and compatibility fixes.
---

This changelog tracks code and product changes in Denju. It intentionally skips docs-site-only updates.

## 0.3.8 — 2026-08-27

- Fixed rename authorization so team-owned skills are resolved through the authenticated actor transaction before the rename is applied.
- Improved built-in CLI help across Denju's command tree, including useful descriptions and examples for commands such as `rename`.
- Made the agent verification loop substantially cheaper with scoped package detection and faster local Rust build defaults.

## 0.3.7 — 2026-08-27

- Stabilized skill projections when `~/.agents/skills` and another configured harness root resolve to the same directory, avoiding duplicate reconciliation work and projection oscillation.
- Made Denju recognize its own canonical projections through symlink resolution while still treating unrelated occupied names as unmanaged collisions.
- Kept collision aliases stable while a conflict exists and returned a skill to its canonical name cleanly once that conflict disappears.

## 0.3.6 — 2026-08-27

- Added `denju list` to show every skill tracked on the machine, including visibility, release version when available, active source, and pack relationships. JSON output also includes canonical and harness projection paths.
- Added clear progress output to `denju upgrade` for update checks, downloads, verification, installation, background-service restart, and rollback without leaking package-manager noise into the normal CLI experience.
- Hardened macOS background-service restarts during in-place upgrades so the existing LaunchAgent can reliably start the newly installed executable.

## 0.3.5 — 2026-08-27

- Moved the shared Agent Skills projection to flat entries directly under `~/.agents/skills`, matching the common cross-harness discovery layout instead of keeping Denju skills in a nested namespace.
- Preserved custom Claude skill roots while migrating older managed Codex projections to the shared Agent Skills root.
- Added stable, readable collision aliases for unmanaged names and preserved those assignments across reconciliation instead of renumbering projections unnecessarily.

## 0.3.4 — 2026-08-25

- Added first-class self-upgrade support for Denju installations managed by Vite+, alongside npm and standalone installations.
- Hardened upgrades with exact release-manifest size and checksum verification, post-install health checks, and automatic rollback to the previous working version when verification fails.
- Taught the npm launcher to detect its installation context so `denju upgrade` uses the package manager that actually installed Denju.

## 0.3.3 — 2026-08-25

- Fixed background synchronization for custom Claude Code installations by persisting the resolved Claude skills root and reusing it when the daemon does not inherit `CLAUDE_CONFIG_DIR`.

## 0.3.2 — 2026-08-25

- Added repository checks that keep the Rust workspace, npm package, container image, release workflow, and generated lockfiles on one coherent Denju version.
- Fixed release metadata drift so all distribution surfaces advance together when a version is cut.

## 0.3.1 — 2026-08-25

- Fixed npm-managed upgrades on Windows by using the correct installed executable path and release-smoke behavior.
- Hardened upgrade acceptance coverage so release verification exercises the installed CLI and upgrade path rather than only build-time artifacts.

## 0.3.0 — 2026-08-25

- Shipped the first official Rust Denju release with identity and account credentials, public discovery and subscriptions, local skill import, private workspace synchronization, immutable publishing, history, diffing, and the full owned-resource lifecycle.
- Added private skill sharing, automatic forks, upstream proposals, conflict-aware synchronization, reproducible packs, teams, team-assigned packs, transfers, and social discovery.
- Added deterministic local generations and harness projections with isolated test homes, plus registry quarantine and validation boundaries for untrusted content.
- Added the hosted registry/server deployment path, standalone installers, npm distribution, release binaries, checksums, container images, and cross-platform release verification.
