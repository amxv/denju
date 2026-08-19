const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const rootDir = path.resolve(__dirname, "..");
const configPath = path.join(rootDir, "project.config.json");

const templateConfig = {
  cliName: "mycli",
  binaryName: "mycli",
  goModule: "github.com/amxv/go-cli-template",
  githubOwner: "amxv",
  githubRepo: "go-cli-template",
  npmPackage: "@amxv/go-cli-template",
  description: "Template for shipping a Go CLI via npm with GitHub release automation",
  homepage: "https://github.com/amxv/go-cli-template#readme",
  canonicalUrl: "https://github.com/amxv/go-cli-template",
  license: "Apache-2.0"
};

function readProjectConfig() {
  if (!fs.existsSync(configPath)) {
    return { ...templateConfig };
  }

  try {
    return { ...templateConfig, ...JSON.parse(fs.readFileSync(configPath, "utf8")) };
  } catch (error) {
    throw new Error(`Unable to read ${path.basename(configPath)}: ${error.message}`);
  }
}

function writeProjectConfig(config) {
  fs.writeFileSync(configPath, `${JSON.stringify(config, null, 2)}\n`);
}

function parseArgs(args) {
  const options = {};

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg.startsWith("--")) {
      if (!options._) options._ = [];
      options._.push(arg);
      continue;
    }

    const withoutPrefix = arg.slice(2);
    const separator = withoutPrefix.indexOf("=");
    if (separator !== -1) {
      const key = toCamelCase(withoutPrefix.slice(0, separator));
      options[key] = withoutPrefix.slice(separator + 1);
      continue;
    }

    const key = toCamelCase(withoutPrefix);
    const next = args[index + 1];
    if (next && !next.startsWith("--")) {
      options[key] = next;
      index += 1;
    } else {
      options[key] = true;
    }
  }

  return options;
}

function toCamelCase(value) {
  return value.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function optionValue(options, name, envName) {
  if (options[name] !== undefined && options[name] !== true) {
    return options[name];
  }

  if (envName && process.env[envName]) {
    return process.env[envName];
  }

  return undefined;
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd || rootDir,
    env: options.env || process.env,
    encoding: "utf8",
    input: options.input,
    stdio: options.stdio || "inherit"
  });

  if (result.error) {
    throw new Error(`${command} is unavailable: ${result.error.message}`);
  }

  if (result.status !== 0 && options.allowFailure !== true) {
    throw new Error(`${command} ${args.join(" ")} exited with status ${result.status ?? 1}`);
  }

  return result;
}

function commandExists(command) {
  const result = spawnSync("sh", ["-c", `command -v ${command}`], {
    cwd: rootDir,
    stdio: "ignore"
  });
  return !result.error && result.status === 0;
}

function requireCommand(command, purpose) {
  if (!commandExists(command)) {
    throw new Error(`Missing ${command}. Install it before ${purpose}.`);
  }
}

function githubUrl(config) {
  return `https://github.com/${config.githubOwner}/${config.githubRepo}`;
}

function githubRemote(config) {
  return `git+${githubUrl(config)}.git`;
}

module.exports = {
  configPath,
  commandExists,
  githubRemote,
  githubUrl,
  optionValue,
  parseArgs,
  readProjectConfig,
  requireCommand,
  rootDir,
  run,
  templateConfig,
  writeProjectConfig
};
