---
title: Bundle behavior
description: Understand validation, archive layout, file preservation, and remote operation order.
order: 2
category: Guides
summary: The guarantees that keep a skill handoff complete and predictable.
---

## Selection and validation

Every positional argument is a skill directory name under `--skills-dir`.
Names must be plain path components. Absolute paths, `..`, and nested paths are
rejected.

All requested skills are validated before packaging starts. Duplicate names are
removed and the final inventory is sorted.

## Archive layout

The gzip-compressed tar archive contains one top-level directory per skill:

```text
agentbox/
  SKILL.md
  references/
  scripts/
dogfood/
  SKILL.md
  assets/
  scripts/
```

The CLI does not filter directory contents. Regular files, nested directories,
symlinks, and Unix executable bits are preserved. A top-level skill symlink is
resolved so its complete target directory is packaged under the selected name.

## Output safety

An explicit `--archive` path is created only when it does not already exist. In
package-only mode without a path, the CLI writes a timestamped archive in the
current directory.

For a live share without `--archive`, the temporary bundle is removed after the
Agentbox commands finish.

## Agentbox operation order

The remote workflow always runs in this order:

1. create a private thread with the sorted skill inventory
2. attach the complete archive
3. grant the selected team access

This prevents a team from seeing a thread before its bundle is attached. The
CLI does not enable Agentbox public visibility.
