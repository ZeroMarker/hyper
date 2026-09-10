#!/usr/bin/env node
"use strict";

/*
 * npm entry point for Hyper.
 *
 * Hyper itself is a Rust program. This shim only forwards to the prebuilt
 * binary shipped in the platform package matching the current
 * `process.platform` / `process.arch`, so `npm install -g hyper-agent` works
 * without a Rust toolchain. It deliberately does no work of its own: stdin,
 * stdout and stderr are inherited so the full-screen TUI keeps a real TTY, and
 * the exit code (or the terminating signal) is passed through unchanged.
 *
 * Which package to use is discovered rather than hardcoded: the platform
 * packages are declared as optionalDependencies, npm installs only the one
 * matching the host, and each carries the `os`/`cpu` it was built for plus the
 * path of its binary. That keeps this file independent of the platform matrix
 * in npm/platforms.json — the Windows package, for example, has to be named
 * `hyper-agent-windows-x64` rather than `hyper-agent-win32-x64` because npm's
 * spam heuristics reject the latter.
 */

const { spawn } = require("node:child_process");
const path = require("node:path");

const PREFIX = "hyper-agent";

function declaredPackages() {
  try {
    const manifest = require("../package.json");
    return Object.keys(manifest.optionalDependencies || {}).sort();
  } catch {
    return [];
  }
}

/** The installed platform package built for this host, if npm installed one. */
function findPlatformPackage() {
  for (const name of declaredPackages()) {
    let directory;
    try {
      directory = path.dirname(require.resolve(`${name}/package.json`));
    } catch {
      // Not installed: either it is for another platform, or optional
      // dependencies were omitted from the install.
      continue;
    }
    const manifest = require(path.join(directory, "package.json"));
    const forOtherOs = (manifest.os || []).length > 0 && !manifest.os.includes(process.platform);
    const forOtherCpu =
      (manifest.cpu || []).length > 0 && !manifest.cpu.includes(process.arch);
    if (forOtherOs || forOtherCpu) {
      continue;
    }
    if (!manifest.hyper || !manifest.hyper.binary) {
      continue;
    }
    return { name, binary: path.join(directory, manifest.hyper.binary) };
  }
  return null;
}

const resolved = process.env.HYPER_BINARY_PATH
  ? { name: null, binary: process.env.HYPER_BINARY_PATH }
  : findPlatformPackage();

if (!resolved) {
  const key = `${process.platform}-${process.arch}`;
  const declared = declaredPackages();
  process.stderr.write(
    `hyper: no prebuilt binary for ${key}.\n` +
      `None of the platform packages declared by ${PREFIX} is installed.\n\n` +
      `npm skips optional dependencies when they are omitted (--omit=optional),\n` +
      `when scripts are disabled, or when the install ran offline. Reinstall with:\n\n` +
      `  npm install --include=optional ${PREFIX}\n\n` +
      `Prebuilt binaries: ${declared.length > 0 ? declared.join(", ") : "(none declared)"}\n` +
      `To run a binary you built yourself, set HYPER_BINARY_PATH.\n`,
  );
  process.exit(1);
}

const child = spawn(resolved.binary, process.argv.slice(2), { stdio: "inherit" });

// The binary inherits the terminal and therefore already receives Ctrl-C, but
// the shim must not exit before it does or the harness would be orphaned.
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => {
    if (!child.killed) {
      child.kill(signal);
    }
  });
}

child.on("error", (error) => {
  process.stderr.write(`hyper: failed to run ${resolved.binary}: ${error.message}\n`);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    // Report the same termination the binary saw, instead of a fake exit code.
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code === null ? 1 : code);
});
