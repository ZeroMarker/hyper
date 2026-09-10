# hyper

[![CI](https://github.com/ZeroMarker/hyper/actions/workflows/ci.yml/badge.svg)](https://github.com/ZeroMarker/hyper/actions/workflows/ci.yml)

Rust-native, terminal-first agent harness for local coding workflows.

The CLI, task runner, policy engine, tools, workspace storage, SQLite index,
checkpoints, sessions, and full-screen TUI are implemented in Rust. The harness
itself needs no Node.js runtime — the npm package below is only a delivery
channel for the prebuilt binaries.

DeepSeek is the default model provider for natural-language tasks. The default
model is `deepseek-v4-flash` and the default endpoint is
`https://api.deepseek.com`.

## Install

No Rust toolchain required — the npm package carries prebuilt binaries and only
resolves the one that matches your platform:

```bash
npm install -g hyper-harness
```

Alternatively download the archive for your platform from
[GitHub Releases](https://github.com/ZeroMarker/hyper/releases), or build from
source below.

## Build

Rust 1.94 is selected by `rust-toolchain.toml`.

```bash
cargo build --release
cargo test
```

The release binaries are `target/release/hyper` and its short alias
`target/release/hy`.

## Quick start

```bash
export DEEPSEEK_API_KEY="sk-..."
cargo run -- init
cargo run -- run examples/hello.json
cargo run -- runs
cargo run -- tui
```

After installing or copying either binary, the shortest workflow is:

```bash
hy                       # open TUI
hy "implement login"     # build mode
hy -p "analyze the bug"  # plan mode
```

On the first `hy` launch, Hyper securely prompts for the DeepSeek API key and
stores it in the user configuration directory with owner-only permissions.
Run `hy config` to replace it. `DEEPSEEK_API_KEY` remains the highest-priority
override and is recommended for CI.

Optional overrides:

```bash
export DEEPSEEK_MODEL="deepseek-v4-pro"
export DEEPSEEK_BASE_URL="https://api.deepseek.com"
```

Natural-language `plan`, `build`, and TUI prompts use DeepSeek. Explicit
instruction prefixes continue to use local deterministic tools and do not
require an API key.

## Commands

```text
hyper init
hyper validate <task.json>
hyper run <task.json>
hyper plan <prompt>
hyper build <prompt>
hyper runs [-n <limit>]
hyper show <run-id>
hyper diff <run-id>
hyper artifacts <run-id>
hyper checkpoints <run-id>
hyper restore <run-id> <checkpoint-id>
hyper undo <run-id>
hyper tui
```

Every command can use `hy` instead, for example `hy tui`.
Common aliases remain available: `hy b`, `hy p`, `hy r`, `hy ls`, and `hy s`.
`hy diff` prints the file diffs recorded by `write`/`edit` tools, `hy artifacts`
lists the run's artifact files, `hy checkpoints` lists snapshots and
`hy restore <run> <checkpoint-id>` rewinds one file to a specific snapshot.

`hy run`, `hy plan`, `hy build` and a direct prompt exit with status `1` when the
run does not finish, so CI can rely on the exit status; the run summary is still
printed on stdout. Subcommand prompts may be passed unquoted as several words:
`hy plan fix the login bug` is the same as `hy -p "fix the login bug"`.

The TUI uses Ratatui and Crossterm. Press `Tab` to switch plan/build mode,
`Enter` to submit, arrow keys to select runs, and `Esc` to exit. Slash commands:
`/help`, `/runs`, `/mode plan|build`, `/new`, and `/quit`. `/runs` lists recent
runs inline.

In the TUI, `bash`, `write` and `edit` actions ask for **interactive
approval** before running: press `y` to allow, `n`/`Esc` to deny (the agent
loop waits for the answer). Command-line runs do not prompt.

## Task format

```json
{
  "name": "hello",
  "steps": [
    {
      "id": "greet",
      "mode": "build",
      "instruction": "bash:echo hello from harness"
    }
  ]
}
```

Supported instructions: `bash:`, `read:`, `search:`, `write:`, and `edit:`.
Plan mode is read-only.

`write:` takes the content from the lines after the path, so the format is
`path\ncontent`. A trailing newline is therefore meaningful: `"write:a.txt\n"`
writes an empty file, while `"write:a.txt"` (no content line at all) is rejected
rather than silently emptying an existing file. `edit:` expects
`path\nsearch\nreplace` and replaces the first occurrence of `search`.

A step whose instruction has no tool prefix runs the **tool-calling agent**:
the model inspects the workspace context, calls tools (`read`, `search`,
`bash`, `write`, `edit`) in a loop, and feeds each result back until it
produces a final answer (capped at 12 turns). In plan mode the model only sees
the read-only tools. The optional per-step `tools` field acts as an allowlist:
`"tools": ["read", "search"]` restricts that step to the listed tools.

## Workspace

Runs are stored beneath `.harness/` using the existing compatible layout:

```text
.harness/
  harness.db
  runs/<run-id>/
    events.jsonl
    task.json
    summary.json
    lock
    artifacts/
    checkpoints/
  sessions/
```

JSONL remains the audit log; SQLite provides the local run/event index.

`runs/<run-id>/lock` is an advisory lock held for the lifetime of the run. Every
Hyper startup checks the runs still marked `running`: if a run's lock is not
held, the process that owned it is gone, so the run is rewritten as
`interrupted` — with a `run.interrupted` event and a `summary.json` — instead of
lingering as `running` forever. Runs that are still executing keep their lock and
are never touched.

## Reliability and limits

- `bash` drains stdout and stderr while the command runs, so a command that
  prints more than the OS pipe capacity cannot deadlock. At most 256 KB per
  stream is captured (the event payload sets `truncated` and reports the real
  byte counts); the rest is discarded so a runaway command cannot exhaust memory
  or the workspace.
- A command that exceeds its timeout is killed as a whole process group and
  reported as `TimeoutError` with `"timedOut": true`, which stays distinguishable
  from an ordinary non-zero exit. On Linux the shell additionally dies with the
  harness (`PR_SET_PDEATHSIG`), so a crashed or `kill -9`ed harness does not
  leave commands running — although processes started *by* that shell can still
  outlive it.
- `hy artifacts` lists `runs/<run-id>/artifacts/`, but no tool writes there yet,
  so it is always empty. `sessions/` is written for future session resume and is
  not read back yet.
- The dangerous-command check is a short substring denylist, not a sandbox: it
  catches obvious accidents and nothing more. Command-line runs apply it without
  any approval prompt, and the workspace context sent to the model contains
  repository file contents, so a repository can influence the model's actions.
  Run Hyper on code you trust.

## Releasing

`.github/workflows/release.yml` runs on `v*` tags. It builds the five supported
targets, attaches the archives to a GitHub Release, and publishes the npm
channel.

The npm channel is `hyper-harness` plus one package per platform
(`hyper-agent-linux-x64`, `hyper-agent-darwin-arm64`, …), each holding the
`hyper` binary built by the same workflow, so the npm and GitHub Release
binaries are byte-identical. The main package lists the platform packages as
`optionalDependencies` and ships only a small Node shim
(`npm/bin/hyper.js`) that locates the right binary and hands over stdio and the
exit status; it runs no lifecycle scripts and downloads nothing at install time.
`hy` is linked to the same shim.

Publishing needs an `NPMJS_TOKEN` repository secret, passed to npm as
`NODE_AUTH_TOKEN`. Use a classic **Automation** token (Access Tokens → Generate
New Token → Classic → Automation), or a granular access token with *Bypass
two-factor authentication* enabled and read/write access to **all** packages — a
token scoped to `hyper-harness*` cannot be created before those packages exist. A
classic *Publish* token does not work unattended: npm rejects it with `EOTP`,
because using it to publish requires a one-time password.

The workflow stages and smoke tests the packages *before* uploading, and skips
versions that are already on the registry, so a partially failed release can be
re-run with `gh run rerun <run-id> --failed`. A manual run of the workflow does
the staging and smoke testing only, unless "Publish to npm" is checked — use it
to rehearse a release without spending a version number.

Maintainers edit `npm/platforms.json` to add a platform; it is the single source
of truth for the package names, npm `os`/`cpu` fields, and the release archive
each package is built from.

