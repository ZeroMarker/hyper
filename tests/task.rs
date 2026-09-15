use harness::{AgentMode, StepSpec, TaskSpec, Workspace};
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

#[test]
fn workspace_configures_sqlite_for_concurrent_access() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(dir.path()).unwrap();
    let journal_mode: String = workspace
        .db
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode.to_lowercase(), "wal");

    let busy_timeout: i64 = workspace
        .db
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap();
    assert_eq!(busy_timeout, 5_000);

    let synchronous: i64 = workspace
        .db
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .unwrap();
    assert_eq!(synchronous, 1);
}
