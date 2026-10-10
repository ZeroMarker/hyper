use harness::{
    AgentMode, StepSpec, TaskSpec, Workspace, get_run_details, restore_checkpoint, run_task,
    run_task_in_session, state,
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Barrier},
};
use tempfile::tempdir;

fn task(instruction: &str) -> TaskSpec {
    TaskSpec {
        id: None,
        name: "audit-boundary".into(),
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

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &path);
        } else {
            fs::copy(entry.path(), path).unwrap();
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "linux")]
#[test]
fn shell_cannot_write_delete_rename_or_link_protected_audit_records() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("file.txt"), "original").unwrap();
    let initial = run_task_in_session(
        &task("write:file.txt\nchanged"),
        root.path(),
        "conversation",
    )
    .unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    assert!(!workspace.paths.dir.starts_with(root.path()));
    let run = workspace.paths.runs.join(&initial.run_id);
    let checkpoint = workspace
        .list_checkpoints(&initial.run_id)
        .unwrap()
        .remove(0);
    let records = vec![
        run.join("events.jsonl"),
        run.join("task.json"),
        run.join("summary.json"),
        checkpoint.snapshot_path.clone(),
        workspace.session_path("conversation"),
    ];
    let expected: Vec<_> = records.iter().map(|p| fs::read(p).unwrap()).collect();
    let mut targets = records.clone();
    targets.push(workspace.paths.db.clone());
    let config = serde_json::to_string(&targets).unwrap();
    let script = r#"import errno,json,os,pathlib,sys
success=[]
for s in json.loads(sys.argv[1]):
 p=pathlib.Path(s)
 for name,action in [('write',lambda:p.write_bytes(b'FORGED')),('delete',p.unlink),('rename',lambda:p.rename('stolen')),('link',lambda:os.link(p,'alias'))]:
  try:
   action();success.append(name)
  except OSError as e:
   assert e.errno in (errno.EACCES,errno.EPERM,errno.EXDEV),e
print('unexpected='+str(success))
sys.exit(bool(success))"#;
    let instruction = format!(
        "bash:python3 -c {} {}",
        shell_quote(script),
        shell_quote(&config)
    );
    let attack = run_task(&task(&instruction), root.path()).unwrap();
    assert_eq!(attack.status, "finished", "{:?}", attack.failure);
    let (_, events) = get_run_details(root.path(), &attack.run_id).unwrap();
    let output = events
        .iter()
        .find(|e| e.event_type == "tool.finished")
        .unwrap();
    assert!(
        output.payload["stdout"]
            .as_str()
            .unwrap()
            .contains("unexpected=[]")
    );
    assert!(events.iter().any(|e| e.event_type == "tool.started"));
    assert!(!events.iter().any(|e| e.event_type == "tool.denied"));
    for (path, expected) in records.iter().zip(expected) {
        assert_eq!(fs::read(path).unwrap(), expected);
    }
    assert_eq!(
        workspace.get_run(&initial.run_id).unwrap().unwrap().status,
        "finished"
    );
    assert_eq!(
        workspace
            .db
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert!(!root.path().join("stolen").exists());
    assert!(!root.path().join("alias").exists());
}

