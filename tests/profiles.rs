#![cfg(target_os = "linux")]
use harness::{AgentMode, TaskSpec, Workspace, prompt_to_task};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use tempfile::tempdir;

fn command(root: &Path, config_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ha"));
    command
        .current_dir(root)
        .env("XDG_CONFIG_HOME", config_home)
        .env("HYPER_APPROVAL", "allow")
        .env("HYPER_AUTOCOMPACT", "0")
        .env("HYPER_LOOP_COMPACT", "0")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("DEEPSEEK_BASE_URL")
        .env_remove("DEEPSEEK_MODEL")
        .env_remove("DEEPSEEK_PROTOCOL");
    command
}

fn save(path: &Path, base: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, json!({"deepseek_api_key":"legacy-secret","active_profile":"local",
        "profiles":{
            "local":{"protocol":"chat","base_url":base,"model":"stub",
                "auth":{"source":"env","variable":"STUB_PROFILE_KEY"},"endpoints":{"chat":"/custom/chat"}},
            "other":{"protocol":"messages","base_url":"http://127.0.0.1:1","model":"other",
                "auth":{"source":"config","api_key":"other-secret"}}
        }}).to_string()).unwrap();
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0);
        request.extend_from_slice(&buffer[..n]);
        let text = String::from_utf8_lossy(&request);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|s| s.trim().parse().unwrap())
                })
                .unwrap();
            if body.len() >= length {
                return text.into_owned();
            }
        }
    }
}

fn accept(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => panic!("stub accept failed: {error}"),
        }
    }
}

fn respond(stream: &mut TcpStream, status: u16, body: &Value) {
    let body = body.to_string();
    write!(stream, "HTTP/1.1 {status} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}

#[test]
fn profile_list_and_use_preserve_configuration_without_loading_or_printing_keys() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let path = config_home.join("hyper/config.json");
    save(&path, "http://127.0.0.1:1");
    let output = command(dir.path(), &config_home)
        .args(["profile", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("legacy-secret") && !text.contains("other-secret"));
    let listed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(listed["activeProfile"], "local");
    assert_eq!(listed["profiles"][0]["authSource"], "env:STUB_PROFILE_KEY");
    let missing = command(dir.path(), &config_home)
        .env_remove("STUB_PROFILE_KEY")
        .args(["profile", "test"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("authentication"));
    let output = command(dir.path(), &config_home)
        .args(["profile", "use", "other"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let selected: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(selected["active_profile"], "other");
    assert_eq!(selected["deepseek_api_key"], "legacy-secret");
    let before = fs::read(&path).unwrap();
    assert!(
        !command(dir.path(), &config_home)
            .args(["profile", "use", "absent"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn explicit_profile_test_does_not_switch_and_sends_the_named_configuration() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let path = config_home.join("hyper/config.json");
    save(&path, &format!("http://{}", listener.local_addr().unwrap()));
    command(dir.path(), &config_home)
        .args(["profile", "use", "other"])
        .output()
        .unwrap();
    let before = fs::read(&path).unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = accept(&listener);
        let request = read_request(&mut stream);
        respond(
            &mut stream,
            200,
            &json!({"model":"stub","choices":[{"message":{"content":"OK"}}]}),
        );
        request
    });
    let output = command(dir.path(), &config_home)
        .env("STUB_PROFILE_KEY", "probe-secret")
        .args(["profile", "test", "local"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["config"]["profile"], "local");
    assert_eq!(result["config"]["authSource"], "env:STUB_PROFILE_KEY");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("probe-secret"));
    assert_eq!(fs::read(path).unwrap(), before);
    let request = server.join().unwrap();
    assert!(request.starts_with("POST /custom/chat HTTP/1.1"));
    assert!(request.contains("authorization: Bearer probe-secret"));
}

#[test]
fn run_freezes_profile_across_steps_and_records_only_safe_effective_settings() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let path = config_home.join("hyper/config.json");
    save(&path, &base);
    let mut task: TaskSpec = prompt_to_task("summarize", AgentMode::Plan);
    let mut second = task.steps[0].clone();
    second.id = "second".into();
    task.steps.push(second);
    fs::write(
        dir.path().join("task.json"),
        serde_json::to_vec(&task).unwrap(),
    )
    .unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for index in 0..2 {
            let mut stream = accept(&listener);
            requests.push(read_request(&mut stream));
            if index == 0 {
                let mut changed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                changed["active_profile"] = json!("other");
                fs::write(&path, changed.to_string()).unwrap();
            }
            respond(
                &mut stream,
                200,
                &json!({"model":"stub","choices":[{"finish_reason":"stop","message":{"content":"done run-secret"}}]}),
            );
        }
        requests
    });
    let output = command(dir.path(), &config_home)
        .env("STUB_PROFILE_KEY", "run-secret")
        .args(["run", "--jsonl", "task.json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("run-secret")
            && !stdout.contains("other-secret")
            && !stdout.contains("legacy-secret")
    );
    let events: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let resolved: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "provider.resolved")
        .collect();
    assert!(stdout.contains("done [REDACTED]"));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["payload"]["profile"], "local");
    assert_eq!(
        resolved[0]["payload"]["endpoint"],
        format!("{base}/custom/chat")
    );
    for event in events
        .iter()
        .filter(|event| event["type"] == "model.started")
    {
        assert_eq!(event["payload"]["effectiveConfig"], resolved[0]["payload"]);
    }
    let run_id = events[0]["runId"].as_str().unwrap();
    let log = fs::read_to_string(
        Workspace::open(dir.path())
            .unwrap()
            .paths
            .runs
            .join(run_id)
            .join("events.jsonl"),
    )
    .unwrap();
    assert_eq!(stdout, log);
    for request in server.join().unwrap() {
        assert!(request.starts_with("POST /custom/chat HTTP/1.1"));
        assert!(request.contains("authorization: Bearer run-secret"));
        assert!(request.contains("\"model\":\"stub\""));
    }
}

#[test]
fn run_and_diagnostic_do_not_log_a_key_echoed_in_provider_errors() {
    for diagnostic in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempdir().unwrap();
        let config_home = dir.path().join("config");
        save(
            &config_home.join("hyper/config.json"),
            &format!("http://{}", listener.local_addr().unwrap()),
        );
        let server = std::thread::spawn(move || {
            let mut stream = accept(&listener);
            read_request(&mut stream);
            respond(
                &mut stream,
                401,
                &json!({"error":{"code":"invalid_api_key","message":"echo leaked-secret"}}),
            );
        });
        let mut command = command(dir.path(), &config_home);
        command.env("STUB_PROFILE_KEY", "leaked-secret");
        if diagnostic {
            command.args(["profile", "test"]);
        } else {
            command.args(["plan", "--jsonl", "summarize"]);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("authentication"), "{text}");
        assert!(!text.contains("leaked-secret"));
        server.join().unwrap();
    }
}
