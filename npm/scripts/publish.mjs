#!/usr/bin/env node
/*
 * Stage and publish the npm distribution of Hyper.
 *
 * Hyper is a Rust program; the npm channel exists so that `npm install -g
 * hyper-harness` works without a Rust toolchain. The layout is the
 * esbuild/biome pattern:
 *
 *   hyper-harness              main package: a JS shim, no lifecycle scripts
 *   hyper-agent-<os>-<cpu>     one package per platform, holds the binary
 *                              (the prefix is historical: npm rejects a main
 *                              package called `hyper-agent`)
 *
 * The main package declares the platform packages as optionalDependencies, so
 * npm downloads and installs exactly the one matching the host. Publishing
 * order matters: platform packages first, main package last.
 *
 * Usage:
 *   node npm/scripts/publish.mjs --artifacts <dir> [--version 0.1.0]
 *                                [--out npm-dist] [--publish]
 *
 * `--artifacts` is a directory holding the GitHub Release archives, named
 * `hyper-<suffix>.tar.gz` / `hyper-<suffix>.zip` (see npm/platforms.json).
 *
 * Without `--publish` nothing is uploaded: the packages are staged in `--out`
 * and the names that would be published are listed, so the workflow can smoke
 * test the exact tree first.
 */

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const npmDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(npmDir, "..");

function usage(message) {
  if (message) {
    console.error(`error: ${message}\n`);
  }
  console.error(
    "usage: node npm/scripts/publish.mjs --artifacts <dir> [--version <x.y.z>] " +
      "[--out <dir>] [--publish]",
  );
  process.exit(2);
}

function parseArgs(argv) {
  const options = { artifacts: null, version: null, out: "npm-dist", publish: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--publish") {
      options.publish = true;
    } else if (arg === "--artifacts" || arg === "--version" || arg === "--out") {
      const value = argv[i + 1];
      if (value === undefined || value.startsWith("--")) {
        usage(`${arg} needs a value`);
      }
      options[arg.slice(2)] = value;
      i += 1;
    } else {
      usage(`unknown argument ${arg}`);
    }
  }
  if (!options.artifacts) {
    usage("--artifacts is required");
  }
  return options;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

/** The crate version, which the release tag must agree with. */
function cargoVersion() {
  const manifest = fs.readFileSync(path.join(repoRoot, "Cargo.toml"), "utf8");
  const match = manifest.match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) {
    throw new Error("could not read `version` from Cargo.toml");
  }
  return match[1];
}

function extract(archive, destination) {
  fs.mkdirSync(destination, { recursive: true });
  const command = archive.endsWith(".zip") ? "unzip" : "tar";
  const args = archive.endsWith(".zip")
    ? ["-q", "-o", archive, "-d", destination]
    : ["-xzf", archive, "-C", destination];
  execFileSync(command, args, { stdio: ["ignore", "inherit", "inherit"] });
}

function stagePlatform(platform, archive, outDir, version, shared) {
  const extracted = path.join(outDir, ".extract", platform.key);
  extract(archive, extracted);

  const binary = path.join(extracted, platform.binary);
  if (!fs.existsSync(binary)) {
    throw new Error(`${archive} does not contain ${platform.binary}`);
  }

  const dir = path.join(outDir, platform.name);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(path.join(dir, "bin"), { recursive: true });
  // The executable bit must survive into the npm tarball.
  fs.copyFileSync(binary, path.join(dir, "bin", platform.binary));
  fs.chmodSync(path.join(dir, "bin", platform.binary), 0o755);
  fs.copyFileSync(path.join(repoRoot, "LICENSE"), path.join(dir, "LICENSE"));
  fs.copyFileSync(path.join(npmDir, "README.md"), path.join(dir, "README.md"));

  writeJson(path.join(dir, "package.json"), {
    name: platform.name,
    version,
    description: `Prebuilt Hyper binary for ${platform.os}/${platform.cpu}`,
    license: "MIT",
    homepage: shared.homepage,
    repository: shared.repository,
    bugs: shared.bugs,
    keywords: shared.keywords,
    // `os`/`cpu` are what make npm skip the wrong platform automatically.
    os: [platform.os],
    cpu: [platform.cpu],
    files: ["bin/"],
    // Read by npm/bin/hyper.js, which must not hardcode a platform matrix:
    // the binary is not always named after its platform package.
    hyper: { binary: `bin/${platform.binary}` },
  });
  return dir;
}

function stageMain(outDir, version, platforms) {
  const manifest = readJson(path.join(npmDir, "package.json"));
  // `private` only guards the staging manifest in this repository.
  delete manifest.private;
  delete manifest["//"];
  manifest.version = version;
  manifest.optionalDependencies = Object.fromEntries(
    platforms.map((platform) => [platform.name, version]),
  );

  const dir = path.join(outDir, manifest.name);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(path.join(dir, "bin"), { recursive: true });
  fs.copyFileSync(path.join(npmDir, "bin", "hyper.js"), path.join(dir, "bin", "hyper.js"));
  fs.chmodSync(path.join(dir, "bin", "hyper.js"), 0o755);
  fs.copyFileSync(path.join(repoRoot, "LICENSE"), path.join(dir, "LICENSE"));
  fs.copyFileSync(path.join(npmDir, "README.md"), path.join(dir, "README.md"));
  writeJson(path.join(dir, "package.json"), manifest);
  return dir;
}

