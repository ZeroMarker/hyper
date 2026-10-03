use harness::{
    AgentMode, ApprovalGate, ExecutionMode, Permission, RunOptions, StepSpec, TaskSpec,
    ToolPermissions, Workspace, get_run_details, run_task, run_task_with_control,
};
use std::{
    collections::HashMap,
    fs,
    process::Command,
    time::{Duration, Instant},
};
use tempfile::tempdir;

fn task(instructions: &[&str]) -> TaskSpec {
    TaskSpec {
        id: None,
        name: "permissions".into(),
        metadata: HashMap::new(),
        steps: instructions
            .iter()
            .enumerate()
            .map(|(i, instruction)| StepSpec {
                id: format!("step-{i}"),
                mode: AgentMode::Build,
                instruction: (*instruction).into(),
                tools: None,
                timeout_ms: None,
                limits: None,
                metadata: HashMap::new(),
            })
            .collect(),
    }
}

#[test]
fn file_tools_cannot_read_or_overwrite_audit_paths_even_with_allow() {
    let root = tempdir().unwrap();
    Workspace::open(root.path()).unwrap();
    let marker = root.path().join(".harness/marker");
    fs::write(&marker, "original").unwrap();
    let absolute = marker.to_string_lossy().into_owned();
    for path in [
        ".harness/marker",
        "src/../.harness/marker",
        absolute.as_str(),
        ".harness/runs/forged/events.jsonl",
        ".harness/sessions/forged.jsonl",
        ".harness/harness.db",
        ".harness/tmp/agent.txt",
        ".harness/runs/fake/checkpoints/fake.snapshot",
    ] {
        for instruction in [
            format!("write:{path}\nforged"),
            format!("edit:{path}\noriginal\nforged"),
            format!("read:{path}"),
        ] {
            let summary = run_task(&task(&[&instruction]), root.path()).unwrap();
            assert_eq!(summary.status, "failed", "{instruction}");
            assert_eq!(summary.failure.unwrap().error_type, "PolicyError");
            let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
            assert!(
                events.iter().any(|e| e.event_type == "tool.denied"
                    && e.payload["source"] == "execution-boundary")
            );
            assert!(!events.iter().any(|e| e.event_type == "tool.started"));
            assert_eq!(fs::read_to_string(&marker).unwrap(), "original");
        }
    }
    assert!(!root.path().join(".harness/runs/forged").exists());
}

#[cfg(unix)]
#[test]
fn audit_symlink_and_hardlink_aliases_are_rejected() {
    use std::os::unix::fs::symlink;
    let root = tempdir().unwrap();
    Workspace::open(root.path()).unwrap();
    let marker = root.path().join(".harness/marker");
    fs::write(&marker, "original").unwrap();
    symlink(root.path().join(".harness"), root.path().join("alias")).unwrap();
    fs::hard_link(&marker, root.path().join("hardlink")).unwrap();
    for path in ["alias/marker", "alias/runs/fake/events.jsonl", "hardlink"] {
        let summary = run_task(&task(&[&format!("write:{path}\nforged")]), root.path()).unwrap();
        assert_eq!(summary.status, "failed");
        assert_eq!(summary.failure.unwrap().error_type, "PolicyError");
        assert_eq!(fs::read_to_string(&marker).unwrap(), "original");
    }
}

#[test]
fn explicit_allow_does_not_prompt_or_expand_execution_boundaries() {
    let root = tempdir().unwrap();
    let gate = ApprovalGate::new();
    let options = || RunOptions {
        gate: Some(gate.clone()),
        permissions: ToolPermissions::with_mutations(Permission::Allow),
        ..RunOptions::default()
    };
    let summary =
        run_task_with_control(&task(&["write:ok.txt\nok"]), root.path(), options()).unwrap();
    assert_eq!(summary.status, "finished");
    assert!(gate.drain().is_empty());
    let mut readonly = options();
    readonly.execution_mode = ExecutionMode::ReadOnly;
    let summary =
        run_task_with_control(&task(&["write:no.txt\nwrong"]), root.path(), readonly).unwrap();
    assert_eq!(summary.status, "failed");
    assert!(!root.path().join("no.txt").exists());
    let mut plan = task(&["write:no.txt\nwrong"]);
    plan.steps[0].mode = AgentMode::Plan;
    assert_eq!(
        run_task_with_control(&plan, root.path(), options())
            .unwrap()
            .status,
        "failed"
    );
    assert!(gate.drain().is_empty());
    let outside = tempdir().unwrap();
    let instruction = format!("write:{}\nwrong", outside.path().join("escape").display());
    assert_eq!(
        run_task_with_control(&task(&[&instruction]), root.path(), options())
            .unwrap()
            .status,
        "failed"
    );
    assert!(!outside.path().join("escape").exists());
}

