#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const pkg = require("../package.json");
const binaryName = pkg.config?.binaryName || "denju";
const executableName = process.platform === "win32" ? `${binaryName}.exe` : `${binaryName}-bin`;
const executablePath = path.join(__dirname, executableName);

function detectInstallContext(moduleDir = __dirname, packageName = pkg.name) {
  const packageRoot = path.resolve(moduleDir, "..");
  const vitePlusPackageRoot = path.resolve(moduleDir, "../../../..");
  const expectedVitePlusPackageRoot = path.join(
    vitePlusPackageRoot,
    "lib",
    "node_modules",
    packageName
  );

  if (
    path.basename(vitePlusPackageRoot) === packageName &&
    path.basename(path.dirname(vitePlusPackageRoot)) === "packages" &&
    packageRoot === expectedVitePlusPackageRoot
  ) {
    const vitePlusHome = path.resolve(vitePlusPackageRoot, "../..");
    const candidates = process.platform === "win32"
      ? ["vp.exe", "vp.cmd", "vp"]
      : ["vp"];
    const command = candidates
      .map((name) => path.join(vitePlusHome, "bin", name))
      .find((candidate) => fs.existsSync(candidate));
    if (command) return { manager: "vite-plus", command };
  }

  return { manager: "npm", command: null };
}

function main() {
  if (!fs.existsSync(executablePath)) {
    console.error(
      `denju binary is missing. Reinstall with script approval: npm install -g --allow-scripts=${pkg.name} ${pkg.name}@${pkg.version}`
    );
    process.exit(1);
  }

  const install = detectInstallContext();
  const env = {
    ...process.env,
    DENJU_INSTALL_SOURCE: "npm",
    DENJU_INSTALL_PACKAGE: pkg.name,
    DENJU_INSTALL_VERSION: pkg.version,
    DENJU_INSTALL_TARGET: executablePath,
    DENJU_INSTALL_MANAGER: install.manager
  };
  if (install.command) env.DENJU_INSTALL_COMMAND = install.command;
  else delete env.DENJU_INSTALL_COMMAND;

  const child = spawnSync(executablePath, process.argv.slice(2), {
    stdio: "inherit",
    env
  });
  if (child.error) {
    console.error(child.error.message);
    process.exit(1);
  }
  if (child.signal) {
    process.kill(process.pid, child.signal);
  }
  process.exit(child.status ?? 1);
}

if (require.main === module) {
  main();
}

module.exports = { detectInstallContext };
