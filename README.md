# hyper

Terminal-first agent harness for local coding workflows.

This MVP is a local product, not a general-purpose SDK. It provides a CLI and a minimal interactive TUI for running structured agent tasks, recording JSONL events, indexing runs in SQLite, and preserving artifacts/checkpoints under a project workspace.

## Quick Start

```bash
npm install
npm run build
node dist/cli/index.js init
node dist/cli/index.js run examples/hello.json
node dist/cli/index.js runs
```

During development:

```bash
npm run dev -- run examples/hello.json
npm run dev -- tui
```

## Workspace

`harness init` creates a local `.harness/` directory:

```text
.harness/
  harness.db
  runs/
    <run-id>/
      events.jsonl
      task.json
      summary.json
      artifacts/
      checkpoints/
  sessions/
```

`events.jsonl` is the source of truth. SQLite is used only for local indexing and queries.

## Commands

```bash
harness init
harness validate <task.json>
harness run <task.json>
harness plan "<query>"
harness build "<command>"
harness runs
harness show <run-id>
harness artifacts <run-id>
harness undo <run-id>
harness tui
```

When running from source, replace `harness` with `node dist/cli/index.js` or `npm run dev --`.

## Task Format

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

Supported instruction prefixes:

- `bash:<command>`
- `read:<path>`
- `search:<text>`
- `write:<path>\n<content>`
- `edit:<path>\n<search>\n<replace>`

`plan` mode is read-only and denies file writes.

## Verification

```bash
npm run typecheck
npm test
npm run build
```
