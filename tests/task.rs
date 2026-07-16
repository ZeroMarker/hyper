use harness::{AgentMode, StepSpec, TaskSpec};
use std::collections::HashMap;

fn step(id: &str) -> StepSpec {
    StepSpec {
        id: id.into(),
        mode: AgentMode::Build,
        instruction: "bash:echo ok".into(),
        tools: None,
        timeout_ms: None,
        metadata: HashMap::new(),
    }
}

#[test]
fn valid_task_defaults_to_build() {
    let task = TaskSpec {
        id: None,
        name: "sample".into(),
        steps: vec![step("one")],
        metadata: HashMap::new(),
    };
    assert!(task.validate().is_ok());
    assert_eq!(task.steps[0].mode, AgentMode::Build)
}

#[test]
fn duplicate_ids_are_rejected() {
    let task = TaskSpec {
        id: None,
        name: "sample".into(),
        steps: vec![step("one"), step("one")],
        metadata: HashMap::new(),
    };
    assert!(
        task.validate()
            .unwrap_err()
            .to_string()
            .contains("duplicate step id")
    )
}

#[test]
fn json_defaults_are_compatible() {
    let task: TaskSpec = serde_json::from_str(
        r#"{"name":"sample","steps":[{"id":"one","instruction":"bash:echo ok"}]}"#,
    )
    .unwrap();
    assert_eq!(task.steps[0].mode, AgentMode::Build);
    assert!(task.validate().is_ok())
}
