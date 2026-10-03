#![cfg(unix)]

use harness::{
    AgentMode, ApprovalGate, CancellationToken, RunOptions, StepSpec, TaskSpec, Workspace,
    get_run_details, replay_messages, run_task_with_control,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use tempfile::tempdir;

fn task(instructions: &[&str]) -> TaskSpec {
    TaskSpec {
        id: None,
        name: "cancel fixture".into(),
        metadata: HashMap::new(),
        steps: instructions
            .iter()
            .enumerate()
            .map(|(i, instruction)| StepSpec {
                id: format!("s{i}"),
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

fn wait_for(mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "condition did not arrive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn assert_cancelled(root: &Path, id: &str) {
    let (row, events) = get_run_details(root, id).unwrap();
    assert_eq!(row.unwrap().status, "cancelled");
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "run.cancelled")
            .count(),
        1
    );
    assert!(!events.iter().any(|e| matches!(
        e.event_type.as_str(),
        "run.failed" | "run.finished" | "run.interrupted"
    )));
    let summary: Value = serde_json::from_slice(
        &fs::read(root.join(".harness/runs").join(id).join("summary.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(summary["status"], "cancelled");
    assert_eq!(summary["stepsFailed"], 0);
    assert_eq!(summary["failure"]["errorType"], "Cancelled");
    let messages = Workspace::open(root)
        .unwrap()
        .session_messages("cancel-session")
        .unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert!(messages[1].content.contains("cancelled"));
    // Repeated opens must never turn a deliberately cancelled run into a crash.
    assert!(
        Workspace::open(root)
            .unwrap()
            .reconcile_stale_runs()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cancellation_releases_pending_approval_without_executing_it() {
    let root = tempdir().unwrap();
    let path = root.path().to_owned();
    let gate = ApprovalGate::new();
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let worker_gate = gate.clone();
    let worker = std::thread::spawn(move || {
        run_task_with_control(
            &task(&["write:out.txt\nwrong", "write:later.txt\nwrong"]),
            &path,
            RunOptions {
                cancellation: token,
                gate: Some(worker_gate),
                session_id: Some("cancel-session".into()),
                ..RunOptions::default()
            },
        )
        .unwrap()
    });
    let mut requests = Vec::new();
    wait_for(|| {
        requests.extend(gate.drain());
        !requests.is_empty()
    });
    let started = Instant::now();
    cancellation.cancel();
    let summary = worker.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_cancelled(root.path(), &summary.run_id);
    assert!(!root.path().join("out.txt").exists());
    assert!(!root.path().join("later.txt").exists());
    assert!(requests.remove(0).response.send(true).is_err());
    let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
    let approval = events
        .iter()
        .find(|e| e.event_type == "tool.approval")
        .unwrap();
    assert_eq!(approval.payload["cancelled"], true);
    assert!(approval.payload["approved"].is_null());
    assert!(!events.iter().any(|e| e.event_type == "tool.denied"));
}

#[test]
fn cancellation_kills_shell_group_keeps_completed_changes_and_stops_later_steps() {
    let root = tempdir().unwrap();
    let path = root.path().to_owned();
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let worker = std::thread::spawn(move || {
        run_task_with_control(
            &task(&[
                "write:kept.txt\nkeep me",
                "bash:sleep 30 & child=$!; echo $child > child.pid; echo partial-output; wait",
                "write:later.txt\nwrong",
            ]),
            &path,
            RunOptions {
                cancellation: token,
                permissions: harness::ToolPermissions::with_mutations(harness::Permission::Allow),
                session_id: Some("cancel-session".into()),
                ..RunOptions::default()
            },
        )
        .unwrap()
    });
    wait_for(|| root.path().join("child.pid").exists());
    let child_pid: i32 = fs::read_to_string(root.path().join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let started = Instant::now();
    cancellation.cancel();
    let summary = worker.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_cancelled(root.path(), &summary.run_id);
    assert_eq!(summary.steps_succeeded, 1);
    assert_eq!(
        fs::read_to_string(root.path().join("kept.txt")).unwrap(),
        "keep me"
    );
    assert!(!root.path().join("later.txt").exists());
    wait_for(|| {
        #[cfg(target_os = "linux")]
        if let Ok(stat) = fs::read_to_string(format!("/proc/{child_pid}/stat")) {
            return stat
                .rsplit(')')
                .next()
                .unwrap()
                .trim_start()
                .starts_with('Z');
        }
        unsafe { libc::kill(child_pid, 0) != 0 }
    });
    let (_, events) = get_run_details(root.path(), &summary.run_id).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "tool.cancelled" && e.payload["cancelled"] == true)
    );
    assert_eq!(
        Workspace::open(root.path())
            .unwrap()
            .list_checkpoints(&summary.run_id)
            .unwrap()
            .len(),
        1
    );
    // A fresh token can continue the same conversation after cancellation.
    let resumed = run_task_with_control(
        &task(&["write:resumed.txt\nok"]),
        root.path(),
        RunOptions {
            session_id: Some("cancel-session".into()),
            permissions: harness::ToolPermissions::with_mutations(harness::Permission::Allow),
            ..RunOptions::default()
        },
    )
    .unwrap();
    assert_eq!(resumed.status, "finished");
    assert_eq!(
        Workspace::open(root.path())
            .unwrap()
            .session_messages("cancel-session")
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn a_pre_cancelled_run_never_starts_a_tool() {
    let root = tempdir().unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let summary = run_task_with_control(
        &task(&["write:out.txt\nwrong"]),
        root.path(),
        RunOptions {
            cancellation,
            session_id: Some("cancel-session".into()),
            ..RunOptions::default()
        },
    )
    .unwrap();
    assert_cancelled(root.path(), &summary.run_id);
    assert!(!root.path().join("out.txt").exists());
    assert_eq!(summary.steps_succeeded, 0);
}

fn request(stream: &mut TcpStream) -> Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
        let text = String::from_utf8_lossy(&bytes);
        if let Some((headers, body)) = text.split_once("\r\n\r\n") {
            let length: usize = headers
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap())
                })
                .unwrap();
            if body.len() >= length {
                return serde_json::from_slice(&bytes[bytes.len() - body.len()..][..length])
                    .unwrap();
            }
        }
    }
}

fn cli(
    root: &Path,
    url: &str,
    protocol: &str,
) -> (
    Child,
    mpsc::Receiver<Value>,
    std::thread::JoinHandle<Vec<Value>>,
) {
    fs::write(root.join("README.md"), "Cancellation fixture\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .env("HYPER_APPROVAL", "allow")
        .current_dir(root)
        .args([
            "--jsonl",
            "build",
            "--session",
            "cancel-session",
            "fix this",
        ])
        .env("DEEPSEEK_API_KEY", "stub")
        .env("DEEPSEEK_MODEL", "stub")
        .env("DEEPSEEK_BASE_URL", url)
        .env("DEEPSEEK_PROTOCOL", protocol)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("HYPER_CONTEXT_TOKENS", "128000")
        .env("HYPER_HISTORY_TOKENS", "16000")
        .env("HYPER_OUTPUT_TOKENS", "8192")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        BufReader::new(stdout)
            .lines()
            .map(|line| {
                let event: Value = serde_json::from_str(&line.unwrap()).unwrap();
                let _ = tx.send(event.clone());
                event
            })
            .collect()
    });
    (child, rx, reader)
}

fn signal_and_check(
    mut child: Child,
    root: &Path,
    reader: std::thread::JoinHandle<Vec<Value>>,
    signal: i32,
) -> String {
    let started = Instant::now();
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
    wait_for(|| child.try_wait().unwrap().is_some());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(child.wait().unwrap().code(), Some(130));
    let events = reader.join().unwrap();
    let id = events[0]["runId"].as_str().unwrap().to_owned();
    assert_cancelled(root, &id);
    let (_, stored) = get_run_details(root, &id).unwrap();
    assert_eq!(
        events,
        stored
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect::<Vec<_>>()
    );
    id
}

#[test]
fn cli_sigint_cancels_waiting_for_response_headers() {
    let root = tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (ready_tx, ready_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let body = request(&mut stream);
        ready_tx.send(body).unwrap();
        let mut byte = [0u8];
        assert_eq!(
            stream.read(&mut byte).unwrap(),
            0,
            "request socket closed on cancellation"
        );
    });
    let (child, _, reader) = cli(root.path(), &url, "chat");
    let body = ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let id = signal_and_check(child, root.path(), reader, libc::SIGINT);
    assert_eq!(
        replay_messages(root.path(), &id).unwrap()[0].messages,
        *body["messages"].as_array().unwrap()
    );
    server.join().unwrap();
}

#[test]
fn cli_sigterm_cancels_sse_for_all_protocols_without_executing_partial_tools() {
    let frames = [
        (
            "chat",
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\",\"tool_calls\":[{\"index\":0,\"id\":\"write-1\",\"function\":{\"name\":\"write\",\"arguments\":\"{\\\"path\\\":\\\"out.txt\\\",\\\"content\\\":\"}}]}}]}\n\n",
        ),
        (
            "responses",
            "event: response.output_text.delta\ndata: {\"delta\":\"partial\"}\n\n",
        ),
        (
            "messages",
            "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"partial\"}}\n\n",
        ),
    ];
    for (protocol, frame) in frames {
        let root = tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            request(&mut stream);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 100000\r\n\r\n{frame}").unwrap();
            stream.flush().unwrap();
            let mut byte = [0u8];
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
        });
        let (child, rx, reader) = cli(root.path(), &url, protocol);
        loop {
            let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if event["type"] == "model.delta" {
                break;
            }
        }
        let id = signal_and_check(child, root.path(), reader, libc::SIGTERM);
        let (_, events) = get_run_details(root.path(), &id).unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.event_type == "model.delta" && e.payload["content"] == "partial")
        );
        assert!(!events.iter().any(|e| e.event_type == "tool.started"));
        assert!(!root.path().join("out.txt").exists());
        server.join().unwrap();
    }
}

