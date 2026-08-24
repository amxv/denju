const assert = require("node:assert/strict");
const { test } = require("node:test");

const { parseManifest } = require("./postinstall.js");

const assets = [
  "denju_darwin_amd64",
  "denju_darwin_arm64",
  "denju_linux_amd64",
  "denju_linux_arm64",
  "denju_windows_amd64.exe",
  "denju_windows_arm64.exe"
];

function manifest(version = "1.2.3") {
  const lines = ["format denju-release-manifest-v1", `version ${version}`];
  for (const name of assets) lines.push(`asset ${name} ${"a".repeat(64)} 12`);
  lines.push(`server_image ghcr.io/amxv/denju-server:v${version}`);
  return `${lines.join("\n")}\n`;
}

test("shared release manifest accepts the exact v1 release shape", () => {
  const parsed = parseManifest(manifest());
  assert.equal(parsed.version, "1.2.3");
  assert.equal(parsed.assets.size, 6);
  assert.deepEqual(parsed.assets.get("denju_linux_arm64"), {
    sha256: "a".repeat(64),
    size: 12
  });
});

test("shared release manifest rejects ambiguous or incomplete input", () => {
  assert.throws(() => parseManifest(`${manifest()}version 1.2.3\n`), /Duplicate/);
  assert.throws(() => parseManifest(`${manifest()}future_field nope\n`), /Invalid release manifest line/);
  assert.throws(() => parseManifest(manifest().replace("denju_linux_arm64", "denju_plan9_arm64")), /Unsupported/);
  assert.throws(() => parseManifest(manifest().replace(/asset denju_linux_arm64 .*\n/, "")), /six supported/);
  assert.throws(
    () => parseManifest(manifest().replace("ghcr.io/amxv/denju-server:v1.2.3", "ghcr.io/example/denju-server:v1.2.3")),
    /server image/
  );
  assert.throws(() => parseManifest(manifest("../1.2.3")), /Invalid Denju release manifest/);
});
