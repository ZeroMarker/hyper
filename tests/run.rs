use harness::{
    AgentMode, ApprovalGate, Checkpoint, StepSpec, TaskSpec, Workspace, get_run_details,
    restore_checkpoint, run_task, run_task_with_approval,
};
use std::{
    collections::HashMap,
    fs,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
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
fn search_treats_dash_prefix_query_literally() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("demo.txt"), "the --pre flag is text here").unwrap();
    // Before the `--` separator, rg parsed `--pre` as a flag (its `--pre`
    // option executes a program) instead of matching the literal string.
    let summary = run_task(
        &task("search", AgentMode::Build, "search:--pre"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "finished");
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
    let dir = tempdir().unwrap();
    // The shell records the pid of the background child it spawns, so the
    // assertion tracks *our* process instead of whatever `sleep` happens to be
    // running on the host (a bare `pgrep -f "sleep 60"` matches unrelated
    // processes and makes this test fail for the wrong reason).
    let mut task = task(
        "timeout",
        AgentMode::Build,
        "bash:sleep 60 & echo $! > child.pid; wait",
    );
    task.steps[0].timeout_ms = Some(1000);
    let summary = run_task(&task, dir.path()).unwrap();
    assert_eq!(summary.status, "failed");

    let pid: i32 = fs::read_to_string(dir.path().join("child.pid"))
        .expect("the shell never recorded its child pid")
        .trim()
        .parse()
        .expect("child.pid did not contain a pid");
    assert!(pid > 0);

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_running(pid) {
        assert!(
            Instant::now() < deadline,
            "process {pid} survived the timeout kill"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// True while `pid` exists and is not a zombie; a reaped or killed process is
/// gone (zombies have no code left to run, so they do not count as survivors).
#[cfg(target_os = "linux")]
fn process_is_running(pid: i32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(')')
            .map(|(_, rest)| !rest.trim_start().starts_with('Z'))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// A command that writes more than the OS pipe capacity (~64 KB) used to
/// deadlock: the child blocked on a full pipe while the harness waited for it
/// to exit, so the step only ended by hitting the timeout.
#[test]
fn large_output_does_not_deadlock_the_shell() {
    let dir = tempdir().unwrap();
    let mut task = task("big", AgentMode::Build, "bash:seq 1 20000");
    task.steps[0].timeout_ms = Some(10_000);
    let started = Instant::now();
    let summary = run_task(&task, dir.path()).unwrap();
    assert_eq!(summary.status, "finished", "{:?}", summary.failure);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the command was not drained while it ran"
    );

    let (_, events) = get_run_details(dir.path(), &summary.run_id).unwrap();
    let payload = &events
        .iter()
        .find(|event| event.event_type == "tool.finished")
        .expect("no tool.finished event")
        .payload;
    assert_eq!(payload["exitCode"], 0);
    assert!(
        payload["stdoutBytes"].as_u64().unwrap() > 64 * 1024,
        "the test command did not exceed the pipe buffer"
    );
}

#[test]
fn huge_output_is_capped_without_failing_the_step() {
    let dir = tempdir().unwrap();
    let mut task = task("huge", AgentMode::Build, "bash:seq 1 400000");
    task.steps[0].timeout_ms = Some(20_000);
    let summary = run_task(&task, dir.path()).unwrap();
    assert_eq!(summary.status, "finished", "{:?}", summary.failure);

    let (_, events) = get_run_details(dir.path(), &summary.run_id).unwrap();
    let payload = &events
        .iter()
        .find(|event| event.event_type == "tool.finished")
        .expect("no tool.finished event")
        .payload;
    assert_eq!(payload["truncated"], true);
    assert_eq!(payload["stdout"].as_str().unwrap().len(), 256 * 1024);
    assert!(payload["stdoutBytes"].as_u64().unwrap() > 256 * 1024);
}

#[test]
fn timeouts_are_reported_as_timeouts() {
    let dir = tempdir().unwrap();
    let mut task = task("timeout", AgentMode::Build, "bash:sleep 30");
    task.steps[0].timeout_ms = Some(200);
    let summary = run_task(&task, dir.path()).unwrap();
    assert_eq!(summary.status, "failed");
    let failure = summary.failure.expect("a timeout must be recorded");
    assert!(failure.message.contains("timed out"), "{}", failure.message);
    assert_eq!(failure.error_type, "TimeoutError");
    assert!(failure.retryable);
}

#[test]
fn write_without_a_content_line_is_rejected() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("keep.txt");
    fs::write(&file, "important content").unwrap();

    let summary = run_task(
        &task("truncate", AgentMode::Build, "write:keep.txt"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "failed");
    assert!(summary.failure.unwrap().message.contains("expected format"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "important content");

    // An explicitly empty content line still empties the file on purpose.
    let summary = run_task(
        &task("empty", AgentMode::Build, "write:keep.txt\n"),
        dir.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "finished");
    assert_eq!(fs::read_to_string(&file).unwrap(), "");
}

#[test]
fn crashed_runs_are_repaired_when_the_workspace_is_reopened() {
    let dir = tempdir().unwrap();
    let workspace = Workspace::open(dir.path()).unwrap();
    let run_id = "abandonedrun";
    workspace.prepare_run(run_id).unwrap();
    // Simulate a harness that died after announcing the run: the row says
    // `running` but nothing holds the run lock.
    workspace
        .create_run(
            run_id,
            &task("crash", AgentMode::Build, "bash:echo ok"),
            "2026-01-01T00:00:00.000Z",
        )
        .unwrap();
    assert_eq!(
        workspace.get_run(run_id).unwrap().unwrap().status,
        "running"
    );

    let reopened = Workspace::open(dir.path()).unwrap();
    assert_eq!(
        reopened.get_run(run_id).unwrap().unwrap().status,
        "interrupted"
    );
    assert!(
        dir.path()
            .join(".harness/runs")
            .join(run_id)
            .join("summary.json")
            .exists(),
        "the repaired run must carry a summary"
    );
    let (_, events) = get_run_details(dir.path(), run_id).unwrap();
    assert!(
        events.iter().any(|e| e.event_type == "run.interrupted"),
        "the repair must reach the JSONL audit log"
    );
}

#[test]
fn running_runs_are_not_repaired() {
    let dir = tempdir().unwrap();
    let task_path = dir.path().join("slow.json");
    fs::write(
        &task_path,
        r#"{"name":"slow","steps":[{"id":"s","mode":"build","instruction":"bash:sleep 2"}]}"#,
    )
    .unwrap();
    let mut harness_process = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .arg("run")
        .arg(&task_path)
        .current_dir(dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let run_id = loop {
        if let Some(row) = Workspace::open(dir.path())
            .unwrap()
            .list_runs(1)
            .unwrap()
            .into_iter()
            .next()
        {
            break row.run_id;
        }
        assert!(Instant::now() < deadline, "the slow run never started");
        std::thread::sleep(Duration::from_millis(20));
    };

    // Reopening the workspace while the run is alive must leave it alone.
    let reopened = Workspace::open(dir.path()).unwrap();
    assert_eq!(
        reopened.get_run(&run_id).unwrap().unwrap().status,
        "running",
        "a live run was wrongly repaired"
    );

    assert!(harness_process.wait().unwrap().success());
}

#[cfg(target_os = "linux")]
#[test]
fn killing_the_harness_kills_running_commands() {
    let dir = tempdir().unwrap();
    let task_path = dir.path().join("slow.json");
    // `exec` makes the shell become the long-running command, so the pid the
    // command records is the one that must not outlive the harness.
    fs::write(
        &task_path,
        r#"{"name":"slow","steps":[{"id":"s","mode":"build","instruction":"bash:echo $$ > shell.pid; exec sleep 60"}]}"#,
    )
    .unwrap();
    let mut harness_process = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .arg("run")
        .arg(&task_path)
        .current_dir(dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let pid_path = dir.path().join("shell.pid");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid_path.exists() {
        assert!(Instant::now() < deadline, "the command never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid: i32 = fs::read_to_string(&pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(process_is_running(pid));

    // A crash or `kill -9` must not leave the command running.
    harness_process.kill().unwrap();
    harness_process.wait().unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_running(pid) {
        assert!(
            Instant::now() < deadline,
            "process {pid} outlived the harness"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn cli_exit_status_follows_the_run_status() {
    let dir = tempdir().unwrap();
    let failing = dir.path().join("failing.json");
    fs::write(
        &failing,
        r#"{"name":"fails","steps":[{"id":"s","mode":"build","instruction":"bash:exit 4"}]}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .arg("run")
        .arg(&failing)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("\"status\": \"failed\""),
        "the summary must still be printed"
    );

    let passing = dir.path().join("passing.json");
    fs::write(
        &passing,
        r#"{"name":"passes","steps":[{"id":"s","mode":"build","instruction":"bash:echo ok"}]}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .arg("run")
        .arg(&passing)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
