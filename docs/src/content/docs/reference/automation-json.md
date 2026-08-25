---
title: Automation and JSON output
description: Use Denju predictably from coding agents, scripts, and CI with one versioned stdout envelope and stable machine error codes.
order: 41
category: Reference
summary: "Add --json for machine compatibility; Denju keeps diagnostics and interactive requirements out of structured stdout."
---

Denju's normal text output is intentionally concise and useful to both people and coding agents, but it is not a long-term machine compatibility contract.

For scripts and strict automation, use:

```bash
denju --json <command> ...
```

## Success envelope

JSON mode emits exactly one result object on stdout:

```json
{
  "version": 1,
  "ok": true,
  "result": {
    "...": "command-specific fields"
  }
}
```

The `version` field versions the CLI envelope independently of individual command payload evolution.

## Failure envelope

Failures use the same envelope shape:

```json
{
  "version": 1,
  "ok": false,
  "error": {
    "code": "not_found",
    "message": "...",
    "recovery": "denju search ..."
  }
}
```

Stable machine error codes include:

```text
invalid_arguments
setup_required
registry_locked
registry_unavailable
local_state
credential_unavailable
service_unavailable
not_found
quota_exceeded
content_verification
interactive_required
confirmation_required
internal
```

A command can include a concrete `recovery` action when there is a useful next command.

## No interactive prompts in JSON mode

JSON mode never opens a password, recovery-secret, or confirmation prompt.

If a human-only secret is required, Denju returns `interactive_required`.

If a destructive command requires confirmation, Denju returns `confirmation_required` unless the command supports and receives `--yes`.

That means an agent can distinguish “the operation failed” from “the caller has not authorized the destructive action yet” without scraping terminal prose.

## stdout and stderr

Structured stdout stays reserved for the one JSON envelope. Operational diagnostics and useful progress can go to stderr without corrupting the machine result.

Denju does not use a pager or animated spinner. When output is piped, terminal-only decoration such as color is disabled.

## Agent-friendly command design

Denju commands generally prefer:

- exact scoped resource locators;
- stable IDs and cursor fields in JSON payloads;
- exact executable recovery commands;
- bounded result pages instead of unbounded lists;
- explicit `--yes` when an already-authorized automation needs to bypass a normal `y/N` confirmation;
- scoped automation tokens rather than human passwords.

Agents should still run `denju <command> --help` against the installed version rather than assuming undocumented options.
