# hyper-harness

Prebuilt binaries for [Hyper](https://github.com/ZeroMarker/hyper), a
terminal-first agent harness for local coding workflows.

This package is a thin wrapper: it contains no logic beyond locating and
executing the Rust binary that matches your platform, which is delivered by a
platform package such as `hyper-agent-linux-x64`. Nothing is compiled or
downloaded during install.

```bash
npm install -g hyper-harness
# or run it without installing
npx hyper-harness --help
```

Both command names become available:

```bash
hyper init
hyper run examples/hello.json
ha tui                     # `ha` is the short alias of `hyper`
ha "implement login"       # build mode
ha -p "analyze the bug"    # plan mode
```

Natural-language prompts call the configured provider, DeepSeek by default. Run
`hyper config` once to store the API key, the base URL and the model (they land
in the user configuration directory with owner-only permissions), or set
`DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` / `DEEPSEEK_MODEL` for that shell — the
environment wins over the stored file. To use a subscription such as OpenCode
Go, set the base URL to `https://opencode.ai/zen/go/v1`; Hyper then picks the
right wire protocol per model, or takes `DEEPSEEK_PROTOCOL`
(`chat` / `responses` / `messages`) when you want to be explicit.

Prompts can continue a conversation, which is what lets a follow-up refer to the
previous answer:

```bash
hyper plan "which files implement the policy check?" --session review
hyper plan "and which tests cover it?" --session review
hyper sessions                     # list conversations
hyper resume review                # open the TUI inside this one
```

## Supported platforms

| Platform | Arch | Package |
| --- | --- | --- |
| Linux (glibc) | x64 | `hyper-agent-linux-x64` |
| Linux (glibc) | arm64 | `hyper-agent-linux-arm64` |
| macOS | x64 | `hyper-agent-darwin-x64` |
| macOS | arm64 | `hyper-agent-darwin-arm64` |
| Windows | x64 | `hyper-agent-windows-x64` |

Platform packages are declared as `optionalDependencies`: npm installs only the
one matching your machine, which it enforces through their `os`/`cpu` fields. If
your install skipped optional dependencies (`--omit=optional`, offline
installs), reinstall with `npm install --include=optional hyper-harness`.

To use a binary you compiled yourself, point the shim at it:

```bash
export HYPER_BINARY_PATH=/path/to/target/release/hyper
```

## Notes

- The binaries are the same ones attached to the
  [GitHub releases](https://github.com/ZeroMarker/hyper/releases); npm is only a
  delivery channel.
- Requires Node.js 18 or newer to resolve and spawn the binary. The harness
  itself has no Node.js runtime dependency.
- The harness runs shell commands and writes files as part of a prompt: `hyper
  "run the tests and fix the failure"`. See the security notes
  in the [main README](https://github.com/ZeroMarker/hyper#reliability-and-limits)
  before pointing it at code you do not trust.

MIT licensed.
