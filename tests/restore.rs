use harness::{AgentMode, StepSpec, TaskSpec, Workspace, restore_checkpoint, run_task};
use std::{collections::HashMap, fs, process::Command};
use tempfile::tempdir;

fn task(instruction: &str) -> TaskSpec {
    TaskSpec {
        id: None,
        name: "restore-boundary".into(),
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
    }
}

#[test]
fn restore_and_undo_cli_restore_existing_files_and_remove_new_files() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("file"), "before").unwrap();
    let run = run_task(&task("write:file\nafter"), root.path()).unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    let cp = workspace.list_checkpoints(&run.run_id).unwrap().remove(0);
    let result = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .current_dir(root.path())
        .args(["restore", &run.run_id, &cp.id])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.path().join("file")).unwrap(),
        "before"
    );
    fs::write(root.path().join("file"), "again").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .current_dir(root.path())
        .args(["undo", &run.run_id])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert_eq!(
        fs::read_to_string(root.path().join("file")).unwrap(),
        "before"
    );
    let created = run_task(&task("write:nested/new\ncreated"), root.path()).unwrap();
    let cp = workspace
        .list_checkpoints(&created.run_id)
        .unwrap()
        .remove(0);
    assert!(!cp.existed);
    let result = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .current_dir(root.path())
        .args(["undo", &created.run_id])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(!root.path().join("nested/new").exists());
    restore_checkpoint(root.path(), &cp).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn failed_restore_has_no_partial_write_and_does_not_follow_links() {
    use std::os::unix::fs::symlink;
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::write(root.path().join("file"), "before").unwrap();
    fs::write(outside.path().join("victim"), "OUTSIDE").unwrap();
    let run = run_task(&task("write:file\nafter"), root.path()).unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    let cp = workspace.list_checkpoints(&run.run_id).unwrap().remove(0);
    fs::remove_file(root.path().join("file")).unwrap();
    symlink(outside.path().join("victim"), root.path().join("file")).unwrap();
    assert!(restore_checkpoint(root.path(), &cp).is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("victim")).unwrap(),
        "OUTSIDE"
    );
    fs::remove_file(root.path().join("file")).unwrap();
    fs::write(root.path().join("file"), "UNCHANGED").unwrap();
    fs::remove_file(&cp.snapshot_path).unwrap();
    symlink(outside.path().join("victim"), &cp.snapshot_path).unwrap();
    assert!(restore_checkpoint(root.path(), &cp).is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("file")).unwrap(),
        "UNCHANGED"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn restore_does_not_modify_snapshot_or_an_outside_hardlink_alias() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::write(root.path().join("file"), "before").unwrap();
    let run = run_task(&task("write:file\nafter"), root.path()).unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    let cp = workspace.list_checkpoints(&run.run_id).unwrap().remove(0);
    fs::remove_file(root.path().join("file")).unwrap();
    fs::write(outside.path().join("alias"), "ALIAS").unwrap();
    fs::hard_link(outside.path().join("alias"), root.path().join("file")).unwrap();
    restore_checkpoint(root.path(), &cp).unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("file")).unwrap(),
        "before"
    );
    assert_eq!(
        fs::read_to_string(outside.path().join("alias")).unwrap(),
        "ALIAS"
    );
    assert_eq!(fs::read_to_string(&cp.snapshot_path).unwrap(), "before");
}

#[test]
fn restore_rejects_legacy_audit_paths() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("file"), "before").unwrap();
    let run = run_task(&task("write:file\nafter"), root.path()).unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    let mut cp = workspace.list_checkpoints(&run.run_id).unwrap().remove(0);
    cp.target_path = ".harness/forged".into();
    assert!(restore_checkpoint(root.path(), &cp).is_err());
    assert!(!root.path().join(".harness").exists());
}