#[test]
fn retired_harness_directory_and_locator_are_never_authoritative() {
    let root = tempdir().unwrap();
    let first = run_task(&task("write:ok.txt\nok"), root.path()).unwrap();
    let actual = Workspace::open(root.path()).unwrap().paths.dir;
    fs::create_dir_all(root.path().join(".harness/runs/fake")).unwrap();
    fs::write(
        root.path().join(".harness/runs/fake/events.jsonl"),
        "FORGED\n",
    )
    .unwrap();
    fs::write(
        root.path().join(".harness/workspace.json"),
        r#"{"storageDir":"/tmp/forged"}"#,
    )
    .unwrap();
    let reopened = Workspace::open(root.path()).unwrap();
    assert_eq!(reopened.paths.dir, actual);
    assert_eq!(reopened.list_runs(100).unwrap().len(), 1);
    assert_eq!(
        reopened.get_run(&first.run_id).unwrap().unwrap().status,
        "finished"
    );
    let later = run_task(&task("write:later.txt\nok"), root.path()).unwrap();
    assert_eq!(later.status, "finished");
    assert_eq!(
        Workspace::open(root.path())
            .unwrap()
            .list_runs(100)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn concurrent_initialization_and_runs_share_one_authoritative_store() {
    let root = tempdir().unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|i| {
            let barrier = barrier.clone();
            let path = root.path().to_owned();
            std::thread::spawn(move || {
                barrier.wait();
                run_task(&task(&format!("write:file-{i}.txt\nok")), path).unwrap()
            })
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap().status, "finished");
    }
    let workspace = Workspace::open(root.path()).unwrap();
    let runs = workspace.list_runs(100).unwrap();
    assert_eq!(runs.len(), 8);
    assert!(runs.iter().all(|r| r.status == "finished"));
    for run in runs {
        let (_, events) = get_run_details(root.path(), &run.run_id).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "run.finished")
                .count(),
            1
        );
    }
}

