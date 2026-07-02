from hyper import Harness, Step, Task


def test_harness_emits_task_and_step_events() -> None:
    task = Task(
        name="sample",
        steps=(Step(id="one", instruction="Do one thing."),),
    )

    events = Harness().run(task)

    assert [event.type for event in events] == [
        "task.started",
        "step.started",
        "step.finished",
        "task.finished",
    ]
    assert events[2].payload["status"] == "noop"


def test_task_from_mapping_validates_required_fields() -> None:
    task = Task.from_mapping(
        {
            "name": "sample",
            "steps": [{"id": "one", "instruction": "Do one thing."}],
        }
    )

    assert task.name == "sample"
    assert task.steps[0].id == "one"
