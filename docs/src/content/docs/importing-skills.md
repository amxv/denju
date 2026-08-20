---
title: Import skills
description: Transfer an existing Agent Skill into a private Denju workspace without risking the source directory.
order: 4
category: Start
summary: Validate, upload, materialize, project, then remove the original discovery path.
---

Import is the first identity-bound write flow. Public search and subscriptions stay anonymous, but creating an owned skill requires a claimed or logged-in user.

```bash
denju import ~/.agents/skills/my-skill
```

The directory name and `name` in `SKILL.md` must already match. Denju validates the complete Agent Skills frontmatter and cross-platform filesystem profile before changing registry or local managed state. Dotfiles, exact file bytes, executable bits, and valid relative in-root symlinks are preserved.

Import is a transfer rather than an unmanaged copy. Denju first creates a deterministic local snapshot and Merkle manifest, stages only the blobs the current namespace must prove, verifies those bytes in the registry's S3-compatible store, commits the private resource, builds a verified local generation, and exposes Codex and Claude projections. **Only after all of that succeeds does Denju remove the original source directory.**

If the registry is offline, a staged object is corrupt, quota is exceeded, or local materialization/projection fails, the source remains available. The operation is UUIDv7-idempotent and journaled, so rerunning the same `denju import PATH` resumes the interrupted transfer instead of manufacturing another resource.

Imported skills start private. Their stable locator is the authenticated namespace plus the validated skill name, for example `@alice/my-skill`. Another logged-in device reconciles the same private workspace through verified snapshot downloads.

Team-targeted `--to @team` import is not available yet; team ownership and publishing permission arrive with the team implementation rather than being guessed during personal import.
