# Changelog

All notable, user-visible changes to Hyper are documented here. Earlier
releases predate this file; their notes are on the
[GitHub Releases](https://github.com/ZeroMarker/hyper/releases) page.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Switch provider profiles inside the TUI.** `/profile` lists the configured
  profiles and marks the active one; `/profile <name>` makes it active, and the
  next message already uses it.
- **List and switch models.** `hyper profile models [name]` lists the models a
  provider offers through its `GET /models` endpoint, and
  `hyper profile model <name>` switches the active profile's model, updating a
  detected protocol along with it.
- **Providers and models from TUI pickers.** `/model` opens a searchable model
  list fetched from the provider, `/profile` a provider list, and `/connect`
  a form to add a provider (preset or custom base URL, masked API key) that
  continues straight into model selection. The header shows
  `profile / model`.
- **Configurable turn limit and request timeout.** `HYPER_MAX_TURNS`
  (default 12, up to 200) sets how many tool-calling turns an agent step may
  take, and `HYPER_REQUEST_TIMEOUT_SECS` (default 120, up to 3600) sets the
  per-request model timeout.

- **Always allow in the approval modal.** `a` approves and stops asking for the
  rest of the TUI session: the exact command for `bash`, the tool for
  `write`/`edit`.
- **Approval shows what will change.** `write`/`edit` approvals show a colored
  unified diff, and commands are shown whole and wrapped instead of cut to one
  line.

- **A real message box.** Cursor movement and shell-style line editing
  (`Ctrl-A/E/W/U/K`, word jumps, `Delete`); multi-line messages via
  `Shift/Alt-Enter`, `Ctrl-J` or a trailing `\`, growing up to eight rows;
  bracketed paste so pasted newlines never send the message; and `↑`/`↓`
  history of earlier messages, saved per workspace.

### Changed

- `↑`/`↓` recall input history instead of scrolling the chat; scroll with
  `PageUp`/`PageDown` or the mouse wheel.

- In the TUI, `Esc` no longer exits while idle; it clears the input. `Ctrl-C`
  clears the input first and exits only on a second press within two seconds.

### Fixed

- A lone command-like word such as `hyper paths` is no longer sent to the model
  as a build task; it is rejected with the closest subcommand. Send one-word
  tasks with `hyper build <word>`.
- Request and history budgets count one token per character instead of per
  UTF-8 byte, so CJK text is no longer charged three tokens per character and
  no longer exhausts the context budget early.
- `read` with a byte `offset` no longer starts or ends inside a multi-byte
  character; the observation reports the offset actually used.
- `read` returned up to 64 KB, but the model only saw the first and last ~2 KB
  of it, so it re-read files it had never fully seen. A read now returns about
  3 KB that fits one observation whole, cut at whole lines for line windows,
  and reports `nextLine` or `nextOffset`. The tool definition now also
  advertises the `line`/`lines` window, which the model could not discover
  before.
- The agent is steered to the `search` tool instead of `grep -r`/`find` in
  `bash`, which also scan ignored build output such as `target/` (one such
  scan took 100 s), and to answer requests unrelated to the project directly.

- The TUI header now shows the model the next run will use (environment,
  active profile, then legacy configuration) instead of only
  `DEEPSEEK_MODEL` or the built-in default.

## [0.3.0] - 2026-10-13

The first release since 0.2.0. It adds named provider profiles, auditable
context compaction, project instructions, line-window reads, atomic edits with
a verification loop, and a hardened external audit store with per-platform file
and shell boundaries.

### Added

- **Named provider profiles.** `profiles` and `active_profile` in the user
  configuration select a protocol, base URL, model, authentication source and
  optional per-protocol endpoint overrides. `ha profile list | use | test`
  inspects and switches them without printing keys, and a run freezes its
  effective settings (recorded without secrets). The legacy single
  configuration and `DEEPSEEK_*` environment overrides keep working.
- **Auditable context compaction.** `ha compact <session> --keep-turns <N>`
  summarizes a conversation's older turns into
  `sessions/<id>.compactions.jsonl`; the transcript is never rewritten and
  `ha replay` stays exact. `HYPER_AUTOCOMPACT=1` summarizes by budget before a
  run, and `HYPER_LOOP_COMPACT=1` summarizes older tool turns inside the agent
  loop without splitting a call from its result.
- **Project instructions.** Every non-ignored `AGENTS.md` in the workspace is
  loaded root-first, sharing a 16 KB budget, delivered as labeled blocks, and
  recorded with its provenance so replay reproduces the request byte for byte.
- **Line-window reads.** `read` accepts a 1-based `line` plus an optional
  `lines` count and reports `startLine`/`endLine`/`totalLines`; the whole-file
  `sha256` is still returned.
- **Edit safeguards and verification.** `edit` requires a unique match or an
  explicit 1-based `occurrence`, verifies an expected or previously read
  `sha256`, and refuses a file whose bytes changed. A build step can declare
  `verify` commands that run through the normal policy, sandbox, timeout and
  resource limits, with a failure fed back for a bounded number of retries.
- **External audit storage.** Authoritative run, session and checkpoint data
  now live outside the checkout; `ha state` prints the paths and
  `ha migrate-state --from <path>` imports a legacy `.harness` directory or a
  backup explicitly.
- **Shared tool permissions.** CLI and TUI share `allow` / `ask` / `deny`
  decisions, `--permissions FILE` adds literal path and command scopes, and the
  effective policy and source are recorded per run.
- **Live automation and cancellation.** `--jsonl` streams persisted events as
  they are written, and one cancellation source stops model waits, retries,
  approvals and shell process groups across the CLI and TUI (Unix `SIGINT`,
  `SIGTERM`, `SIGHUP`; TUI `Esc`/`Ctrl-C`).
- **Shell limits.** Linux `bash` runs under a Landlock workspace-write boundary
  (or `read-only`) plus seccomp socket filtering and `RLIMIT_AS`,
  `RLIMIT_FSIZE` and `RLIMIT_CPU`.
- **Evaluation harness.** A fixed offline task suite, independent graders, and
  a pinned real-model JSONL/Markdown baseline runner live under `evals/`.
- **Localization.** Interface text is English by default; `HYPER_LANG=zh`
  selects Chinese.

### Changed

- Session history and the total request budget are bounded by
  `HYPER_HISTORY_TOKENS`, `HYPER_CONTEXT_TOKENS` and `HYPER_OUTPUT_TOKENS`,
  with each estimate and decision recorded in `model.context_budget`.
- `write` and `edit` stage their content and commit atomically, leaving the
  previous bytes in place on failure and preserving snapshots and hardlink
  aliases.
- Interrupted or incomplete model replies are rejected before any tool in the
  reply runs, for all three protocols.

### Fixed

- `bash` drains stdout and stderr while the command runs, so large output no
  longer deadlocks, and a timeout is reported distinctly as `TimeoutError`
  with `"timedOut": true`.
- Benign redirects to `/dev/null` are allowed while other `/dev` targets stay
  protected.
- A crash no longer leaves a run stuck in `running`; the next workspace open
  repairs it to `interrupted`, and a missing SQLite index is rebuilt from the
  JSONL log.
- The Windows atomic replace closes its target handle before replacing.

### Security

- Linux file tools open through anchored `openat2` descriptors; restore and
  undo commit through pinned directory descriptors and external staging.
- A `read-only` Linux shell rejects explicit metadata mutations via seccomp, and
  both restricted modes deny explicit socket syscalls.
- macOS file tools walk pinned directory descriptors, and administrative
  checkpoint/restore use the same boundary.
- Windows file tools reject hardlink aliases.
- The audit store sits outside the workspace and is anchored to the
  workspace-root inode, so a different directory at the same path cannot reuse
  it.
- Repository and project-instruction content is treated as untrusted input: it
  can guide the model but cannot widen tool permissions, the sandbox, the
  workspace boundary or audit protection.

[0.3.0]: https://github.com/ZeroMarker/hyper/compare/v0.2.0...v0.3.0