#[test]
fn cli_cancels_retry_after_and_json_fallback_body_waits() {
    for phase in ["retry", "json"] {
        let root = tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            request(&mut stream);
            if phase == "retry" {
                write!(stream, "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 4\r\nRetry-After: 10\r\n\r\nwait").unwrap();
            } else {
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10000\r\n\r\n{{\"model\":").unwrap();
            }
            stream.flush().unwrap();
            ready_tx.send(()).unwrap();
            let mut byte = [0u8];
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
            listener.set_nonblocking(true).unwrap();
            assert!(listener.accept().is_err(), "no retry request after cancel");
        });
        let (child, _, reader) = cli(root.path(), &url, "chat");
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Let the already delivered status/body reach the transport's wait.
        std::thread::sleep(Duration::from_millis(100));
        signal_and_check(child, root.path(), reader, libc::SIGINT);
        server.join().unwrap();
    }
}

#[test]
fn cancelled_terminal_state_survives_a_broken_output_sink() {
    struct BrokenOnStep {
        bytes: Vec<u8>,
        token: CancellationToken,
    }
    impl Write for BrokenOnStep {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            let event: Value = serde_json::from_slice(&self.bytes).unwrap();
            self.bytes.clear();
            if event["type"] == "step.started" || self.token.is_cancelled() {
                self.token.cancel();
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "closed consumer",
                ))
            } else {
                Ok(())
            }
        }
    }
    let root = tempdir().unwrap();
    let token = CancellationToken::new();
    let summary = run_task_with_control(
        &task(&["write:out.txt\nwrong"]),
        root.path(),
        RunOptions {
            cancellation: token.clone(),
            session_id: Some("cancel-session".into()),
            sink: Some(harness::EventSink::jsonl(BrokenOnStep {
                bytes: Vec::new(),
                token,
            })),
            ..RunOptions::default()
        },
    )
    .unwrap();
    assert_cancelled(root.path(), &summary.run_id);
    assert!(!root.path().join("out.txt").exists());
}

