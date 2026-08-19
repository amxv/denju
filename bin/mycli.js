#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const pkg = require("../package.json");
const cliName = pkg.config?.cliName || Object.keys(pkg.bin || {})[0] || "mycli";
const binaryName = pkg.config?.cliBinaryName || cliName;
const executableName = process.platform === "win32" ? `${binaryName}.exe` : `${binaryName}-bin`;
const executablePath = path.join(__dirname, executableName);

if (!fs.existsSync(executablePath)) {
  console.error(`${cliName} binary is not installed. Re-run: npm rebuild -g ${pkg.name}`);
  process.exit(1);
}

const child = spawnSync(executablePath, process.argv.slice(2), { stdio: "inherit" });

if (child.error) {
  console.error(child.error.message);
  process.exit(1);
}

if (child.signal) {
  process.kill(process.pid, child.signal);
}

process.exit(child.status ?? 1);
