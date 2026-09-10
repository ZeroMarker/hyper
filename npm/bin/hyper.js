#!/usr/bin/env node
"use strict";

/*
 * npm entry point for Hyper.
 *
 * Hyper itself is a Rust program. This shim only forwards to the prebuilt
 * binary shipped in the platform package that matches the current
 * `process.platform` / `process.arch`, so `npm install -g hyper-agent` works
 * without a Rust toolchain. It deliberately does no work of its own: stdin,
 * stdout and stderr are inherited so the full-screen TUI keeps a real TTY, and
 * the exit code (or the terminating signal) is passed through unchanged.
 */

const { spawn } = require("node:child_process");
const path = require("node:path");

const PREFIX = "hyper-agent";
const platformKey = `${process.platform}-${process.arch}`;
const packageName = `${PREFIX}-${platformKey}`;
const binaryFile = process.platform === "win32" ? "hyper.exe" : "hyper";

function declaredPlatforms() {
  try {
    const manifest = require("../package.json");
    return Object.keys(manifest.optionalDependencies || {})
      .map((name) => name.slice(PREFIX.length + 1))
      .sort();
  } catch {
    return [];
  }
}

function resolveBinary() {
  // Escape hatch for people who built Hyper themselves (or for local testing).
  if (process.env.HYPER_BINARY_PATH) {
    return process.env.HYPER_BINARY_PATH;
  }
  try {
    const manifest = require.resolve(`${packageName}/package.json`);
    return path.join(path.dirname(manifest), "bin", binaryFile);
  } catch {
    return null;
  }
}

const binary = resolveBinary();
if (!binary) {
  const supported = declaredPlatforms();
  process.stderr.write(
    `hyper: no prebuilt binary for ${platformKey}.\n` +
      `The optional dependency "${packageName}" is not installed.\n\n` +
      `npm skips optional dependencies when they are omitted (--omit=optional),\n` +
      `when scripts are disabled, or when the install ran offline. Reinstall with:\n\n` +
      `  npm install --include=optional ${PREFIX}\n\n` +
      `Prebuilt binaries: ${supported.length > 0 ? supported.join(", ") : "(none declared)"}\n` +
      `To run a binary you built yourself, set HYPER_BINARY_PATH.\n`,
  );
  process.exit(1);
}

const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });

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
  process.stderr.write(`hyper: failed to run ${binary}: ${error.message}\n`);
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
