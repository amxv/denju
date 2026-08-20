#!/usr/bin/env node

const crypto = require("node:crypto");
const fs = require("node:fs");
const https = require("node:https");
const path = require("node:path");

const pkg = require("../package.json");
const owner = pkg.config.githubOwner;
const repo = pkg.config.githubRepo;
const binaryName = pkg.config.binaryName;
const platform = {
  darwin: "darwin",
  linux: "linux",
  win32: "windows"
}[process.platform];
const arch = {
  x64: "amd64",
  arm64: "arm64"
}[process.arch];

if (!platform || !arch) {
  console.error(`Unsupported Denju platform: ${process.platform}/${process.arch}`);
  process.exit(1);
}

const extension = platform === "windows" ? ".exe" : "";
const assetName = `${binaryName}_${platform}_${arch}${extension}`;
const releaseBase = `https://github.com/${owner}/${repo}/releases/download/v${pkg.version}`;
const destinationName = platform === "windows" ? `${binaryName}.exe` : `${binaryName}-bin`;
const destination = path.join(__dirname, "..", "bin", destinationName);

async function main() {
  const [manifest, bytes] = await Promise.all([
    download(`${releaseBase}/checksums.txt`),
    download(`${releaseBase}/${assetName}`)
  ]);
  const expected = checksumFor(manifest.toString("utf8"), assetName);
  const actual = crypto.createHash("sha256").update(bytes).digest("hex");
  if (actual !== expected) {
    throw new Error(`Checksum mismatch for ${assetName}`);
  }
  fs.writeFileSync(destination, bytes, { mode: platform === "windows" ? 0o644 : 0o755 });
  if (platform !== "windows") fs.chmodSync(destination, 0o755);
}

function checksumFor(manifest, assetName) {
  for (const line of manifest.split(/\r?\n/)) {
    const match = line.trim().match(/^([a-f0-9]{64})\s+\*?(.+)$/i);
    if (match && match[2] === assetName) return match[1].toLowerCase();
  }
  throw new Error(`No checksum found for ${assetName}`);
}

function download(url, redirects = 0) {
  if (redirects > 8) return Promise.reject(new Error("Too many redirects"));
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { "User-Agent": "denju-npm-installer" } }, (response) => {
      if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        response.resume();
        download(new URL(response.headers.location, url).toString(), redirects + 1).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`HTTP ${response.statusCode} for ${url}`));
        return;
      }
      const chunks = [];
      response.on("data", (chunk) => chunks.push(chunk));
      response.on("end", () => resolve(Buffer.concat(chunks)));
      response.on("error", reject);
    }).on("error", reject);
  });
}

main().catch((error) => {
  try { fs.rmSync(destination, { force: true }); } catch {}
  console.error(`Unable to install Denju: ${error.message}`);
  process.exit(1);
});
