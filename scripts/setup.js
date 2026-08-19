#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const {
  githubRemote,
  githubUrl,
  commandExists,
  optionValue,
  parseArgs,
  readProjectConfig,
  rootDir,
  templateConfig,
  writeProjectConfig
} = require("./project");

const licenseDefinitions = {
  AGPL: { generator: "AGPL", spdx: "AGPL-3.0-only", label: "AGPL-3.0" },
  "Apache-2.0": { generator: "Apache", spdx: "Apache-2.0", label: "Apache 2.0" },
  BSD: { generator: "BSD", spdx: "BSD-3-Clause", label: "BSD-3-Clause" },
  CC0: { generator: "CC0", spdx: "CC0-1.0", label: "CC0-1.0" },
  "CC-BY": { generator: "CC-BY", spdx: "CC-BY-4.0", label: "CC-BY-4.0" },
  "CC-BY-NC": { generator: "CC-BY-NC", spdx: "CC-BY-NC-4.0", label: "CC-BY-NC-4.0" },
  "CC-BY-NC-SA": {
    generator: "CC-BY-NC-SA",
    spdx: "CC-BY-NC-SA-4.0",
    label: "CC-BY-NC-SA-4.0"
  },
  "CC-BY-SA": { generator: "CC-BY-SA", spdx: "CC-BY-SA-4.0", label: "CC-BY-SA-4.0" },
  "EUPL-1.2": { generator: "EUPL-1.2", spdx: "EUPL-1.2", label: "EUPL-1.2" },
  "GPL-2": { generator: "GPL-2", spdx: "GPL-2.0-only", label: "GPL-2.0" },
  "GPL-3": { generator: "GPL-3", spdx: "GPL-3.0-only", label: "GPL-3.0" },
  ISC: { generator: "ISC", spdx: "ISC", label: "ISC" },
  "LGPL-3": { generator: "LGPL-3", spdx: "LGPL-3.0-only", label: "LGPL-3.0" },
  MIT: { generator: "MIT", spdx: "MIT", label: "MIT" },
  "MPL-2": { generator: "MPL-2", spdx: "MPL-2.0", label: "MPL-2.0" },
  Unlicense: { generator: "Unlicense", spdx: "Unlicense", label: "Unlicense" }
};

const licenseAliases = {
  apache: "Apache-2.0",
  "apache2": "Apache-2.0",
  "apache2.0": "Apache-2.0",
  "apache-2.0": "Apache-2.0",
  "apachelicense2.0": "Apache-2.0",
  gpl2: "GPL-2",
  gpl3: "GPL-3",
  lgpl3: "LGPL-3",
  mpl2: "MPL-2",
  "bsd-3-clause": "BSD",
  "mitlicense": "MIT"
};

function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    printHelp();
    return;
  }

  const current = readProjectConfig();
  let config = resolveConfig(current, options);
  validateConfig(config);
  warnForPrivateAnonymousInstall(options);
  const explicitLicense = Boolean(optionValue(options, "license", "LICENSE"));
  config = regenerateLicense(config, explicitLicense);

  renameStarterPaths(current, config);
  replaceIdentityReferences(current, config);
  updateBuildMetadata(config);
  updatePackageMetadata(config);
  updateReadmeLicense(config);
  writeProjectConfig(config);

  console.log(`Initialized ${config.cliName} for ${githubUrl(config)}.`);
  console.log(`Go module: ${config.goModule}`);
  console.log(`npm package: ${config.npmPackage}`);
  console.log(`License: ${config.license}`);
}

