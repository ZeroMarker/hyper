from __future__ import annotations

import argparse
import json
from dataclasses import asdict
from pathlib import Path

from hyper.harness import Harness, Task


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="hyper")
    subparsers = parser.add_subparsers(dest="command", required=True)

    run_parser = subparsers.add_parser("run", help="run a task file")
    run_parser.add_argument("task", type=Path, help="path to a JSON task file")

    args = parser.parse_args(argv)

    if args.command == "run":
        return run_task(args.task)

    parser.error(f"unknown command: {args.command}")
    return 2


def run_task(path: Path) -> int:
    task = Task.from_mapping(json.loads(path.read_text(encoding="utf-8")))
    harness = Harness()

    for event in harness.stream(task):
        print(json.dumps(asdict(event), ensure_ascii=False))

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
