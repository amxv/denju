#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const pkg = require("../package.json");
const binaryName = pkg.config?.binaryName || "denju";
const executableName = process.platform === "win32" ? `${binaryName}.exe` : `${binaryName}-bin`;
const executablePath = path.join(__dirname, executableName);

if (!fs.existsSync(executablePath)) {
  console.error(
    `denju binary is missing. Reinstall with script approval: npm install -g --allow-scripts=${pkg.name} ${pkg.name}@${pkg.version}`
  );
  process.exit(1);
}

const child = spawnSync(executablePath, process.argv.slice(2), {
  stdio: "inherit",
  env: {
    ...process.env,
    DENJU_INSTALL_SOURCE: "npm",
    DENJU_INSTALL_PACKAGE: pkg.name,
    DENJU_INSTALL_VERSION: pkg.version,
    DENJU_INSTALL_TARGET: executablePath
  }
});
if (child.error) {
  console.error(child.error.message);
  process.exit(1);
}
if (child.signal) {
  process.kill(process.pid, child.signal);
}
process.exit(child.status ?? 1);