function resolveConfig(current, options) {
  const detected = readGithubRemoteIdentity();
  const owner =
    optionValue(options, "githubOwner", "GITHUB_OWNER") ||
    (detected && current.githubOwner === templateConfig.githubOwner ? detected.owner : current.githubOwner);
  const repo =
    optionValue(options, "githubRepo", "GITHUB_REPO") ||
    (detected && current.githubRepo === templateConfig.githubRepo ? detected.repo : current.githubRepo);
  const repositoryChanged = owner !== current.githubOwner || repo !== current.githubRepo;
  const cliName =
    optionValue(options, "cliName", "CLI_NAME") ||
    (repositoryChanged && current.cliName === templateConfig.cliName ? repo : current.cliName);
  const binaryName =
    optionValue(options, "binaryName", "BINARY_NAME") ||
    (current.binaryName === current.cliName ? cliName : current.binaryName);
  const goModule =
    optionValue(options, "goModule", "GO_MODULE") ||
    (repositoryChanged && current.goModule === templateConfig.goModule
      ? `github.com/${owner}/${repo}`
      : current.goModule);
  const npmPackage =
    optionValue(options, "npmPackage", "NPM_PACKAGE") ||
    (repositoryChanged && current.npmPackage === templateConfig.npmPackage
      ? `@${owner.toLowerCase()}/${repo.toLowerCase()}`
      : current.npmPackage);
  const homepage =
    optionValue(options, "homepage", "HOMEPAGE") ||
    (repositoryChanged && current.homepage === templateConfig.homepage
      ? `${githubUrl({ githubOwner: owner, githubRepo: repo })}#readme`
      : current.homepage);
  const canonicalUrl =
    optionValue(options, "canonicalUrl", "CANONICAL_URL") ||
    (repositoryChanged && current.canonicalUrl === templateConfig.canonicalUrl
      ? githubUrl({ githubOwner: owner, githubRepo: repo })
      : current.canonicalUrl);
  const license = normalizeLicense(
    optionValue(options, "license", "LICENSE") || current.license || "Apache-2.0"
  );

  return {
    cliName,
    binaryName,
    goModule,
    githubOwner: owner,
    githubRepo: repo,
    npmPackage,
    description:
      optionValue(options, "description", "DESCRIPTION") || current.description,
    homepage,
    canonicalUrl,
    license,
    paths: {
      command: `cmd/${cliName}`,
      wrapper: `bin/${cliName}.js`,
      binary: `bin/${binaryName}-bin`
    }
  };
}

function validateConfig(config) {
  const namePattern = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
  if (!namePattern.test(config.cliName)) {
    throw new Error("CLI_NAME must contain only letters, numbers, dots, underscores, or hyphens.");
  }
  if (!namePattern.test(config.binaryName)) {
    throw new Error("BINARY_NAME must contain only letters, numbers, dots, underscores, or hyphens.");
  }
  if (!/^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?$/.test(config.githubOwner)) {
    throw new Error("GITHUB_OWNER must be a valid GitHub owner name.");
  }
  if (!/^[A-Za-z0-9._-]+$/.test(config.githubRepo)) {
    throw new Error("GITHUB_REPO must be a valid GitHub repository name.");
  }
  if (!config.goModule || /\s/.test(config.goModule)) {
    throw new Error("GO_MODULE must be a non-empty Go module path without spaces.");
  }
  if (!/^(@[a-z0-9][a-z0-9._~-]*\/)?[a-z0-9][a-z0-9._~-]*$/.test(config.npmPackage)) {
    throw new Error("NPM_PACKAGE must be a valid lowercase npm package name.");
  }
  for (const [name, value] of [
    ["HOMEPAGE", config.homepage],
    ["CANONICAL_URL", config.canonicalUrl]
  ]) {
    try {
      new URL(value);
    } catch {
      throw new Error(`${name} must be an absolute URL.`);
    }
  }
  if (!licenseDefinitions[config.license]) {
    throw new Error(`Unsupported license ${config.license}. Choose: ${Object.keys(licenseDefinitions).join(", ")}`);
  }
}

function warnForPrivateAnonymousInstall(options) {
  const visibility = (optionValue(options, "visibility", "VISIBILITY") || "").toLowerCase();
  const anonymousNpm = optionValue(options, "anonymousNpm", "ANONYMOUS_NPM");
  if (visibility === "private" && [true, "1", "true", "yes"].includes(anonymousNpm)) {
    console.warn(
      "Warning: anonymous npm installs cannot download release assets from a private GitHub repository. Use a public repo for the normal npm path, or arrange authenticated/bundled assets in bootstrap-go-cli."
    );
  }
}

function normalizeLicense(value) {
  const normalized = String(value)
    .trim()
    .toLowerCase()
    .replace(/[ _]/g, "");
  const alias = licenseAliases[normalized];
  if (alias) {
    return alias;
  }
  const match = Object.entries(licenseDefinitions).find(([key, definition]) => {
    return [key, definition.generator, definition.spdx]
      .map((candidate) => candidate.toLowerCase().replace(/[ _]/g, ""))
      .includes(normalized);
  });

  if (!match) {
    return value;
  }
  return match[0];
}

function renameStarterPaths(current, config) {
  renamePath(path.join(rootDir, "cmd", current.cliName), path.join(rootDir, "cmd", config.cliName));
  renamePath(path.join(rootDir, "bin", `${current.cliName}.js`), path.join(rootDir, "bin", `${config.cliName}.js`));
}