#[test]
fn asking_approves_only_one_invocation_and_can_apply_to_reads() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("input.txt"), "data").unwrap();
    let gate = ApprovalGate::new();
    let worker_gate = gate.clone();
    let path = root.path().to_owned();
    let worker = std::thread::spawn(move || {
        run_task_with_control(
            &task(&[
                "read:input.txt",
                "write:first.txt\nkeep",
                "write:second.txt\nwrong",
            ]),
            path,
            RunOptions {
                gate: Some(worker_gate),
                permissions: ToolPermissions {
                    read: Permission::Ask,
                    ..ToolPermissions::default()
                },
                ..RunOptions::default()
            },
        )
        .unwrap()
    });
    for (tool, approve) in [("read", true), ("write", true), ("write", false)] {
        let deadline = Instant::now() + Duration::from_secs(5);
        let request = loop {
            let requests = gate.drain();
            assert!(requests.len() <= 1);
            if let Some(request) = requests.into_iter().next() {
                break request;
            }
            assert!(Instant::now() < deadline, "approval never arrived");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(request.tool, tool);
        request.response.send(approve).unwrap();
    }
    let summary = worker.join().unwrap();
    assert_eq!(summary.status, "failed");
    assert_eq!(
        fs::read_to_string(root.path().join("first.txt")).unwrap(),
        "keep"
    );
    assert!(!root.path().join("second.txt").exists());
    let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "tool.approval")
            .count(),
        3
    );
}

#[test]
fn deny_stops_before_approval_and_before_shell_spawn() {
    let root = tempdir().unwrap();
    let gate = ApprovalGate::new();
    for instruction in ["write:no.txt\nwrong", "bash:echo wrong > no.txt"] {
        let summary = run_task_with_control(
            &task(&[instruction]),
            root.path(),
            RunOptions {
                gate: Some(gate.clone()),
                permissions: ToolPermissions::with_mutations(Permission::Deny),
                ..RunOptions::default()
            },
        )
        .unwrap();
        assert_eq!(summary.status, "failed");
        assert!(gate.drain().is_empty());
        assert!(!root.path().join("no.txt").exists());
        let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.event_type == "tool.policy" && e.payload["decision"] == "deny")
        );
        assert!(!events.iter().any(|e| e.event_type == "tool.started"));
    }
}

#[test]
fn cli_default_ask_fails_closed_without_polluting_jsonl() {
    let root = tempdir().unwrap();
    fs::write(
        root.path().join("task.json"),
        serde_json::to_vec(&task(&["write:no.txt\nwrong"])).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .env_remove("HYPER_APPROVAL")
        .args(["--jsonl", "run", "task.json"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!root.path().join("no.txt").exists());
    let events: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "tool.approval" && e["payload"]["approved"] == false)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive approval handler"));
}

#[test]
fn cli_permissions_are_explicit_strict_and_record_precedence() {
    let root = tempdir().unwrap();
    fs::write(
        root.path().join("task.json"),
        serde_json::to_vec(&task(&["write:out.txt\nok"])).unwrap(),
    )
    .unwrap();
    let config = root.path().join("permissions.json");
    fs::write(&config, r#"{"write":"allow","bash":"deny"}"#).unwrap();
    let run = |override_flag: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hyper"))
            .env("HYPER_APPROVAL", "deny")
            .args(["--permissions", config.to_str().unwrap(), "--jsonl"])
            .args(override_flag)
            .args(["run", "task.json"])
            .current_dir(root.path())
            .output()
            .unwrap()
    };
    let output = run(&[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events[0]["payload"]["permissions"]["write"], "allow");
    assert!(
        events[0]["payload"]["permissionSource"]
            .as_str()
            .unwrap()
            .starts_with("file:")
    );
    fs::remove_file(root.path().join("out.txt")).unwrap();
    assert_eq!(run(&["--approval", "deny"]).status.code(), Some(1));
    assert!(!root.path().join("out.txt").exists());
    fs::write(&config, r#"{"wrtie":"allow"}"#).unwrap();
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid permissions file"));
}

#[cfg(unix)]
#[test]
fn search_does_not_return_audit_content_through_hardlinks() {
    let root = tempdir().unwrap();
    Workspace::open(root.path()).unwrap();
    let marker = root.path().join(".harness/marker");
    fs::write(&marker, "AUDIT_SECRET_SENTINEL").unwrap();
    fs::hard_link(marker, root.path().join("visible.txt")).unwrap();
    fs::write(root.path().join("safe.txt"), "AUDIT_SECRET_SENTINEL public").unwrap();
    let summary = run_task(&task(&["search:AUDIT_SECRET_SENTINEL"]), root.path()).unwrap();
    assert_eq!(summary.status, "finished");
    let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
    let result = events
        .iter()
        .find(|e| e.event_type == "tool.finished")
        .unwrap();
    let lines = result.payload["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].as_str().unwrap().contains("safe.txt"));
}

#[test]
fn explicit_cli_policy_overrides_invalid_lower_precedence_environment() {
    let root = tempdir().unwrap();
    fs::write(
        root.path().join("task.json"),
        serde_json::to_vec(&task(&["write:out.txt\nok"])).unwrap(),
    )
    .unwrap();
    let execute = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hyper"))
            .env("HYPER_APPROVAL", "typo")
            .args(args)
            .args(["run", "task.json"])
            .current_dir(root.path())
            .output()
            .unwrap()
    };
    let output = execute(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!root.path().join(".harness").exists());
    assert!(execute(&["--approval", "allow"]).status.success());
    fs::write(root.path().join("permissions.json"), r#"{"write":"allow"}"#).unwrap();
    assert!(
        execute(&["--permissions", "permissions.json"])
            .status
            .success()
    );
}
