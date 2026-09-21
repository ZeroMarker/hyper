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
cargo build --release   # target/release/{hyper,ha}
cargo test
```

Neither binary is on `PATH` until it is installed, so a bare `cargo build`
leaves `hyper: command not found`. Install them into `~/.cargo/bin`, which a Rust
toolchain already puts on `PATH`:

```bash
cargo install --path . --locked   # installs both `hyper` and `ha`
```

The release binaries are `target/release/hyper` and its short alias
`target/release/ha`. Both run the same program with the same commands; `ha`
changes nothing but the name, so `--version` reports `hyper` and only the usage
line names the command you actually typed.

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
ha                       # open TUI
ha "implement login"     # build mode
ha -p "analyze the bug"  # plan mode
```

`ha` is the short alias of `hyper`; every command below works with either name.

On the first `ha` launch, Hyper prompts for the provider API key and stores it,
along with the API base URL and the model, in the user configuration directory
with owner-only permissions. Run `ha config` to replace any of the three; the
base URL and model prompts accept their defaults with a bare Enter.

Environment variables override the stored file, which overrides the defaults,
so CI needs no configuration file:

```bash
export DEEPSEEK_API_KEY="sk-..."
export DEEPSEEK_BASE_URL="https://api.deepseek.com"   # default
export DEEPSEEK_MODEL="deepseek-v4-pro"               # default: deepseek-v4-flash
```

## Providers

Any OpenAI-compatible chat-completions service works: point `DEEPSEEK_BASE_URL`
(or the stored `base_url`) at it and set the model. DeepSeek is the default.

### OpenCode Go

