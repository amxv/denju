# denju

Package complete agent skill directories and share them in a new Agentbox thread.

The CLI preserves each selected skill's `SKILL.md`, scripts, references, assets,
symlinks, and executable permissions. It creates one `.tar.gz`, attaches it to a
new thread, and grants a chosen Agentbox team access.

## Install

```bash
npm install -g denju-cli
denju --help
```

The separate [`@amxv/agentbox`](https://github.com/amxv/agentbox) CLI must also
be installed and authenticated when sharing a bundle:

```bash
npm install -g @amxv/agentbox
agentbox login
agentbox doctor
```

## Share skills

```bash
denju \
  --team ama \
  --title "Reusable agent skills" \
  agentbox dogfood frontend-design
```

Skill names are resolved under `~/.agents/skills` by default. The command:

1. validates every selected skill before creating a thread
2. packages each complete directory without filtering nested files
3. creates a new Agentbox thread with an inventory
4. attaches the archive
5. shares the thread with the requested team

The thread remains private to its owner and shared teams unless it is published
separately through Agentbox.

## Package without sharing

```bash
denju \
  --package-only \
  --archive ./agent-skills.tar.gz \
  agentbox dogfood
```

If `--archive` is omitted in package-only mode, the archive is written to the
current directory with a timestamped name. Existing archive files are never
overwritten.

## Options

```text
--skills-dir <path>  skill directory root (default: ~/.agents/skills)
--team <slug>        Agentbox team to share with
--title <title>      thread title
--archive <path>     retain the bundle at this path
--package-only       create a bundle without using Agentbox
```

`--team` is required unless `--package-only` is used.

## Development

```bash
bun install --frozen-lockfile
make check
make build
./dist/denju --help
bun run docs:check
bun run docs:build
```

The repository includes a tag-driven GitHub release workflow, an npm wrapper,
and an Astro/ZueDocs documentation site.

## License

Apache-2.0
