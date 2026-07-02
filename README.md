# hyper

Agent harness for running repeatable, inspectable agent workflows.

`hyper` is intentionally small: it models a task as ordered steps, executes those steps through a pluggable runner, and records structured events that can be inspected or tested.

## Goals

- Keep agent workflows reproducible.
- Make task progress observable through events.
- Allow different execution backends without changing task definitions.
- Stay dependency-light for easy embedding in other projects.

## Quick Start

```bash
python -m hyper run examples/hello.json
```

Example task:

```json
{
  "name": "hello",
  "steps": [
    {
      "id": "greet",
      "instruction": "Say hello from the harness."
    }
  ]
}
```

## Development

```bash
python -m venv .venv
. .venv/bin/activate
python -m pip install -e .
python -m pytest
```

## License

MIT