[OpenCode Go](https://opencode.ai/docs/go) serves the same DeepSeek models under
a subscription, so the only change is the endpoint:

```bash
export DEEPSEEK_API_KEY="<opencode-go key>"           # from the OpenCode Zen console
export DEEPSEEK_BASE_URL="https://opencode.ai/zen/go/v1"
export DEEPSEEK_MODEL="deepseek-v4-flash"             # or deepseek-v4-pro
```

Or store the same three values once with `ha config` and drop the exports.
OpenCode Go requires every request to identify its conversation in
`x-opencode-session`; Hyper sends one id per step (all turns of that step share
it) and identifies itself as `hyper/<version>` rather than as its HTTP library.
Runs against a non-DeepSeek endpoint record that in the audit log: the
`model.*` events carry `"provider": "opencode-go"`, the base URL and the
protocol, so a run is never filed as a DeepSeek call it did not make.

### Protocols

A gateway can expose its models over more than one wire format, and OpenCode Go
does: the model determines which endpoint serves it.

| Protocol | Endpoint | OpenCode Go models |
| --- | --- | --- |
| `chat` | `{base}/chat/completions` | GLM, Kimi, LongCat, MiMo, DeepSeek, Hy |
| `responses` | `{base}/responses` | Grok, GPT, Muse Spark |
| `messages` | `{base}/messages` | MiniMax, Qwen3.6 Plus / 3.7 / 3.8 |

Hyper picks the protocol for you when the base URL is an OpenCode one, and
otherwise stays on `chat/completions`, which is what any OpenAI-compatible
service speaks. Set it explicitly when detection would guess wrong, or for a
gateway that maps models differently:

```bash
export DEEPSEEK_PROTOCOL="messages"   # chat | responses | messages
```

The value in the configuration file, written by `ha config`, is used the same
way, and the environment wins over it. An unrecognised name is rejected rather
than silently ignored. Each model's protocol is recorded in the run's
`model.started` event, so a surprising answer is always traceable to the
endpoint it came from.

Natural-language `plan`, `build`, and TUI prompts use the configured provider.
Explicit instruction prefixes continue to use local deterministic tools and do
not require an API key.

## Conversations

A prompt run belongs to a conversation, and a conversation is what gives a
follow-up its context.

```bash
ha plan "which files implement the policy check?" --session review
ha plan "and which tests cover it?" --session review
ha sessions                       # list conversations
ha session review                 # print the transcript
ha resume review                  # open the TUI inside this conversation
ha forget review                  # delete the conversation, keep its runs
```

Runs without `--session` stay standalone, and the TUI is a single conversation:
the first message opens one, every later message continues it, and the header
shows which one. `/new` starts a fresh conversation (the old one stays readable
under `sessions/`), and `/session` prints the current id so a CLI run can pick
the conversation up with `--session`.

The transcript keeps the prompt and the answer — what the model is replayed on
the next turn — while the tool calls behind them stay in the run's
`events.jsonl`. A conversation is therefore small and readable, and the full
trace is still there when you need it.

## Commands

```text
hyper config
hyper init
hyper validate <task.json>
hyper run <task.json>
hyper plan <prompt> [--session <session-id>]
hyper build <prompt> [--session <session-id>]
hyper runs [-n <limit>]
hyper show <run-id>
hyper sessions [-n <limit>]
hyper session <session-id>
hyper forget <session-id>
hyper tui
hyper resume <session-id>
hyper diff <run-id>
hyper artifacts <run-id>
hyper checkpoints <run-id>
hyper restore <run-id> <checkpoint-id>
hyper undo <run-id>
```

Every command can use `ha` instead, for example `ha tui` or `ha config`.
Common aliases remain available: `ha b`, `ha p`, `ha r`, `ha ls`, and `ha s`.
`ha diff` prints the file diffs recorded by `write`/`edit` tools, `ha artifacts`
lists the run's artifact files, `ha checkpoints` lists snapshots and
`ha restore <run> <checkpoint-id>` rewinds one file to a specific snapshot.

`ha run`, `ha plan`, `ha build` and a direct prompt exit with status `1` when the
run does not finish, so CI can rely on the exit status; the run summary is still
printed on stdout. Subcommand prompts may be passed unquoted as several words:
`ha plan fix the login bug` is the same as `ha -p "fix the login bug"`.

The TUI uses Ratatui and Crossterm. Press `Tab` to switch plan/build mode,
`Enter` to submit, arrow keys to select runs, and `Esc` to exit. Slash commands:
`/help`, `/runs`, `/session`, `/mode plan|build`, `/new`, and `/quit`. `/runs`
lists recent runs inline; `/session` shows the conversation the next message
will join.

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
  sessions/<session-id>.jsonl
```

JSONL remains the audit log; SQLite provides the local run/event index and the
conversation registry (`sessions` table: title, turn count, run count, last
update). A session id is also a file name, so it is restricted to letters,
digits, `-` and `_`.

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
- `ha artifacts` lists `runs/<run-id>/artifacts/`, but no tool writes there yet,
  so it is always empty. Truncated tool output is not kept anywhere either, so
  what a long command printed past the cap is gone once the run ends.
- The dangerous-command check is a **lightweight denylist over shell words, not a
  sandbox**. It rejects the accidents a model talks itself into: `rm`/`shred`/
  `chmod` and redirections aimed at `/`, the home directory, the workspace root
  or a system directory; machine-level programs (`sudo`, `dd`, `mkfs*`, `fdisk`,
  `shutdown`, `systemctl`, …); `curl … | sh`; and the same commands hidden one
  level down inside `sh -c '…'`, `sudo`, `env`, `timeout` or `xargs`. Quoting and
  command substitution are parsed rather than string-matched, so `rm -rf target`
  and `curl -o f.tar.gz` still work. It is **not** a containment boundary: a
  command that is not recognised still runs with your privileges, and a
  determined model can reach the same effect through a path this list does not
  name. Command-line runs apply the check without any approval prompt; the TUI
  additionally asks before every `bash` call.
- The workspace context sent to the model contains repository file contents, so a
  repository can influence the model's actions (prompt injection). Run Hyper on
  code you trust.

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
`ha` is linked to the same shim.

Publishing needs an `NPMJS_TOKEN` repository secret, passed to npm as
`NODE_AUTH_TOKEN`. Use a classic **Automation** token (Access Tokens → Generate
New Token → Classic → Automation), or a granular access token with *Bypass
two-factor authentication* enabled and read/write access to **all** packages — a
token scoped to `hyper-harness*` cannot be created before those packages exist. A
classic *Publish* token does not work unattended: npm rejects it with `EOTP`,
because using it to publish requires a one-time password.

Platform packages are published first, and the main package only once every one
of them is actually retrievable: npm acknowledges a publish while the upload is
still queued ("Your package is being processed and may take a few minutes to
become available"), and an optionalDependency that cannot be resolved yet is
skipped *silently* — which would leave an installed `hyper-harness` with the
shim but no binary.

The workflow stages and smoke tests the packages *before* uploading, and skips
versions that are already on the registry, so a partially failed release can be
re-run with `gh run rerun <run-id> --failed`. A manual run of the workflow does
the staging and smoke testing only, unless "Publish to npm" is checked — use it
to rehearse a release without spending a version number.

Maintainers edit `npm/platforms.json` to add a platform; it is the single source
of truth for the package names, npm `os`/`cpu` fields, and the release archive
each package is built from.

