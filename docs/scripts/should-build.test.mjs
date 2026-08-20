import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const script = join(dirname(fileURLToPath(import.meta.url)), "should-build.mjs");

const git = (cwd, ...args) =>
  execFileSync("git", args, { cwd, encoding: "utf8" }).trim();

const write = (cwd, path, content) => {
  const target = join(cwd, path);
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, content);
};

const commit = (cwd, message) => {
  git(cwd, "add", "-A");
  git(cwd, "commit", "-m", message);
  return git(cwd, "rev-parse", "HEAD");
};

const createRepo = () => {
  const cwd = mkdtempSync(join(tmpdir(), "denju-vercel-ignore-"));
  git(cwd, "init", "-q");
  git(cwd, "config", "user.name", "Denju Test");
  git(cwd, "config", "user.email", "denju-test@example.invalid");
  write(cwd, "docs/index.md", "initial docs\n");
  write(cwd, "README.md", "initial readme\n");
  const initial = commit(cwd, "initial");
  return { cwd, initial };
};

const runGuard = (cwd, base, head, extraEnv = {}) =>
  spawnSync(process.execPath, [script], {
    cwd,
    env: {
      ...process.env,
      VERCEL_GIT_PREVIOUS_SHA: base,
      VERCEL_GIT_COMMIT_SHA: head,
      VERCEL_GIT_PULL_REQUEST_BASE_SHA: "",
      SITE_DEPLOY_DIFF_BASE: "",
      ...extraEnv,
    },
    stdio: "pipe",
  }).status;

test("builds when docs change", () => {
  const { cwd, initial } = createRepo();
  try {
    write(cwd, "docs/index.md", "changed docs\n");
    const head = commit(cwd, "docs change");
    assert.equal(runGuard(cwd, initial, head), 1);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("skips when only non-docs paths change", () => {
  const { cwd, initial } = createRepo();
  try {
    write(cwd, "README.md", "changed readme\n");
    const head = commit(cwd, "readme change");
    assert.equal(runGuard(cwd, initial, head), 0);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("builds for docs rename or deletion", () => {
  const { cwd, initial } = createRepo();
  try {
    git(cwd, "mv", "docs/index.md", "docs/renamed.md");
    const renamed = commit(cwd, "rename docs");
    assert.equal(runGuard(cwd, initial, renamed), 1);

    git(cwd, "rm", "docs/renamed.md");
    const deleted = commit(cwd, "delete docs");
    assert.equal(runGuard(cwd, renamed, deleted), 1);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("builds when docs changed earlier in a multi-commit range", () => {
  const { cwd, initial } = createRepo();
  try {
    write(cwd, "docs/index.md", "changed docs\n");
    commit(cwd, "docs change");
    write(cwd, "README.md", "later unrelated change\n");
    const head = commit(cwd, "readme change");
    assert.equal(runGuard(cwd, initial, head), 1);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("fails open when deployment SHAs are missing or invalid", () => {
  const { cwd, initial } = createRepo();
  try {
    assert.equal(runGuard(cwd, "", initial), 1);
    assert.equal(runGuard(cwd, "not-a-commit", initial), 1);
    assert.equal(runGuard(cwd, initial, "not-a-commit"), 1);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});
