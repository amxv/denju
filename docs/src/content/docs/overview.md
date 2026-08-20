---
title: Overview
description: Understand Denju's current product boundary, local model, and registry architecture before changing implementation details.
order: 1
category: Start
summary: "The product model: one native CLI, durable local state, and one registry boundary."
---

Denju is an agent-native, CLI-only registry and synchronization system for Agent Skills.

## What exists today

The Rust implementation provides deterministic Agent Skills validation and content identity, anonymous `denju setup`, public skill discovery, account-wide direct subscriptions, verified cold installs from S3-compatible storage, optional password/recovery identity claim and login, revocable devices and scoped automation credentials, durable SQLite-backed local state, Codex and Claude projections, per-user service management, and the PostgreSQL-backed registry foundation used by the CLI.

The repository also contains a thin npm installer for the native binary and this documentation site. Product behavior lives in Rust; JavaScript is not a second implementation path.

## Local model

Each Denju installation owns one local state database and one canonical managed skills tree. Harness-facing paths are projections of that managed state rather than independent copies. The CLI remains correct when the background service is stopped because durable local state, not local IPC, is authoritative.

## Registry model

The registry is a separate Rust server backed by PostgreSQL and required S3-compatible object storage. The official service uses Neon PostgreSQL and Cloudflare R2, while local development uses the same interfaces with PostgreSQL and Garage.

Mutable references and registry metadata live in PostgreSQL. Skill bytes are content-addressed objects. This keeps synchronization and recovery based on durable state rather than process memory or filesystem events.

Public reads and direct subscriptions are cold-start safe: restarting the registry process does not change discovery or installation results because PostgreSQL, object storage, and durable installation/account state are authoritative. Anonymous subscriptions are adopted by the account when that installation claims or logs into an identity; another authenticated device then reconciles the same desired state.