#[test]
fn explicit_legacy_import_retains_source_and_rebinds_absolute_checkpoints() {
    let origin = tempdir().unwrap();
    let moved = tempdir().unwrap();
    fs::write(origin.path().join("file.txt"), "before").unwrap();
    let initial = run_task_in_session(
        &task("write:file.txt\nafter"),
        origin.path(),
        "conversation",
    )
    .unwrap();
    let old = Workspace::open(origin.path()).unwrap().paths.dir;
    let legacy = moved.path().join(".harness");
    copy_tree(&old, &legacy);
    fs::remove_file(legacy.join("workspace.json")).unwrap();
    fs::write(moved.path().join("file.txt"), "after").unwrap();
    let cp_dir = legacy
        .join("runs")
        .join(&initial.run_id)
        .join("checkpoints");
    let cp_path = fs::read_dir(&cp_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let mut cp: harness::Checkpoint = serde_json::from_slice(&fs::read(&cp_path).unwrap()).unwrap();
    cp.target_path = origin
        .path()
        .join("file.txt")
        .to_string_lossy()
        .into_owned();
    cp.snapshot_path = origin
        .path()
        .join(".harness/runs")
        .join(&initial.run_id)
        .join("checkpoints")
        .join(format!("{}.snapshot", cp.id));
    fs::write(&cp_path, serde_json::to_vec(&cp).unwrap()).unwrap();
    let original = fs::read(&cp_path).unwrap();
    assert!(
        Workspace::open(moved.path())
            .unwrap_err_string()
            .contains("legacy .harness")
    );
    let imported = state::migrate(moved.path(), &legacy).unwrap();
    assert_eq!(fs::read(&cp_path).unwrap(), original);
    let workspace = Workspace::open(moved.path()).unwrap();
    assert_eq!(workspace.paths.dir, imported);
    assert_eq!(workspace.session_messages("conversation").unwrap().len(), 2);
    let cp = workspace
        .list_checkpoints(&initial.run_id)
        .unwrap()
        .remove(0);
    assert_eq!(cp.target_path, "file.txt");
    assert!(cp.snapshot_path.starts_with(&imported));
    restore_checkpoint(moved.path(), &cp).unwrap();
    assert_eq!(
        fs::read_to_string(moved.path().join("file.txt")).unwrap(),
        "before"
    );
    assert_eq!(
        fs::read_to_string(origin.path().join("file.txt")).unwrap(),
        "after"
    );
    assert!(
        state::migrate(moved.path(), &legacy).is_err(),
        "import must never replace initialized history"
    );
}

trait ErrorString {
    fn unwrap_err_string(self) -> String;
}
impl<T> ErrorString for anyhow::Result<T> {
    fn unwrap_err_string(self) -> String {
        match self {
            Ok(_) => panic!("expected error"),
            Err(e) => e.to_string(),
        }
    }
}

#[test]
fn active_source_runs_block_import_and_wal_backup_preserves_committed_rows() {
    let origin = tempdir().unwrap();
    let moved = tempdir().unwrap();
    let workspace = Workspace::open(origin.path()).unwrap();
    workspace.prepare_run("active").unwrap();
    let guard = workspace.lock_run("active").unwrap();
    assert!(
        state::migrate(moved.path(), &workspace.paths.dir)
            .unwrap_err()
            .to_string()
            .contains("source run is active")
    );
    assert!(!state::directory(moved.path()).unwrap().exists());
    drop(guard);
    let source = workspace.paths.dir.clone();
    drop(workspace);
    let database = rusqlite::Connection::open(source.join("harness.db")).unwrap();
    database.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE import_probe(value TEXT); INSERT INTO import_probe VALUES ('from-wal');").unwrap();
    assert!(source.join("harness.db-wal").exists());
    state::migrate(moved.path(), &source).unwrap();
    let reopened = Workspace::open(moved.path()).unwrap();
    assert_eq!(
        reopened
            .db
            .query_row("SELECT value FROM import_probe", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "from-wal"
    );
}

#[cfg(unix)]
#[test]
fn failed_import_never_commits_and_fresh_inodes_remove_legacy_hardlink_aliases() {
    use std::os::unix::fs::{MetadataExt, symlink};
    let origin = tempdir().unwrap();
    let moved = tempdir().unwrap();
    let initial = run_task(&task("write:file.txt\nafter"), origin.path()).unwrap();
    let source = Workspace::open(origin.path()).unwrap().paths.dir;
    let record = source.join("runs").join(&initial.run_id).join("task.json");
    let alias = origin.path().join("old-alias");
    fs::hard_link(&record, &alias).unwrap();
    let bad = source.join("unsafe-link");
    symlink(&alias, &bad).unwrap();
    assert!(
        state::migrate(moved.path(), &source)
            .unwrap_err()
            .to_string()
            .contains("refuses symlink")
    );
    assert!(!state::directory(moved.path()).unwrap().exists());
    fs::remove_file(&bad).unwrap();
    let imported = state::migrate(moved.path(), &source).unwrap();
    let new = imported
        .join("runs")
        .join(&initial.run_id)
        .join("task.json");
    assert_eq!(fs::metadata(&new).unwrap().nlink(), 1);
    assert_ne!(
        fs::metadata(&new).unwrap().ino(),
        fs::metadata(&alias).unwrap().ino()
    );
    let expected = fs::read(&new).unwrap();
    fs::write(alias, "FORGED").unwrap();
    assert_eq!(fs::read(new).unwrap(), expected);
}

#[cfg(unix)]
#[test]
fn prepared_hardlinks_to_external_state_fail_closed_before_shell_execution() {
    let root = tempdir().unwrap();
    let initial = run_task(&task("write:file.txt\nafter"), root.path()).unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    let record = workspace.paths.runs.join(&initial.run_id).join("task.json");
    fs::hard_link(&record, root.path().join("alias")).unwrap();
    let attack = run_task(&task("bash:echo ran > spawned"), root.path()).unwrap();
    assert_eq!(attack.status, "failed");
    assert_eq!(attack.failure.unwrap().error_type, "PolicyError");
    assert!(!root.path().join("spawned").exists());
}

#[test]
fn cli_refuses_inside_workspace_state_and_uses_explicit_state_location() {
    let root = tempdir().unwrap();
    let external = tempdir().unwrap();
    let invoke = |path: &Path| {
        Command::new(env!("CARGO_BIN_EXE_hyper"))
            .env("HYPER_STATE_DIR", path)
            .args(["state"])
            .current_dir(root.path())
            .output()
            .unwrap()
    };
    let output = invoke(&root.path().join("inside"));
    assert_eq!(output.status.code(), Some(1));
    assert!(!root.path().join("inside").exists());
    let output = invoke(external.path());
    assert!(output.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let actual = PathBuf::from(metadata["storageDir"].as_str().unwrap());
    assert!(actual.starts_with(external.path()));
    assert!(!actual.starts_with(root.path()));
    fs::create_dir_all(root.path().join(".harness/runs/fake")).unwrap();
    assert!(invoke(external.path()).status.success());
}

#[cfg(target_os = "linux")]
#[test]
fn shell_does_not_inherit_audit_fds_or_reopen_parent_audit_fds() {
    let root = tempdir().unwrap();
    let workspace = Workspace::open(root.path()).unwrap();
    workspace.prepare_run("held").unwrap();
    let _run = workspace.lock_run("held").unwrap();
    let audit = workspace.paths.dir.to_string_lossy().into_owned();
    let parent_fds: Vec<_> = fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            fs::read_link(entry.path()).is_ok_and(|p| p.starts_with(&workspace.paths.dir))
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!parent_fds.is_empty());
    let config = serde_json::to_string(&(std::process::id(), parent_fds, audit)).unwrap();
    let script = r#"import json,os,sys
parent,fds,audit=json.loads(sys.argv[1])
for fd in os.listdir('/proc/self/fd'):
 try:
  target=os.readlink('/proc/self/fd/'+fd)
 except FileNotFoundError:
  continue
 assert not target.startswith(audit),target
for fd in fds:
 try:
  opened=os.open('/proc/'+str(parent)+'/fd/'+fd,os.O_WRONLY)
 except PermissionError:
  continue
 os.close(opened);raise AssertionError('parent audit fd was accessible')
print('audit-fds-blocked')"#;
    let summary = run_task(
        &task(&format!(
            "bash:python3 -c {} {}",
            shell_quote(script),
            shell_quote(&config)
        )),
        root.path(),
    )
    .unwrap();
    assert_eq!(summary.status, "finished", "{:?}", summary.failure);
    let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
    assert!(events.iter().any(|e| {
        e.event_type == "tool.finished"
            && e.payload["stdout"]
                .as_str()
                .is_some_and(|s| s.contains("audit-fds-blocked"))
    }));
}

#[cfg(unix)]
#[test]
fn a_symlinked_state_registry_fails_closed() {
    use std::os::unix::fs::symlink;
    let root = tempdir().unwrap();
    let external = tempdir().unwrap();
    let redirected = root.path().join("redirected");
    fs::create_dir(&redirected).unwrap();
    symlink(&redirected, external.path().join("workspaces")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .env("HYPER_STATE_DIR", external.path())
        .args(["state"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read_dir(redirected).unwrap().count(), 0);
}

#[test]
fn imported_model_history_replays_exact_requests_and_session_can_continue() {
    use std::io::{BufRead, Read, Write};
    let origin = tempdir().unwrap();
    let moved = tempdir().unwrap();
    fs::write(origin.path().join("README.md"), "Replay fixture\n").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let provider = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
        let mut request = vec![0; length];
        reader.read_exact(&mut request).unwrap();
        let response = r#"{"model":"stub","choices":[{"message":{"content":"inspected"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
        serde_json::from_slice::<serde_json::Value>(&request).unwrap()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .env("DEEPSEEK_API_KEY", "stub")
        .env("DEEPSEEK_BASE_URL", format!("http://{address}/v1"))
        .env("DEEPSEEK_MODEL", "stub")
        .env("DEEPSEEK_PROTOCOL", "chat")
        .env_remove("HYPER_CONTEXT_TOKENS")
        .env_remove("HYPER_HISTORY_TOKENS")
        .env_remove("HYPER_OUTPUT_TOKENS")
        .args(["--jsonl", "plan", "--session", "conversation", "inspect"])
        .current_dir(origin.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = provider.join().unwrap();
    let first: serde_json::Value = serde_json::from_str(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let id = first["runId"].as_str().unwrap();
    let expected = harness::replay_messages(origin.path(), id).unwrap();
    assert_eq!(
        serde_json::to_value(&expected[0].messages).unwrap(),
        request["messages"]
    );
    let source = Workspace::open(origin.path()).unwrap().paths.dir;
    state::migrate(moved.path(), &source).unwrap();
    let actual = harness::replay_messages(moved.path(), id).unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let resumed = run_task_in_session(
        &task("write:continued.txt\nok"),
        moved.path(),
        "conversation",
    )
    .unwrap();
    assert_eq!(resumed.status, "finished");
    assert_eq!(
        Workspace::open(moved.path())
            .unwrap()
            .session_messages("conversation")
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn malformed_import_rolls_back_and_transcript_registry_is_rebuilt() {
    let origin = tempdir().unwrap();
    let moved = tempdir().unwrap();
    run_task_in_session(
        &task("write:file.txt\nafter"),
        origin.path(),
        "conversation",
    )
    .unwrap();
    let source = Workspace::open(origin.path()).unwrap().paths.dir;
    let transcript = source.join("sessions/conversation.jsonl");
    let original = fs::read(&transcript).unwrap();
    fs::write(&transcript, b"{invalid}\n").unwrap();
    assert!(state::migrate(moved.path(), &source).is_err());
    assert!(!state::directory(moved.path()).unwrap().exists());
    fs::write(&transcript, original).unwrap();
    // Deliberately remove the derived registry row while keeping its facts.
    let database = rusqlite::Connection::open(source.join("harness.db")).unwrap();
    database.execute("DELETE FROM sessions", []).unwrap();
    drop(database);
    state::migrate(moved.path(), &source).unwrap();
    let restored = Workspace::open(moved.path())
        .unwrap()
        .session("conversation")
        .unwrap()
        .unwrap();
    assert_eq!(restored.messages, 2);
    assert_eq!(restored.runs, 1);
}

#[test]
fn concurrent_imports_commit_once_without_replacing_history() {
    let origin = tempdir().unwrap();
    let moved = tempdir().unwrap();
    let initial = run_task(&task("write:file.txt\nafter"), origin.path()).unwrap();
    let source = Workspace::open(origin.path()).unwrap().paths.dir;
    let barrier = Arc::new(Barrier::new(4));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let source = source.clone();
            let root = moved.path().to_owned();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                state::migrate(&root, &source)
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .map(|w| w.join().unwrap())
            .filter(Result::is_ok)
            .count(),
        1
    );
    let workspace = Workspace::open(moved.path()).unwrap();
    assert_eq!(workspace.list_runs(100).unwrap().len(), 1);
    assert_eq!(
        workspace.get_run(&initial.run_id).unwrap().unwrap().status,
        "finished"
    );
}

#[test]
fn migrated_crash_tail_is_preserved_and_interrupted_event_remains_valid_jsonl() {
    let origin = tempdir().unwrap();
    let moved = tempdir().unwrap();
    let workspace = Workspace::open(origin.path()).unwrap();
    let run = workspace.prepare_run("crashed").unwrap();
    let task = task("write:file.txt\nafter");
    fs::write(&run.task, serde_json::to_vec(&task).unwrap()).unwrap();
    workspace
        .create_run("crashed", &task, &harness::workspace::now())
        .unwrap();
    let event = harness::HarnessEvent {
        event_id: "started".into(),
        run_id: "crashed".into(),
        task_id: task.task_id().into(),
        event_type: "run.started".into(),
        timestamp: harness::workspace::now(),
        step_id: None,
        step_index: None,
        payload: serde_json::json!({"taskName":"audit-boundary"}),
    };
    workspace.insert_event(&event).unwrap();
    let tail = b"{\"type\":\"model.delta\",\"payload\":";
    let mut original = serde_json::to_vec(&event).unwrap();
    original.push(b'\n');
    original.extend_from_slice(tail);
    fs::write(&run.events, &original).unwrap();
    let source = workspace.paths.dir.clone();
    drop(workspace);
    state::migrate(moved.path(), &source).unwrap();
    assert_eq!(fs::read(&run.events).unwrap(), original);
    let reopened = Workspace::open(moved.path()).unwrap();
    assert_eq!(
        reopened.get_run("crashed").unwrap().unwrap().status,
        "interrupted"
    );
    let path = reopened.paths.runs.join("crashed");
    let events: Vec<serde_json::Value> = fs::read_to_string(path.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "run.interrupted")
            .count(),
        1
    );
    assert!(
        fs::read_dir(path.join("artifacts"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with("import-partial-event-")
                && fs::read(e.path()).unwrap() == tail)
    );
}

#[test]
fn imported_database_path_identifiers_cannot_escape_the_store() {
    let origin = tempdir().unwrap();
    let moved = tempdir().unwrap();
    let source = Workspace::open(origin.path()).unwrap().paths.dir;
    let database = rusqlite::Connection::open(source.join("harness.db")).unwrap();
    database
        .execute(
            "INSERT INTO runs VALUES('../escape','task','forged','running','now',NULL)",
            [],
        )
        .unwrap();
    drop(database);
    let error = state::migrate(moved.path(), &source).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid imported database path identifier")
    );
    assert!(!state::directory(moved.path()).unwrap().exists());
}

#[cfg(unix)]
#[test]
fn a_different_directory_at_the_same_path_cannot_reuse_audit_state() {
    let parent = tempdir().unwrap();
    let root = parent.path().join("project");
    fs::create_dir(&root).unwrap();
    let initial = run_task(&task("write:a.txt\nok"), &root).unwrap();
    assert_eq!(initial.status, "finished");
    let store = Workspace::open(&root).unwrap().paths.dir;
    // The path-derived key is unchanged after a move, so only the recorded root
    // inode can distinguish the real checkout from a different directory that
    // now occupies the same canonical path.
    fs::rename(&root, parent.path().join("moved-away")).unwrap();
    fs::create_dir(&root).unwrap();
    let error = Workspace::open(&root).unwrap_err_string();
    assert!(error.contains("workspace root inode"), "{error}");
    assert_eq!(state::directory(&root).unwrap(), store);
}

#[cfg(unix)]
#[test]
fn legacy_identity_without_an_inode_anchor_still_opens() {
    let root = tempdir().unwrap();
    run_task(&task("write:a.txt\nok"), root.path()).unwrap();
    let store = Workspace::open(root.path()).unwrap().paths.dir;
    let path = store.join("workspace.json");
    let mut identity: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let object = identity.as_object_mut().unwrap();
    assert!(object.remove("workspaceDev").is_some());
    assert!(object.remove("workspaceIno").is_some());
    fs::write(&path, serde_json::to_vec(&identity).unwrap()).unwrap();
    let reopened = Workspace::open(root.path()).unwrap();
    assert_eq!(reopened.paths.dir, store);
    assert_eq!(reopened.list_runs(100).unwrap().len(), 1);
}

#[test]
fn import_rejects_sqlite_triggers_before_registry_repairs() {
    let origin = tempdir().unwrap();
    let moved = tempdir().unwrap();
    run_task_in_session(
        &task("write:file.txt\nafter"),
        origin.path(),
        "conversation",
    )
    .unwrap();
    let source = Workspace::open(origin.path()).unwrap().paths.dir;
    let database = rusqlite::Connection::open(source.join("harness.db")).unwrap();
    database.execute_batch("CREATE TRIGGER forged_history AFTER UPDATE ON sessions BEGIN INSERT INTO runs VALUES('../escape','task','forged','running','now',NULL); END;").unwrap();
    drop(database);
    let error = state::migrate(moved.path(), &source).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported executable schema objects")
    );
    assert!(!state::directory(moved.path()).unwrap().exists());
}
