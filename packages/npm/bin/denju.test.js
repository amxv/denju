const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { test } = require("node:test");

const { detectInstallContext } = require("./denju.js");

test("ordinary npm layout keeps npm as the package manager", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "denju-npm-layout-"));
  const moduleDir = path.join(root, "lib", "node_modules", "denju-cli", "bin");
  fs.mkdirSync(moduleDir, { recursive: true });

  assert.deepEqual(detectInstallContext(moduleDir, "denju-cli"), {
    manager: "npm",
    command: null
  });
});

test("Vite+ global package layout selects the Vite+ manager", () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "denju-vite-plus-layout-"));
  const moduleDir = path.join(
    home,
    "packages",
    "denju-cli",
    "lib",
    "node_modules",
    "denju-cli",
    "bin"
  );
  fs.mkdirSync(moduleDir, { recursive: true });
  const commandName = process.platform === "win32" ? "vp.exe" : "vp";
  const command = path.join(home, "bin", commandName);
  fs.mkdirSync(path.dirname(command), { recursive: true });
  fs.writeFileSync(command, "");

  assert.deepEqual(detectInstallContext(moduleDir, "denju-cli"), {
    manager: "vite-plus",
    command
  });
});
