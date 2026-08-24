---
title: Install and upgrade
description: Install Denju without a source toolchain and keep the native client safely updated.
order: 2
category: Start
summary: npm, shell, and PowerShell installs share one verified release contract and one upgrade path.
---

## Install the native client

Denju ships six native client binaries: macOS, Linux, and Windows on both amd64 and arm64. The npm
package, POSIX installer, PowerShell installer, and `denju upgrade` all consume the same
`denju-release-manifest-v1` GitHub Release manifest. Every path verifies the exact asset byte size
and SHA-256 before making the binary visible. None of them falls back to a local Rust source build.

### npm

```bash
npm install -g --allow-scripts=denju-cli denju-cli
denju setup
```

The npm package is only a launcher/installer for the matching native release asset. Current npm
releases block dependency lifecycle scripts unless they are explicitly approved, so the install
command grants script permission only to `denju-cli`; do not enable lifecycle scripts globally.

### macOS and Linux

Download the release `denju.sh` installer and run it with `sh`. By default it installs to
`~/.local/bin/denju`; set `DENJU_INSTALL_DIR` to choose another binary directory.

### Windows

Run the release `denju.ps1` installer in PowerShell. It installs to `%LOCALAPPDATA%\Denju\bin` by
default and adds that directory to the user PATH when necessary.

The standalone installers record only the installation source under `~/.denju` so later upgrades
stay on the same distribution channel. Denju account/session secrets remain separate from this
metadata.

## Upgrade safely

```bash
denju upgrade
```

For a standalone installation, Denju downloads the latest shared release manifest, stages the
matching binary, verifies size and SHA-256, and checks that the staged executable reports the
manifest's version. For an npm installation, `denju upgrade` delegates the package replacement to
the same npm package whose postinstall verifies that shared manifest.

After replacement, Denju rewrites/restarts the supported per-user background service and executes
the **new** binary's hidden health probe. If the installation has already been set up, that probe
checks local SQLite integrity, managed harness roots/projection prerequisites, and registry
readiness. If health verification fails, the standalone path atomically restores the prior binary;
the npm path reinstalls the previous package version. The recovery command remains `denju doctor`.

## Release manifest contract

The text manifest deliberately stays simple enough for Rust, Node, POSIX shell, and PowerShell to
parse identically:

```text
format denju-release-manifest-v1
version 1.2.3
asset denju_darwin_arm64 <sha256> <bytes>
server_image ghcr.io/amxv/denju-server:v1.2.3
```

The real manifest lists all six client assets. A release tag also publishes the same ordinary
`denju-server` container for Linux amd64 and arm64.
