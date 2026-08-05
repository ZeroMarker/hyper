use harness::{
    AgentMode, ApprovalGate, Checkpoint, StepSpec, TaskSpec, get_run_details, restore_checkpoint,
    run_task, run_task_with_approval,
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

fn task_with_tools(name: &str, tools: Vec<String>, instruction: &str) -> TaskSpec {
    TaskSpec {
        id: None,
        name: name.into(),
        steps: vec![StepSpec {
            id: "step".into(),
            mode: AgentMode::Build,
            instruction: instruction.into(),
            tools: Some(tools),
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
fn edit_requires_search_and_replace_lines() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("demo.txt"), "before after").unwrap();
    // Missing replace line would silently delete the search text; it must be rejected.
    let summary = run_task(
        &task("edit", AgentMode::Build, "edit:demo.txt\nbefore"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "failed");
    assert!(summary.failure.unwrap().message.contains("expected format"));
    assert_eq!(
        fs::read_to_string(dir.path().join("demo.txt")).unwrap(),
        "before after"
    );
}

#[test]
fn edit_replaces_first_occurrence() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("demo.txt"), "a b a").unwrap();
    let summary = run_task(
        &task("edit", AgentMode::Build, "edit:demo.txt\na\nX"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "finished");
    assert_eq!(
        fs::read_to_string(dir.path().join("demo.txt")).unwrap(),
        "X b a"
    );
}

#[test]
fn step_tools_allowlist_is_enforced() {
    let dir = tempdir().unwrap();
    let summary = run_task(
        &task_with_tools(
            "restricted",
            vec!["read".into()],
            "bash:echo should-not-run",
        ),
        dir.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "failed");
    assert!(
        summary
            .failure
            .unwrap()
            .message
            .contains("not allowed for step")
    );
}

#[test]
fn step_tools_allowlist_allows_configured_tools() {
    let dir = tempdir().unwrap();
    let summary = run_task(
        &task_with_tools("allowed", vec!["bash".into()], "bash:echo ok"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "finished");
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

#[test]
fn checkpoints_list_in_order_and_restore_individually() {
    let dir = tempdir().unwrap();
    let workspace = harness::Workspace::open(dir.path()).unwrap();
    let file = dir.path().join("demo.txt");
    fs::write(&file, "original").unwrap();
    let first = run_task(
        &task("first", AgentMode::Build, "write:demo.txt\nA"),
        dir.path(),
    )
    .unwrap();
    let second = run_task(
        &task("second", AgentMode::Build, "write:demo.txt\nB"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "B");

    let cps = workspace.list_checkpoints(&second.run_id).unwrap();
    assert_eq!(cps.len(), 1);
    restore_checkpoint(dir.path(), &cps[0]).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "A");

    let cps = workspace.list_checkpoints(&first.run_id).unwrap();
    assert_eq!(cps.len(), 1);
    restore_checkpoint(dir.path(), &cps[0]).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "original");
}

#[test]
fn approval_gate_allows_the_tool() {
    let dir = tempdir().unwrap();
    let gate = ApprovalGate::new();
    let worker = gate.clone();
    let handle = std::thread::spawn(move || {
        run_task_with_approval(
            &task("gated", AgentMode::Build, "bash:echo ok"),
            dir.path(),
            worker,
        )
        .unwrap()
    });
    let request = wait_for_request(&gate);
    assert_eq!(request.tool, "bash");
    request.response.send(true).unwrap();
    let summary = handle.join().unwrap();
    assert_eq!(summary.status, "finished");
}

#[test]
fn approval_gate_denial_fails_the_step() {
    let dir = tempdir().unwrap();
    let gate = ApprovalGate::new();
    let worker = gate.clone();
    let handle = std::thread::spawn(move || {
        run_task_with_approval(
            &task("gated", AgentMode::Build, "bash:echo should-not-run"),
            dir.path(),
            worker,
        )
        .unwrap()
    });
    let request = wait_for_request(&gate);
    request.response.send(false).unwrap();
    let summary = handle.join().unwrap();
    assert_eq!(summary.status, "failed");
    assert!(summary.failure.unwrap().message.contains("denied"));
}

fn wait_for_request(gate: &ApprovalGate) -> harness::ApprovalRequest {
    loop {
        if let Some(request) = gate.drain().into_iter().next() {
            return request;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn bash_timeout_kills_entire_process_group() {
    use std::process::Command;
    let dir = tempdir().unwrap();
    let mut task = task("timeout", AgentMode::Build, "bash:sleep 60");
    task.steps[0].timeout_ms = Some(300);
    let summary = run_task(&task, dir.path()).unwrap();
    assert_eq!(summary.status, "failed");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut survivors = 1;
    while std::time::Instant::now() < deadline {
        let out = Command::new("pgrep")
            .args(["-f", "^sleep 60$"])
            .output()
            .unwrap();
        survivors = String::from_utf8_lossy(&out.stdout).trim().lines().count();
        if survivors == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(survivors, 0, "sleep 60 survived the timeout");
}
