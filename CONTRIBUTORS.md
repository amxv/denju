# CONTRIBUTORS.md

Maintainer notes for this template repository.

## Prerequisites

- Go `1.26+`
- Node `18+`
- Bun `1+`
- optional `license-generator` for non-Apache license changes
- npm account with publish rights for the package name in `package.json`
- GitHub repo admin access

## Local development

```bash
make check
make build
./dist/mycli --help
```

Install command locally:

```bash
make install-local
mycli --help
```

For a new CLI, use the bootstrap flow so the command, module, repository,
npm package, release metadata, docs config, and license stay synchronized:

```bash
make bootstrap BOOTSTRAP_ARGS='--cli-name pluck --go-module github.com/acme/pluck \
  --github-owner acme --github-repo pluck --npm-package @acme/pluck'
```

## Release process

1. Ensure `main` is green and the docs pass serially:

```bash
make check
bun run docs:check
bun run docs:build
```

2. Edit `src/content/docs/changelog.md` manually, then commit and push it:

```bash
git add src/content/docs/changelog.md
git commit -m "docs: update changelog for v0.1.0"
git push origin HEAD
```

3. Push the version tag. The low-level Make target is optional:

```bash
make release-tag VERSION=0.1.0
# or: git tag v0.1.0 && git push origin v0.1.0
```

4. GitHub Actions `release` workflow runs automatically:
- quality checks
- cross-platform binary build
- GitHub release publish
- npm publish

## Required GitHub secret

- `NPM_TOKEN`: npm automation token with publish rights for your package.

Configure it once in the repository settings or with `gh secret set NPM_TOKEN`.
The template does not read local keychains or synchronize release secrets.

## npm token setup

Create token at npm:

- Profile -> Access Tokens -> Create New Token
- Use an automation/granular token scoped to required package/org

Validate npm auth locally with your normal npm tooling if needed. Do not print
the token.

## Notes on package naming

Before first publish, set a package name you control in `package.json`.

- Example unscoped: `"name": "your-cli-name"`
- Example scoped: `"name": "@your-scope/your-cli-name"`

The default license is Apache-2.0 and does not require `license-generator`. Use
`make bootstrap BOOTSTRAP_ARGS='--license MIT'` (or another name from
`license-generator --list`) to regenerate `LICENSE` and synchronize the package
and docs metadata when the optional CLI is installed. Without it, bootstrap
continues with Apache-2.0 and prints a warning.

For a private GitHub repository, release assets require authenticated downloads.
The normal anonymous npm path is a public repository; use the explicit
`--anonymous-npm` warning before choosing a private repo for that flow.