function renamePath(source, destination) {
  if (source === destination || !fs.existsSync(source)) {
    return;
  }
  if (fs.existsSync(destination)) {
    throw new Error(`Cannot rename ${source} to ${destination}: destination already exists.`);
  }
  fs.renameSync(source, destination);
}

function replaceIdentityReferences(current, config) {
  const replacements = [
    [current.goModule, config.goModule],
    [current.npmPackage, config.npmPackage],
    [githubUrl(current), githubUrl(config)],
    [current.homepage, config.homepage],
    [current.canonicalUrl, config.canonicalUrl],
    ...(current.binaryName !== current.cliName
      ? [[current.binaryName, config.binaryName]]
      : []),
    [current.cliName, config.cliName]
  ]
    .filter(([from, to]) => from && to && from !== to)
    .sort(([left], [right]) => right.length - left.length);

  for (const filePath of collectTextFiles(rootDir)) {
    if (path.basename(filePath) === "LICENSE") {
      continue;
    }

    const original = fs.readFileSync(filePath, "utf8");
    let updated = original;
    for (const [from, to] of replacements) {
      updated = updated.split(from).join(to);
    }
    if (updated !== original) {
      fs.writeFileSync(filePath, updated);
    }
  }
}

function updateBuildMetadata(config) {
  const makefilePath = path.join(rootDir, "Makefile");
  let makefile = fs.readFileSync(makefilePath, "utf8");
  makefile = makefile
    .replace(/^BIN_NAME \?=.*$/m, `BIN_NAME ?= ${config.binaryName}`)
    .replace(/^CMD_NAME \?=.*$/m, `CMD_NAME ?= ${config.cliName}`)
    .replace(/^CMD_PATH \?=.*$/m, "CMD_PATH ?= ./cmd/$(CMD_NAME)");
  fs.writeFileSync(makefilePath, makefile);

  const workflowPath = path.join(rootDir, ".github/workflows/release.yml");
  let workflow = fs.readFileSync(workflowPath, "utf8");
  workflow = workflow
    .replace(/^  CLI_BINARY:.*$/m, `  CLI_BINARY: "${config.binaryName}"`)
    .replace(/^  CLI_ENTRYPOINT:.*$/m, `  CLI_ENTRYPOINT: "${config.cliName}"`)
    .replace(/^  CLI_MODULE:.*$/m, `  CLI_MODULE: "${config.goModule}"`);
  fs.writeFileSync(workflowPath, workflow);
}

function collectTextFiles(directory) {
  const files = [];
  const ignoredDirectories = new Set([".git", ".astro", "dist", "node_modules", "scripts"]);
  const textExtensions = new Set([".astro", ".go", ".js", ".json", ".lock", ".md", ".mjs", ".ts", ".yml"]);

  function visit(currentDirectory) {
    for (const entry of fs.readdirSync(currentDirectory, { withFileTypes: true })) {
      if (entry.isDirectory() && ignoredDirectories.has(entry.name)) {
        continue;
      }
      const entryPath = path.join(currentDirectory, entry.name);
      if (entry.isDirectory()) {
        visit(entryPath);
        continue;
      }
      if (entry.name.endsWith("-bin") || entry.name.endsWith(".exe")) {
        continue;
      }
      if (entry.name === "Makefile" || textExtensions.has(path.extname(entry.name))) {
        if (fs.statSync(entryPath).size <= 2 * 1024 * 1024) {
          files.push(entryPath);
        }
      }
    }
  }

  visit(directory);
  return files;
}

function updatePackageMetadata(config) {
  const packagePath = path.join(rootDir, "package.json");
  const packageJSON = JSON.parse(fs.readFileSync(packagePath, "utf8"));
  packageJSON.name = config.npmPackage;
  packageJSON.description = config.description;
  packageJSON.license = licenseDefinitions[config.license].spdx;
  packageJSON.repository = { type: "git", url: githubRemote(config) };
  packageJSON.homepage = config.homepage;
  packageJSON.bugs = { url: `${githubUrl(config)}/issues` };
  packageJSON.bin = { [config.cliName]: config.paths.wrapper };
  packageJSON.config = {
    ...(packageJSON.config || {}),
    cliName: config.cliName,
    cliBinaryName: config.binaryName,
    goModule: config.goModule
  };
  packageJSON.files = Array.from(
    new Set([...(packageJSON.files || []), "project.config.json"])
  );

  fs.writeFileSync(packagePath, `${JSON.stringify(packageJSON, null, 2)}\n`);
}

