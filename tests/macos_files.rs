#![cfg(target_os = "macos")]

use harness::{
    AgentMode, Permission, RunOptions, StepSpec, TaskSpec, ToolPermissions, Workspace,
    get_run_details, run_task_with_control,
};
use std::{collections::HashMap, fs, os::unix::fs::symlink, process::Command};

fn task(instruction: &str) -> TaskSpec {
    TaskSpec {
        id: None,
        name: "macos-files".into(),
        metadata: HashMap::new(),
        steps: vec![StepSpec {
            id: "step".into(),
            mode: AgentMode::Build,
            instruction: instruction.into(),
            tools: None,
            timeout_ms: None,
            limits: None,
            metadata: HashMap::new(),
        }],
    }
}

fn run(root: &std::path::Path, instruction: &str) -> harness::RunSummary {
    run_task_with_control(
        &task(instruction),
        root,
        RunOptions {
            permissions: ToolPermissions::with_mutations(Permission::Allow),
            ..RunOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn macos_direct_tools_create_edit_read_search_and_capture_original_bytes() {
    let root = tempfile::tempdir().unwrap();
    for instruction in [
        "write:src/nested/file.txt\noriginal",
        "read:src/nested/file.txt",
    ] {
        assert_eq!(
            run(root.path(), instruction).status,
            "finished",
            "{instruction}"
        );
    }
    let summary = run(root.path(), "edit:src/nested/file.txt\noriginal\nupdated");
    assert_eq!(summary.status, "finished");
    let workspace = Workspace::open(root.path()).unwrap();
    let checkpoints = workspace.list_checkpoints(&summary.run_id).unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(
        fs::read(&checkpoints[0].snapshot_path).unwrap(),
        b"original"
    );
    assert_eq!(
        fs::read(root.path().join("src/nested/file.txt")).unwrap(),
        b"updated"
    );
    let summary = run(root.path(), "search:updated");
    assert_eq!(summary.status, "finished");
    let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
    let lines = events
        .iter()
        .find(|e| e.event_type == "tool.finished")
        .unwrap()
        .payload["lines"]
        .as_array()
        .unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].as_str().unwrap().contains("file.txt"));
}

#[test]
fn macos_tools_preserve_internal_links_and_reject_audit_aliases() {
    let root = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    fs::write(root.path().join("original"), "ordinary").unwrap();
    symlink("original", root.path().join("internal")).unwrap();
    assert_eq!(
        run(root.path(), "edit:internal\nordinary\nupdated").status,
        "finished"
    );
    assert_eq!(fs::read(root.path().join("original")).unwrap(), b"updated");
    let marker = workspace.paths.dir.join("marker");
    fs::write(&marker, "AUDIT_SECRET").unwrap();
    fs::hard_link(&marker, root.path().join("alias")).unwrap();
    for instruction in [
        "read:alias",
        "write:alias\nFORGED",
        "edit:alias\nAUDIT_SECRET\nFORGED",
    ] {
        let summary = run(root.path(), instruction);
        assert_eq!(summary.status, "failed", "{instruction}");
        assert_eq!(summary.failure.unwrap().error_type, "PolicyError");
    }
    assert_eq!(fs::read(marker).unwrap(), b"AUDIT_SECRET");
    let summary = run(root.path(), "search:AUDIT_SECRET");
    assert_eq!(summary.status, "finished");
    let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
    let result = events
        .iter()
        .find(|e| e.event_type == "tool.finished")
        .unwrap();
    assert!(result.payload["lines"].as_array().unwrap().is_empty());
}

#[test]
fn macos_cli_jsonl_file_tools_match_persisted_events() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("task.json"),
        serde_json::to_vec(&task("write:new/file\ncontent")).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .current_dir(root.path())
        .args(["--approval", "allow", "--jsonl", "run", "task.json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let emitted: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let run_id = emitted[0]["runId"].as_str().unwrap();
    let (_, persisted) = get_run_details(root.path(), run_id).unwrap();
    assert_eq!(
        emitted,
        persisted
            .into_iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(fs::read(root.path().join("new/file")).unwrap(), b"content");
}
