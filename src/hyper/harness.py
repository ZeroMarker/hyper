from __future__ import annotations

from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from datetime import UTC, datetime
from typing import Any


@dataclass(frozen=True)
class Step:
    id: str
    instruction: str
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class Task:
    name: str
    steps: tuple[Step, ...]
    metadata: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_mapping(cls, data: dict[str, Any]) -> "Task":
        steps = tuple(
            Step(
                id=str(step["id"]),
                instruction=str(step["instruction"]),
                metadata=dict(step.get("metadata", {})),
            )
            for step in data.get("steps", [])
        )

        if not data.get("name"):
            raise ValueError("task requires a name")
        if not steps:
            raise ValueError("task requires at least one step")

        return cls(
            name=str(data["name"]),
            steps=steps,
            metadata=dict(data.get("metadata", {})),
        )


@dataclass(frozen=True)
class Event:
    type: str
    task: str
    step: str | None
    timestamp: str
    payload: dict[str, Any] = field(default_factory=dict)


Runner = Callable[[Step], dict[str, Any] | None]


class Harness:
    def __init__(self, runner: Runner | None = None) -> None:
        self.runner = runner or self._default_runner

    def run(self, task: Task) -> list[Event]:
        events: list[Event] = [self._event("task.started", task.name, None)]

        for step in task.steps:
            events.append(self._event("step.started", task.name, step.id))
            result = self.runner(step) or {}
            events.append(self._event("step.finished", task.name, step.id, result))

        events.append(self._event("task.finished", task.name, None))
        return events

    def stream(self, task: Task) -> Iterable[Event]:
        for event in self.run(task):
            yield event

    @staticmethod
    def _default_runner(step: Step) -> dict[str, Any]:
        return {"instruction": step.instruction, "status": "noop"}

    @staticmethod
    def _event(
        event_type: str,
        task_name: str,
        step_id: str | None,
        payload: dict[str, Any] | None = None,
    ) -> Event:
        return Event(
            type=event_type,
            task=task_name,
            step=step_id,
            timestamp=datetime.now(UTC).isoformat(),
            payload=payload or {},
        )
