#!/usr/bin/env node

const crypto = require("node:crypto");
const fs = require("node:fs");
const http = require("node:http");
const https = require("node:https");
const path = require("node:path");

const pkg = require("../package.json");
const owner = pkg.config.githubOwner;
const repo = pkg.config.githubRepo;
const binaryName = pkg.config.binaryName;
const manifestFormat = "denju-release-manifest-v1";
const clientAssets = new Set([
  "denju_darwin_amd64",
  "denju_darwin_arm64",
  "denju_linux_amd64",
  "denju_linux_arm64",
  "denju_windows_amd64.exe",
  "denju_windows_arm64.exe"
]);
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
const releaseBase = process.env.DENJU_RELEASE_BASE_URL
  ? process.env.DENJU_RELEASE_BASE_URL.replace(/\/$/, "")
  : `https://github.com/${owner}/${repo}/releases/download/v${pkg.version}`;
const destinationName = platform === "windows" ? `${binaryName}.exe` : `${binaryName}-bin`;
const destination = path.join(__dirname, "..", "bin", destinationName);

async function main() {
  const [manifest, bytes] = await Promise.all([
    download(`${releaseBase}/release-manifest.txt`),
    download(`${releaseBase}/${assetName}`)
  ]);
  const release = parseManifest(manifest.toString("utf8"));
  if (release.version !== pkg.version) {
    throw new Error(`Release manifest version ${release.version} does not match package ${pkg.version}`);
  }
  const expected = release.assets.get(assetName);
  if (!expected) throw new Error(`No release manifest entry for ${assetName}`);
  if (bytes.length !== expected.size) {
    throw new Error(`Size mismatch for ${assetName}`);
  }
  const actual = crypto.createHash("sha256").update(bytes).digest("hex");
  if (actual !== expected.sha256) {
    throw new Error(`Checksum mismatch for ${assetName}`);
  }
  installVerifiedBinary(bytes);
}

function installVerifiedBinary(bytes) {
  const staged = `${destination}.stage.${process.pid}`;
  fs.writeFileSync(staged, bytes, { mode: platform === "windows" ? 0o644 : 0o755 });
  if (platform !== "windows") {
    fs.chmodSync(staged, 0o755);
    try {
      fs.renameSync(staged, destination);
    } catch (error) {
      try { fs.rmSync(staged, { force: true }); } catch {}
      throw error;
    }
    return;
  }

  cleanupRetiredWindowsBinaries();
  const retired = `${destination}.old.${process.pid}`;
  let retiredCurrent = false;
  try {
    if (fs.existsSync(destination)) {
      fs.renameSync(destination, retired);
      retiredCurrent = true;
    }
    fs.renameSync(staged, destination);
  } catch (error) {
    try { fs.rmSync(staged, { force: true }); } catch {}
    if (retiredCurrent && !fs.existsSync(destination)) {
      try { fs.renameSync(retired, destination); } catch {}
    }
    throw error;
  }
  try { fs.rmSync(retired, { force: true }); } catch {}
}

function cleanupRetiredWindowsBinaries() {
  if (platform !== "windows") return;
  const directory = path.dirname(destination);
  const prefix = `${path.basename(destination)}.old.`;
  for (const entry of fs.readdirSync(directory)) {
    if (!entry.startsWith(prefix)) continue;
    try { fs.rmSync(path.join(directory, entry), { force: true }); } catch {}
  }
}

function parseManifest(text) {
  let format = null;
  let version = null;
  let serverImage = null;
  const assets = new Map();
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    const fields = line.split(/\s+/);
    if (fields[0] === "format" && fields.length === 2) {
      if (format !== null) throw new Error("Duplicate release manifest format");
      format = fields[1];
    } else if (fields[0] === "version" && fields.length === 2) {
      if (version !== null) throw new Error("Duplicate release manifest version");
      version = fields[1];
    } else if (fields[0] === "asset" && fields.length === 4) {
      if (!clientAssets.has(fields[1])) {
        throw new Error(`Unsupported release manifest asset: ${fields[1]}`);
      }
      if (assets.has(fields[1])) {
        throw new Error(`Duplicate release manifest asset: ${fields[1]}`);
      }
      const size = Number(fields[3]);
      if (!/^[a-f0-9]{64}$/i.test(fields[2]) || !/^[0-9]+$/.test(fields[3]) || !Number.isSafeInteger(size) || size < 0) {
        throw new Error(`Invalid release manifest asset entry: ${line}`);
      }
      assets.set(fields[1], { sha256: fields[2].toLowerCase(), size });
    } else if (fields[0] === "server_image" && fields.length === 2) {
      if (serverImage !== null) throw new Error("Duplicate release manifest server image");
      serverImage = fields[1];
    } else {
      throw new Error(`Invalid release manifest line: ${line}`);
    }
  }
  if (format !== manifestFormat || !version || !/^[A-Za-z0-9.+-]{1,64}$/.test(version)) {
    throw new Error("Invalid Denju release manifest");
  }
  if (assets.size !== clientAssets.size || [...clientAssets].some((name) => !assets.has(name))) {
    throw new Error("Release manifest must contain exactly the six supported client assets");
  }
  const expectedServerImage = `ghcr.io/${owner}/denju-server:v${version}`;
  if (serverImage !== expectedServerImage) {
    throw new Error(`Release manifest server image must be ${expectedServerImage}`);
  }
  return { version, assets };
}

function download(url, redirects = 0) {
  if (redirects > 8) return Promise.reject(new Error("Too many redirects"));
  return new Promise((resolve, reject) => {
    const transport = new URL(url).protocol === "http:" ? http : https;
    transport.get(url, { headers: { "User-Agent": "denju-npm-installer" } }, (response) => {
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

if (require.main === module) {
  main().catch((error) => {
    console.error(`Unable to install Denju: ${error.message}`);
    process.exit(1);
  });
}

module.exports = { parseManifest };