function alreadyPublished(name, version) {
  try {
    const found = execFileSync("npm", ["view", `${name}@${version}`, "version"], {
      stdio: ["ignore", "pipe", "ignore"],
    });
    return found.toString().trim() === version;
  } catch {
    return false;
  }
}

function publish(dir, { publish }) {
  const manifest = readJson(path.join(dir, "package.json"));
  if (!publish) {
    console.log(`  would publish ${manifest.name}@${manifest.version}`);
    return;
  }
  if (alreadyPublished(manifest.name, manifest.version)) {
    console.log(`= ${manifest.name}@${manifest.version} is already published, skipping`);
    return;
  }
  console.log(`\n$ npm publish ${path.relative(process.cwd(), dir) || "."} --access public`);
  try {
    execFileSync("npm", ["publish", dir, "--access", "public"], { stdio: "inherit" });
  } catch (error) {
    // EOTP is the one failure that is not obvious from npm's own output: the
    // token itself is fine, it is simply the wrong *type* for unattended CI.
    console.error(
      "\nIf npm asked for a one-time password (EOTP), the NPMJS_TOKEN secret is a\n" +
        "token type that requires 2FA. Regenerate it as a classic Automation token,\n" +
        "or as a granular access token with \"Bypass two-factor authentication\"\n" +
        "enabled and read/write access to all packages (a scoped token cannot cover\n" +
        "packages that do not exist yet). Nothing was published; re-run the failed\n" +
        "job once the secret is updated.",
    );
    throw error;
  }
}

/**
 * npm refuses a name that only differs from an existing package by punctuation
 * ("hyper-agent" vs the existing "hyperagent"), and it only says so once the
 * publish is attempted — after the platform packages have already gone out.
 * Check the normalised form up front so a bad name fails before anything is
 * uploaded. Returns null when the registry cannot be reached.
 */
async function findRejectedNames(names) {
  const rejected = [];
  for (const name of names) {
    const normalized = name.toLowerCase().replace(/[-_.]/g, "");
    if (normalized === name.toLowerCase()) {
      continue;
    }
    try {
      const response = await fetch(`https://registry.npmjs.org/${normalized}`, { method: "HEAD" });
      if (response.ok) {
        rejected.push(`${name} (npm sees it as the existing package "${normalized}")`);
      }
    } catch {
      return null;
    }
  }
  return rejected;
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const { platforms } = readJson(path.join(npmDir, "platforms.json"));

  const crate = cargoVersion();
  const version = options.version ?? crate;
  if (version !== crate) {
    throw new Error(
      `release version ${version} does not match Cargo.toml (${crate}); ` +
        "tag the crate version you want to publish",
    );
  }

  const committed = readJson(path.join(npmDir, "package.json"));
  const rejected = await findRejectedNames([
    committed.name,
    ...platforms.map((platform) => platform.name),
  ]);
  if (rejected === null) {
    console.warn("warning: could not reach the npm registry to pre-check package names");
  } else if (rejected.length > 0) {
    throw new Error(`npm will refuse these package names:\n  - ${rejected.join("\n  - ")}`);
  }

  const declared = Object.keys(committed.optionalDependencies ?? {}).sort().join(",");
  const expected = platforms.map((platform) => platform.name).sort().join(",");
  if (declared !== expected) {
    console.warn(
      `warning: npm/package.json optionalDependencies are out of sync with ` +
        `npm/platforms.json (${declared} != ${expected}); the staged manifest is ` +
        `generated, so this only affects a local install from npm/`,
    );
  }

  const outDir = path.resolve(options.out);
  // The staging directory is wiped before use, so refuse anything that would
  // delete the repository or the npm sources.
  if (outDir === repoRoot || repoRoot.startsWith(`${outDir}${path.sep}`)) {
    throw new Error(`refusing to use ${outDir} as the staging directory`);
  }
  fs.rmSync(outDir, { recursive: true, force: true });
  fs.mkdirSync(outDir, { recursive: true });

  const artifacts = path.resolve(options.artifacts);
  const shared = {
    homepage: committed.homepage,
    repository: committed.repository,
    bugs: committed.bugs,
    keywords: committed.keywords,
  };

  console.log(
    `Staging ${committed.name} ${version} for ${platforms.length} platforms ` +
      `(${options.publish ? "publishing" : "dry run"})`,
  );

  // Platform packages first: the main package's optionalDependencies must
  // already exist on the registry when it is published.
  const staged = platforms.map((platform) => {
    const archive = path.join(artifacts, `hyper-${platform.suffix}.${platform.archive}`);
    if (!fs.existsSync(archive)) {
      throw new Error(
        `missing release archive ${archive}; build the ${platform.target} target first`,
      );
    }
    const dir = stagePlatform(platform, archive, outDir, version, shared);
    console.log(`  staged ${platform.name} <- hyper-${platform.suffix}.${platform.archive}`);
    return dir;
  });

  const mainDir = stageMain(outDir, version, platforms);
  console.log(`  staged ${path.basename(mainDir)}`);

  for (const dir of [...staged, mainDir]) {
    publish(dir, options);
  }

  if (options.publish) {
    console.log(`\nPublished ${committed.name}@${version} and ${platforms.length} platform packages.`);
  } else {
    console.log("\nDry run complete. Re-run with --publish to upload.");
  }
}

try {
  await main();
} catch (error) {
  console.error(`\nerror: ${error.message}`);
  process.exit(1);
}
