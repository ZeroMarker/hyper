# hyper-agent

Prebuilt binaries for [Hyper](https://github.com/ZeroMarker/hyper), a
terminal-first agent harness for local coding workflows.

This package is a thin wrapper: it contains no logic beyond locating and
executing the Rust binary that matches your platform, which is delivered by a
platform package such as `hyper-agent-linux-x64`. Nothing is compiled or
downloaded during install.

```bash
npm install -g hyper-agent
# or run it without installing
npx hyper-agent --help
```

Both command names become available:

```bash
hyper init
hyper run examples/hello.json
hy tui                     # `hy` is the short alias
hy "implement login"       # build mode
hy -p "analyze the bug"    # plan mode
```

Natural-language prompts call the DeepSeek API. Set `DEEPSEEK_API_KEY` (or run
`hyper config` once to store a key), and optionally `DEEPSEEK_MODEL` /
`DEEPSEEK_BASE_URL`.

## Supported platforms

| Platform | Arch | Package |
| --- | --- | --- |
| Linux (glibc) | x64 | `hyper-agent-linux-x64` |
| Linux (glibc) | arm64 | `hyper-agent-linux-arm64` |
| macOS | x64 | `hyper-agent-darwin-x64` |
| macOS | arm64 | `hyper-agent-darwin-arm64` |
| Windows | x64 | `hyper-agent-win32-x64` |

Platform packages are declared as `optionalDependencies`: npm installs only the
one matching your machine. If your install skipped optional dependencies
(`--omit=optional`, offline installs), reinstall with
`npm install --include=optional hyper-agent`.

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
- `hyper bash` runs shell commands and can write files. See the security notes
  in the [main README](https://github.com/ZeroMarker/hyper#reliability-and-limits)
  before pointing it at code you do not trust.

MIT licensed.