#[test]
fn cli_cancels_while_jsonl_consumer_stops_reading() {
    let root = tempdir().unwrap();
    let giant = format!("write:later.txt\n{}", "x".repeat(512 * 1024));
    let specification = task(&["write:kept.txt\nkeep me", &giant]);
    let file = root.path().join("task.json");
    fs::write(&file, serde_json::to_vec(&specification).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hyper"))
        .env("HYPER_APPROVAL", "allow")
        .current_dir(root.path())
        .args(["--jsonl", "run"])
        .arg(&file)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    // Hold the read descriptor open but never drain it. The large step.started
    // event is persisted before publication, making this a deterministic gate.
    let _unread_stdout = child.stdout.take().unwrap();
    let mut event_file = None;
    wait_for(|| {
        event_file = fs::read_dir(root.path().join(".harness/runs"))
            .ok()
            .and_then(|mut dirs| dirs.next())
            .and_then(Result::ok)
            .map(|entry| entry.path().join("events.jsonl"));
        event_file
            .as_ref()
            .is_some_and(|p| p.metadata().is_ok_and(|m| m.len() > 100000))
    });
    let started = Instant::now();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    wait_for(|| child.try_wait().unwrap().is_some());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(child.wait().unwrap().code(), Some(130));
    let run = Workspace::open(root.path())
        .unwrap()
        .list_runs(1)
        .unwrap()
        .remove(0);
    assert_eq!(run.status, "cancelled");
    let summary: Value = serde_json::from_slice(
        &fs::read(event_file.unwrap().parent().unwrap().join("summary.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(summary["status"], "cancelled");
    assert_eq!(
        fs::read_to_string(root.path().join("kept.txt")).unwrap(),
        "keep me"
    );
    assert!(!root.path().join("later.txt").exists());
}

#[test]
fn cancellation_during_agent_tool_batch_keeps_edit_and_replays_only_sent_request() {
    let root = tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let sent = request(&mut stream);
        let call = |id, name, args: Value| serde_json::json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}});
        let body = serde_json::json!({"model":"stub","choices":[{"message":{"content":null,"tool_calls":[
            call("keep", "write", serde_json::json!({"path":"kept.txt","content":"keep me"})),
            call("wait", "bash", serde_json::json!({"command":"sleep 30 & child=$!; echo $child > child.pid; echo partial; wait"})),
            call("later", "write", serde_json::json!({"path":"later.txt","content":"wrong"})),
        ]}}]}).to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        sent
    });
    let (child, _, reader) = cli(root.path(), &url, "chat");
    wait_for(|| root.path().join("child.pid").exists());
    let id = signal_and_check(child, root.path(), reader, libc::SIGINT);
    assert_eq!(
        fs::read_to_string(root.path().join("kept.txt")).unwrap(),
        "keep me"
    );
    assert!(!root.path().join("later.txt").exists());
    let sent = server.join().unwrap();
    assert_eq!(
        replay_messages(root.path(), &id).unwrap()[0].messages,
        *sent["messages"].as_array().unwrap()
    );
    let (_, events) = get_run_details(root.path(), &id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "model.context_budget")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "model.observation")
            .count(),
        2
    );
}
