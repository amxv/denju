---
title: Install and set up
description: Install the native Denju client, initialize this machine, and verify that Codex and Claude Code can see managed skills.
order: 2
category: Start
summary: "Install the native CLI, run setup once, then search and subscribe anonymously."
---

Denju ships as a native CLI for macOS, Linux, and Windows on both amd64 and arm64. The npm package is a small installer/launcher for that native binary; it is not a JavaScript implementation of Denju.

## Install with npm

```bash
npm install -g --allow-scripts=denju-cli denju-cli
```

The package downloads the native release for your platform and verifies it against Denju's release manifest before making it available.

You can also use the standalone `denju.sh` or `denju.ps1` attached to each GitHub Release. All installation paths verify the same release artifacts and none falls back to compiling Denju from source.

## Set up this machine

```bash
denju setup
```

Setup is anonymous. It:

- creates Denju's local state under `~/.denju`;
- connects to the official registry at `https://registry.denju.ashray.xyz`;
- configures managed projections for Codex and Claude Code;
- notices existing unmanaged skills without importing or modifying them;
- installs the per-user background service where the platform supports it;
- validates the result.

If Denju finds an existing skill you may want to manage, it prints an explicit `denju import <path>` suggestion. It does not silently take ownership of that directory.

## Use another registry

A Denju installation is bound to one registry. Choose a self-hosted registry during setup:

```bash
denju setup --registry https://denju.example.com
```

For a local development or same-host self-hosted registry, loopback HTTP is supported:

```bash
denju setup --registry http://127.0.0.1:7788
```

V1 does not switch or federate an existing installation between registries. Pick the registry you want this installation to belong to when you set it up.

## Check the installation

Run Denju with no command:

```bash
denju
```

Denju is state-aware and normally prints either a compact healthy state or one useful next action.

For explicit diagnostics:

```bash
denju status
denju doctor
```

`status` is about synchronization and blocked resources. `doctor` checks and repairs the installation itself: local state, background service, harness projections, credentials, and registry connectivity.

## Install your first skill

No account is required:

```bash
denju search "react performance"
denju show @owner/skill
denju subscribe @owner/skill
```

After subscription, the skill is managed under Denju's canonical tree and projected into the configured harness roots. You do not need to copy it into Codex or Claude Code yourself.

When the publisher creates a new release, the background service normally updates the subscribed skill automatically. You can force one complete reconciliation at any time:

```bash
denju sync
```

Next: [Quickstart](/docs/start/quickstart) walks through the normal anonymous-to-publisher journey end to end.
