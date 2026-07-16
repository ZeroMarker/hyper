# hyper

Rust-native, terminal-first agent harness for local coding workflows.

The CLI, task runner, policy engine, tools, workspace storage, SQLite index,
checkpoints, sessions, and full-screen TUI are implemented in Rust. There is no
Node.js runtime dependency.

DeepSeek is the default model provider for natural-language tasks. The default
model is `deepseek-v4-flash` and the default endpoint is
`https://api.deepseek.com`.

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
hyper artifacts <run-id>
hyper undo <run-id>
hyper tui
```

Every command can use `hy` instead, for example `hy tui`.
Common aliases remain available: `hy b`, `hy p`, `hy r`, `hy ls`, and `hy s`.

The TUI uses Ratatui and Crossterm. Press `Tab` to switch plan/build mode,
`Enter` to submit, arrow keys to select runs, and `Esc` to exit. Slash commands:
`/help`, `/runs`, `/mode plan|build`, `/new`, and `/quit`.

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

## Workspace

Runs are stored beneath `.harness/` using the existing compatible layout:

```text
.harness/
  harness.db
  runs/<run-id>/
    events.jsonl
    task.json
    summary.json
    artifacts/
    checkpoints/
  sessions/
```

JSONL remains the audit log; SQLite provides the local run/event index.
