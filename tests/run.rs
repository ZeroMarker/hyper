use harness::{
    AgentMode, Checkpoint, StepSpec, TaskSpec, get_run_details, restore_checkpoint, run_task,
};
use std::{collections::HashMap, fs};
use tempfile::tempdir;

fn task(name: &str, mode: AgentMode, instruction: &str) -> TaskSpec {
    TaskSpec {
        id: None,
        name: name.into(),
        steps: vec![StepSpec {
            id: "step".into(),
            mode,
            instruction: instruction.into(),
            tools: None,
            timeout_ms: None,
            metadata: HashMap::new(),
        }],
        metadata: HashMap::new(),
    }
}

#[test]
fn shell_run_records_events() {
    let dir = tempdir().unwrap();
    let summary = run_task(&task("hello", AgentMode::Build, "bash:echo ok"), dir.path()).unwrap();
    assert_eq!(summary.status, "finished");
    let (_, events) = get_run_details(dir.path(), &summary.run_id).unwrap();
    assert!(events.iter().any(|e| e.event_type == "run.finished"));
    assert!(events.iter().any(|e| e.event_type == "tool.finished"))
}

#[test]
fn plan_mode_denies_writes() {
    let dir = tempdir().unwrap();
    let summary = run_task(
        &task("readonly", AgentMode::Plan, "write:demo.txt\nnope"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "failed");
    assert!(summary.failure.unwrap().message.contains("read-only"))
}

#[test]
fn nonzero_shell_fails() {
    let dir = tempdir().unwrap();
    let summary = run_task(&task("fail", AgentMode::Build, "bash:exit 7"), dir.path()).unwrap();
    assert_eq!(summary.status, "failed");
    assert!(summary.failure.unwrap().message.contains("exit code 7"))
}

#[test]
fn path_escape_is_rejected() {
    let dir = tempdir().unwrap();
    let summary = run_task(
        &task("escape", AgentMode::Build, "read:../outside.txt"),
        dir.path(),
    )
    .unwrap();
    assert!(
        summary
            .failure
            .unwrap()
            .message
            .contains("escapes workspace root")
    )
}

#[cfg(unix)]
#[test]
fn symlink_read_escape_is_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), "outside").unwrap();
    symlink(outside.path(), dir.path().join("link")).unwrap();
    let summary = run_task(
        &task("escape", AgentMode::Build, "read:link/secret.txt"),
        dir.path(),
    )
    .unwrap();
    assert!(
        summary
            .failure
            .unwrap()
            .message
            .contains("escapes workspace root")
    )
}

#[cfg(unix)]
#[test]
fn symlink_write_escape_is_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    symlink(outside.path(), dir.path().join("link")).unwrap();
    let summary = run_task(
        &task("escape", AgentMode::Build, "write:link/evil.txt\npwned"),
        dir.path(),
    )
    .unwrap();
    assert!(
        summary
            .failure
            .unwrap()
            .message
            .contains("escapes workspace root")
    );
    assert!(!outside.path().join("evil.txt").exists());
}

#[test]
fn write_creates_nested_directories() {
    let dir = tempdir().unwrap();
    let summary = run_task(
        &task("write", AgentMode::Build, "write:a/b/c.txt\nhello"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "finished");
    assert_eq!(
        fs::read_to_string(dir.path().join("a/b/c.txt")).unwrap(),
        "hello"
    );
}

#[test]
fn write_can_be_restored() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("demo.txt");
    fs::write(&file, "before").unwrap();
    let summary = run_task(
        &task("write", AgentMode::Build, "write:demo.txt\nafter"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "after");
    let cp_dir = dir
        .path()
        .join(".harness/runs")
        .join(summary.run_id)
        .join("checkpoints");
    let cp_file = fs::read_dir(cp_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|x| x.path())
        .find(|p| p.extension().is_some_and(|x| x == "json"))
        .unwrap();
    let cp: Checkpoint = serde_json::from_slice(&fs::read(cp_file).unwrap()).unwrap();
    restore_checkpoint(dir.path(), &cp).unwrap();
    assert_eq!(fs::read_to_string(file).unwrap(), "before")
}