function updateReadmeLicense(config) {
  const readmePath = path.join(rootDir, "README.md");
  const readme = fs.readFileSync(readmePath, "utf8");
  const updated = readme.replace(
    /(## License\s*\n\s*)[^\n]*/,
    `$1${licenseDefinitions[config.license].label}`
  );
  if (updated !== readme) {
    fs.writeFileSync(readmePath, updated);
  }
}

function regenerateLicense(config, explicitLicense) {
  if (config.license === "Apache-2.0" && !explicitLicense) {
    updateApacheLicenseNotice(config);
    return config;
  }

  if (!commandExists("license-generator")) {
    if (config.license !== "Apache-2.0") {
      console.warn(
        `Warning: license-generator is unavailable; continuing with Apache-2.0 instead of ${config.license}. Install license-generator to select another license.`
      );
      config = { ...config, license: "Apache-2.0" };
    }
    updateApacheLicenseNotice(config);
    return config;
  }

  const definition = licenseDefinitions[config.license];
  const author =
    process.env.LICENSE_AUTHOR ||
    readGitConfig("user.name") ||
    config.githubOwner;
  const project = process.env.LICENSE_PROJECT || config.githubRepo;
  const year = process.env.LICENSE_YEAR || String(new Date().getFullYear());
  const args = [
    definition.generator,
    "--output",
    "LICENSE",
    "--author",
    author,
    "--project",
    project,
    "--year",
    year
  ];
  const result = spawnSync("license-generator", args, {
    cwd: rootDir,
    stdio: "inherit",
    timeout: 10000
  });

  if (!result.error && result.status === 0) {
    return config;
  }

  console.warn(
    `Warning: license-generator could not generate ${config.license}; continuing with Apache-2.0. Check the CLI output if you need a different license.`
  );
  updateApacheLicenseNotice(config);
  return {
    ...config,
    license: "Apache-2.0"
  };
}

function updateApacheLicenseNotice(config) {
  const licensePath = path.join(rootDir, "LICENSE");
  if (!fs.existsSync(licensePath)) {
    return;
  }

  const license = fs.readFileSync(licensePath, "utf8");
  if (!/^\s*Apache License\s*$/m.test(license)) {
    return;
  }

  const author = (process.env.LICENSE_AUTHOR || readGitConfig("user.name") || config.githubOwner)
    .replace(/\s+/g, " ")
    .trim();
  const year = process.env.LICENSE_YEAR || String(new Date().getFullYear());
  const updated = license.replace(/Copyright \d{4} .*$/m, `Copyright ${year} ${author}`);
  if (updated !== license) {
    fs.writeFileSync(licensePath, updated);
  }
}

function readGitConfig(key) {
  const result = spawnSync("git", ["config", "--get", key], {
    cwd: rootDir,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"]
  });
  if (result.status !== 0) {
    return "";
  }
  return result.stdout.trim();
}

function readGithubRemoteIdentity() {
  const result = spawnSync("git", ["remote", "get-url", "origin"], {
    cwd: rootDir,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"]
  });
  if (result.status !== 0) {
    return undefined;
  }

  const match = result.stdout.trim().match(/github\.com[:/](.+?)\/(.+?)(?:\.git)?$/i);
  if (!match) {
    return undefined;
  }
  return { owner: match[1], repo: match[2] };
}

function printHelp() {
  console.log(`Usage: node scripts/setup.js [options]

Initialize the repeated project identity fields in one pass.

Options (also accepted as uppercase environment variables):
  --cli-name NAME          User-facing CLI command and cmd/ directory
  --binary-name NAME       Release binary and Makefile output name
  --go-module MODULE       Go module path
  --github-owner OWNER     GitHub owner or organization
  --github-repo REPO       GitHub repository name
  --npm-package PACKAGE    npm package name
  --description TEXT       Package and repository description
  --homepage URL            npm and GitHub homepage
  --canonical-url URL       Astro canonical site URL
  --license NAME            Apache-2.0 by default; optional license-generator names
  --visibility private|public
                            Optional warning context for the already-created repo
  --anonymous-npm           Warn when private release assets would need anonymous downloads

Example:
  make bootstrap BOOTSTRAP_ARGS='--cli-name pluck --go-module github.com/acme/pluck \\
    --github-owner acme --github-repo pluck --npm-package @acme/pluck \\
    --description "A fast file picker" --license Apache-2.0'
`);
}

try {
  main();
} catch (error) {
  console.error(`Bootstrap failed: ${error.message}`);
  process.exit(1);
}
