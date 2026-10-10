#![cfg(windows)]

use harness::{
    AgentMode, Permission, RunOptions, StepSpec, TaskSpec, ToolPermissions, Workspace,
    get_run_details, run_task_with_control,
};
use std::{collections::HashMap, fs};

fn run(root: &std::path::Path, instruction: &str) -> harness::RunSummary {
    let task = TaskSpec {
        id: None,
        name: "windows-hardlinks".into(),
        metadata: HashMap::new(),
        steps: vec![StepSpec {
            id: "step".into(),
            mode: AgentMode::Build,
            instruction: instruction.into(),
            tools: None,
            timeout_ms: None,
            limits: None,
            verify: None,
            metadata: HashMap::new(),
        }],
    };
    run_task_with_control(
        &task,
        root,
        RunOptions {
            permissions: ToolPermissions::with_mutations(Permission::Allow),
            ..RunOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn windows_tools_reject_legacy_and_external_audit_aliases_with_explicit_allow() {
    let root = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    fs::create_dir(root.path().join(".harness")).unwrap();
    for marker in [
        root.path().join(".harness/marker"),
        workspace.paths.dir.join("marker"),
    ] {
        fs::write(&marker, "AUDIT_SECRET").unwrap();
        let alias = root.path().join("visible.txt");
        fs::hard_link(&marker, &alias).unwrap();
        for instruction in [
            "read:visible.txt",
            "write:visible.txt\nFORGED",
            "edit:visible.txt\nAUDIT_SECRET\nFORGED",
        ] {
            let summary = run(root.path(), instruction);
            assert_eq!(summary.status, "failed", "{instruction}");
            assert_eq!(summary.failure.unwrap().error_type, "PolicyError");
            let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
            assert!(events.iter().any(|event| event.event_type == "tool.denied"));
            assert!(
                !events
                    .iter()
                    .any(|event| event.event_type == "tool.started")
            );
            assert_eq!(fs::read_to_string(&marker).unwrap(), "AUDIT_SECRET");
        }
        fs::remove_file(alias).unwrap();
    }
}

#[test]
fn windows_search_excludes_hardlink_content_and_keeps_regular_files() {
    let root = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    let marker = workspace.paths.dir.join("marker");
    fs::write(&marker, "AUDIT_SECRET").unwrap();
    fs::hard_link(marker, root.path().join("visible.txt")).unwrap();
    fs::write(root.path().join("safe.txt"), "AUDIT_SECRET public").unwrap();
    let summary = run(root.path(), "search:AUDIT_SECRET");
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
fn windows_ordinary_hardlinks_are_denied_but_single_link_tools_still_work() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("original"), "old").unwrap();
    fs::hard_link(root.path().join("original"), root.path().join("alias")).unwrap();
    assert_eq!(run(root.path(), "write:alias\nwrong").status, "failed");
    assert_eq!(
        fs::read_to_string(root.path().join("original")).unwrap(),
        "old"
    );
    fs::remove_file(root.path().join("alias")).unwrap();
    for instruction in [
        "read:original",
        "write:original\nnew",
        "edit:original\nnew\nupdated",
        "write:created\ncreated",
    ] {
        assert_eq!(
            run(root.path(), instruction).status,
            "finished",
            "{instruction}"
        );
    }
    assert_eq!(
        fs::read_to_string(root.path().join("original")).unwrap(),
        "updated"
    );
}
