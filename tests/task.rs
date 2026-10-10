use harness::{AgentMode, StepSpec, TaskSpec, VerifySpec, Workspace};
use std::collections::HashMap;

fn model_step(id: &str) -> StepSpec {
    StepSpec {
        id: id.into(),
        mode: AgentMode::Build,
        instruction: "make it pass".into(),
        tools: None,
        timeout_ms: None,
        limits: None,
        verify: None,
        metadata: HashMap::new(),
    }
}

fn step(id: &str) -> StepSpec {
    StepSpec {
        id: id.into(),
        mode: AgentMode::Build,
        instruction: "bash:echo ok".into(),
        tools: None,
        timeout_ms: None,
        limits: None,
        verify: None,
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
fn zero_resource_limits_are_rejected() {
    let mut task = TaskSpec {
        id: None,
        name: "bad limits".into(),
        steps: vec![step("one")],
        metadata: Default::default(),
    };
    task.steps[0].limits = Some(harness::BashResourceLimits {
        cpu_seconds: Some(0),
        ..Default::default()
    });
    assert!(
        task.validate()
            .unwrap_err()
            .to_string()
            .contains("cpuSeconds")
    );
}

/// `verify` is explicit lint/test configuration for a model step: it needs at
/// least one command, a bounded retry count, a build step, and a model loop to
/// feed a failure back to.
#[test]
fn verify_requires_a_bounded_model_step() {
    let mut task = TaskSpec {
        id: None,
        name: "verify".into(),
        steps: vec![model_step("one")],
        metadata: Default::default(),
    };

    task.steps[0].verify = Some(VerifySpec {
        commands: vec![],
        retries: 1,
    });
    assert!(
        task.validate()
            .unwrap_err()
            .to_string()
            .contains("at least one command")
    );

    task.steps[0].verify = Some(VerifySpec {
        commands: vec!["  ".into()],
        retries: 1,
    });
    assert!(
        task.validate()
            .unwrap_err()
            .to_string()
            .contains("must not be empty")
    );

    task.steps[0].verify = Some(VerifySpec {
        commands: vec!["cargo test".into()],
        retries: 99,
    });
    assert!(task.validate().unwrap_err().to_string().contains("at most"));

    task.steps[0].verify = Some(VerifySpec {
        commands: vec!["cargo test".into()],
        retries: 1,
    });
    task.steps[0].mode = AgentMode::Plan;
    assert!(
        task.validate()
            .unwrap_err()
            .to_string()
            .contains("build step")
    );

    task.steps[0].mode = AgentMode::Build;
    task.steps[0].instruction = "bash:cargo test".into();
    assert!(
        task.validate()
            .unwrap_err()
            .to_string()
            .contains("model step")
    );

    task.steps[0].instruction = "make it pass".into();
    assert!(task.validate().is_ok());
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
