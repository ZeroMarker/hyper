use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use diffy::create_patch;
use ignore::WalkBuilder;
use serde_json::{Value, json};
use wait_timeout::ChildExt;

use crate::{
    approval::ApprovalGate,
    deepseek::{DeepSeekConfig, ToolSpec, chat_messages, system_prompt},
    event_sink::EventSink,
    model::*,
    policy,
    workspace::{self, RunPaths, Workspace, create_checkpoint, now, resolve_path},
};

struct EventWriter<'a> {
    run_id: String,
    task: &'a TaskSpec,
    path: &'a Path,
    workspace: &'a Workspace,
    gate: Option<ApprovalGate>,
    sink: Option<EventSink>,
    /// Earlier turns of the session this run belongs to, oldest first. Empty
    /// for a standalone run.
    history: Vec<SessionMessage>,
}
impl EventWriter<'_> {
    fn write(
        &self,
        kind: &str,
        payload: Value,
        step: Option<&str>,
        index: Option<usize>,
    ) -> Result<HarnessEvent> {
        let event = HarnessEvent {
            event_id: workspace::id(),
            run_id: self.run_id.clone(),
            task_id: self.task.task_id().into(),
            event_type: kind.into(),
            timestamp: now(),
            step_id: step.map(str::to_owned),
            step_index: index,
            payload,
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path)?;
        serde_json::to_writer(&mut file, &event)?;
        writeln!(file)?;
        self.workspace.insert_event(&event)?;
        if let Some(sink) = &self.sink {
            sink.push(&event);
        }
        Ok(event)
    }
}

fn assert_allowed(
    root: &Path,
    mode: AgentMode,
    action: &str,
    target: Option<&str>,
    command: Option<&str>,
) -> Result<()> {
    // Read-only guard first so plan mode always gets the accurate diagnostic,
    // regardless of the target path or shell command supplied.
    if mode == AgentMode::Plan && (action == "write" || action == "bash") {
        bail!("plan mode is read-only")
    }
    if let Some(target) = target {
        resolve_path(root, target)?;
    }
    if action == "bash" {
        policy::check_command(command.unwrap_or_default(), root)?;
    }
    Ok(())
}

fn tool(
    events: &EventWriter<'_>,
    run: &RunPaths,
    root: &Path,
    step: &StepSpec,
    index: usize,
) -> Result<(bool, Value)> {
    let instruction = step.instruction.trim();
    const TOOLS: [&str; 5] = ["bash", "read", "search", "write", "edit"];
    if let Some(name) = TOOLS
        .iter()
        .find(|t| instruction.starts_with(&format!("{t}:")))
        && !tool_allowed(step, name)
    {
        bail!("tool '{name}' is not allowed for step '{}'", step.id)
    }
    // `write:` and `edit:` bodies are read from the raw instruction: a trailing
    // newline is meaningful there (`write:path\n` means "empty content"), and
    // trimming it away would silently change what gets written.
    let body_source = step.instruction.trim_start();
    if let Some(command) = instruction.strip_prefix("bash:") {
        return bash(events, root, step, index, command.trim());
    }
    if let Some(path) = instruction.strip_prefix("read:") {
        return read(events, root, step, index, path.trim());
    }
    if let Some(query) = instruction.strip_prefix("search:") {
        return search(events, root, step, index, query.trim(), 100);
    }
    if let Some(body) = body_source.strip_prefix("write:") {
        // An instruction with no newline is almost always a truncated prompt;
        // defaulting the content to "" would silently empty an existing file.
        let Some((path, content)) = body.split_once('\n') else {
            bail!("write: expected format 'path\\ncontent' (missing content line)")
        };
        return write_file(events, run, root, step, index, path.trim(), content);
    }
    if let Some(body) = body_source.strip_prefix("edit:") {
        let mut lines = body.splitn(3, '\n');
        let path = lines.next().unwrap_or("").trim();
        let search = lines.next().unwrap_or("");
        let replace = match lines.next() {
            Some(replace) => replace,
            None => bail!("edit: expected format 'path\\nsearch\\nreplace'"),
        };
        if search.trim().is_empty() {
            bail!("edit: search text must not be empty")
        }
        return edit_file(events, run, root, step, index, path, search, replace);
    }
    agent(events, run, root, step, index, instruction)
}

fn tool_allowed(step: &StepSpec, name: &str) -> bool {
    step.tools
        .as_ref()
        .is_none_or(|tools| tools.iter().any(|allowed| allowed == name))
}

/// The tool-calling agent loop: the model chooses tools to call, each call is
/// executed and its result fed back as an observation, until the model replies
/// without tool calls or the turn budget is exhausted.
fn agent(
    events: &EventWriter<'_>,
    run: &RunPaths,
    root: &Path,
    step: &StepSpec,
    index: usize,
    prompt: &str,
) -> Result<(bool, Value)> {
    const MAX_TURNS: usize = 12;
    let config = DeepSeekConfig::from_env()?;
    events.write(
        "model.started",
        json!({"provider":config.provider,"baseUrl":config.base_url,"model":config.model,"protocol":config.protocol.as_str(),"agent":true,"maxTurns":MAX_TURNS}),
        Some(&step.id),
        Some(index),
    )?;
    let context = workspace_context(root)?;
    let mut messages = vec![json!({ "role": "system", "content": system_prompt(step.mode) })];
    // Earlier turns of the conversation come first, so the model can answer a
    // follow-up that depends on what was already discussed.
    messages.extend(
        events
            .history
            .iter()
            .map(|message| json!({"role": message.role, "content": message.content})),
    );
    messages.push(json!({ "role": "user", "content": format!(
        "<workspace_context>\n{context}\n</workspace_context>\n\n<request>\n{prompt}\n</request>"
    ) }));
    let specs = tool_specs_for(step);
    for turn in 0..MAX_TURNS {
        let reply = chat_messages(&config, &messages, Some(&specs))?;
        events.write(
            "model.iteration",
            json!({"turn":turn,"model":reply.model,"usage":reply.usage}),
            Some(&step.id),
            Some(index),
        )?;
        if reply.tool_calls.is_empty() {
            let payload = serde_json::to_value(&reply)?;
            events.write(
                "model.finished",
                json!({"provider":config.provider,"response":payload}),
                Some(&step.id),
                Some(index),
            )?;
            return Ok((true, payload));
        }
        events.write(
            "model.tool_calls",
            json!({"turn":turn,"calls":reply.tool_calls.iter().map(|call| json!({
                "id":call.id,"name":call.function.name,"arguments":call.function.arguments
            })).collect::<Vec<_>>()}),
            Some(&step.id),
            Some(index),
        )?;
        messages.push(json!({
            "role":"assistant",
            "content": if reply.content.is_empty() { Value::Null } else { json!(reply.content) },
            "tool_calls": reply.tool_calls.iter().map(|call| json!({
                "id":call.id,"type":"function",
                "function":{"name":call.function.name,"arguments":call.function.arguments}
            })).collect::<Vec<_>>(),
        }));
        for call in &reply.tool_calls {
            let observation = run_agent_tool(
                events,
                run,
                root,
                step,
                index,
                &call.function.name,
                &call.function.arguments,
            );
            messages.push(json!({
                "role":"tool",
                "tool_call_id":call.id,
                "content": observation,
            }));
        }
    }
    bail!("agent exceeded {MAX_TURNS} tool-calling turns")
}

fn tool_specs_for(step: &StepSpec) -> Vec<ToolSpec> {
    let all = vec![
        ToolSpec {
            name: "read",
            description: "Read a text file inside the workspace (up to 64 KB).",
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        },
        ToolSpec {
            name: "search",
            description: "Search for a fixed string across the workspace using rg.",
            parameters: json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
        },
        ToolSpec {
            name: "bash",
            description: "Run a shell command in the workspace root and capture stdout and stderr.",
            parameters: json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
        },
        ToolSpec {
            name: "write",
            description: "Create or overwrite a file inside the workspace with the given content.",
            parameters: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        },
        ToolSpec {
            name: "edit",
            description: "Replace the first occurrence of a search string in a file with a replacement.",
            parameters: json!({"type":"object","properties":{"path":{"type":"string"},"search":{"type":"string"},"replace":{"type":"string"}},"required":["path","search","replace"]}),
        },
    ];
    let mut specs = all;
    if step.mode == AgentMode::Plan {
        specs.retain(|spec| matches!(spec.name, "read" | "search"));
    }
    if let Some(allow) = &step.tools {
        specs.retain(|spec| allow.iter().any(|name| name == spec.name));
    }
    specs
}

fn arg_str<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn run_agent_tool(
    events: &EventWriter<'_>,
    run: &RunPaths,
    root: &Path,
    step: &StepSpec,
    index: usize,
    name: &str,
    arguments: &str,
) -> String {
    const MAX_OBSERVATION: usize = 4_000;
    let args: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    // The allowlist must also bind model-initiated calls: the model can emit a
    // tool call for a tool that was never advertised in `tools`.
    let result = if !tool_allowed(step, name) {
        Err(anyhow::anyhow!(
            "tool '{name}' is not allowed for step '{}'",
            step.id
        ))
    } else {
        match name {
            "bash" => bash(events, root, step, index, arg_str(&args, "command")),
            "read" => read(events, root, step, index, arg_str(&args, "path")),
            "search" => search(events, root, step, index, arg_str(&args, "query"), 100),
            "write" => write_file(
                events,
                run,
                root,
                step,
                index,
                arg_str(&args, "path"),
                arg_str(&args, "content"),
            ),
            "edit" => edit_file(
                events,
                run,
                root,
                step,
                index,
                arg_str(&args, "path"),
                arg_str(&args, "search"),
                arg_str(&args, "replace"),
            ),
            other => Err(anyhow::anyhow!("unknown tool: {other}")),
        }
    };
    let text = match result {
        Ok((true, payload)) => serde_json::to_string(&payload)
            .unwrap_or_else(|error| format!("serialize error: {error}")),
        Ok((false, payload)) => {
            let code = payload
                .get("exitCode")
                .and_then(Value::as_i64)
                .unwrap_or(-1);
            let stderr = payload.get("stderr").and_then(Value::as_str).unwrap_or("");
            if stderr.trim().is_empty() {
                serde_json::to_string(&payload)
                    .unwrap_or_else(|error| format!("serialize error: {error}"))
            } else {
                format!("tool failed (exit {code}): {}", stderr.trim())
            }
        }
        Err(error) => format!("tool error: {error}"),
    };
    truncate_head_tail(&text, MAX_OBSERVATION)
}

fn workspace_context(root: &Path) -> Result<String> {
    const MAX_TOTAL: usize = 64_000;
    const MAX_FILE: usize = 6_000;
    let files = match Command::new("rg")
        .args([
            "--files",
            "--hidden",
            "-g",
            "!.git/**",
            "-g",
            "!.harness/**",
            "-g",
            "!target/**",
        ])
        .current_dir(root)
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .take(300)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        Ok(output) => bail!(
            "rg failed to enumerate workspace files: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            workspace_files(root, true, 300)
        }
        Err(error) => return Err(error).context("failed to enumerate workspace files with rg"),
    };
    let mut context = format!(
        "Workspace: {}\n\nFiles:\n{}\n",
        root.display(),
        files.join("\n")
    );
    let preferred = files.iter().filter(|path| {
        matches!(
            path.as_str(),
            "README.md" | "Cargo.toml" | "plan.md" | "todo.md"
        ) || path.starts_with("src/") && path.ends_with(".rs")
    });
    for relative in preferred {
        if context.len() >= MAX_TOTAL {
            break;
        }
        let Ok(content) = fs::read_to_string(root.join(relative)) else {
            continue;
        };
        let remaining = MAX_TOTAL.saturating_sub(context.len());
        let limit = MAX_FILE.min(remaining);
        let excerpt = truncate_utf8(&content, limit);
        context.push_str(&format!("\n--- {relative} ---\n{excerpt}\n"));
    }
    Ok(context)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// Keep both ends of an observation so the model sees the command setup and
/// the final compiler/test error. The returned string never exceeds `max_bytes`.
fn truncate_head_tail(value: &str, max_bytes: usize) -> String {
    const MARKER: &str = "\n... [truncated] ...\n";
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    if max_bytes <= MARKER.len() {
        return truncate_utf8(value, max_bytes).to_owned();
    }

    let available = max_bytes - MARKER.len();
    let head_budget = available / 2;
    let tail_budget = available - head_budget;
    let head = truncate_utf8(value, head_budget);
    let mut tail_start = value.len().saturating_sub(tail_budget);
    while !value.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!("{head}{MARKER}{}", &value[tail_start..])
}

fn workspace_files(root: &Path, include_hidden: bool, limit: usize) -> Vec<String> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!include_hidden)
        .git_ignore(true)
        .git_global(true);
    builder.filter_entry(|entry| {
        if entry.depth() == 0 {
            return true;
        }
        !matches!(
            entry.file_name().to_str(),
            Some(".git" | ".harness" | "target" | "node_modules")
        )
    });
    builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            entry
                .path()
                .strip_prefix(root)
                .ok()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
        })
        .take(limit)
        .collect()
}

/// Default wall-clock budget for a `bash` step.
const DEFAULT_BASH_TIMEOUT_MS: u64 = 120_000;
/// Bytes kept per stream before the rest is discarded. The reader keeps
/// draining regardless, so the cap never re-introduces a pipe stall.
const MAX_CAPTURED_OUTPUT: usize = 256 * 1024;
/// How long to wait for a timed-out shell to actually die before giving up.
const KILL_GRACE: Duration = Duration::from_secs(5);

fn bash(
    events: &EventWriter<'_>,
    root: &Path,
    step: &StepSpec,
    index: usize,
    command: &str,
) -> Result<(bool, Value)> {
    assert_allowed(root, step.mode, "bash", None, Some(command))?;
    if let Some(gate) = &events.gate
        && !gate.request("bash", command)
    {
        events.write(
            "tool.denied",
            json!({"tool":"bash","command":command}),
            Some(&step.id),
            Some(index),
        )?;
        bail!("user denied bash command")
    }
    events.write(
        "tool.started",
        json!({"tool":"bash","command":command}),
        Some(&step.id),
        Some(index),
    )?;
    let started = Instant::now();
    #[cfg(windows)]
    let mut builder = {
        let mut command_builder = Command::new("cmd");
        command_builder.args(["/D", "/S", "/C", command]);
        command_builder
    };
    #[cfg(not(windows))]
    let mut builder = {
        let mut command_builder = Command::new("sh");
        command_builder.args(["-c", command]);
        command_builder
    };
    builder
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Run the shell in its own process group so a timeout can kill the whole
    // tree (shell plus any background children) instead of leaving orphans.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        builder.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        builder.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    // On Linux, ask the kernel to kill the shell when the harness dies, so a
    // crashed or `kill -9`ed harness cannot leave running commands behind.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // Safety: `pre_exec` runs between fork and exec, where only
        // async-signal-safe calls are allowed. `prctl`, `getppid` and `_exit`
        // all qualify.
        unsafe {
            builder.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Guard the fork/prctl race: if the harness already died, the
                // signal would never arrive, so exit instead of running on.
                if libc::getppid() == 1 {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }
    let mut child = builder.spawn()?;
    let stdout = child
        .stdout
        .take()
        .context("bash child stdout was not piped")?;
    let stderr = child
        .stderr
        .take()
        .context("bash child stderr was not piped")?;
    // Drain both pipes while the child runs. Waiting first and reading after
    // deadlocks as soon as the command writes more than the pipe capacity
    // (~64 KB), which would hang every non-trivial build or test command until
    // the timeout and then report it as a failure.
    let stdout_reader = capture(stdout, MAX_CAPTURED_OUTPUT);
    let stderr_reader = capture(stderr, MAX_CAPTURED_OUTPUT);
    let mut guard = ProcessGroupGuard::new(child.id());
    let timeout = Duration::from_millis(step.timeout_ms.unwrap_or(DEFAULT_BASH_TIMEOUT_MS));
    let timed_out = child.wait_timeout(timeout)?.is_none();
    if timed_out {
        kill_process_tree(&mut child);
    }
    let status = match child.wait_timeout(KILL_GRACE)? {
        Some(status) => status,
        None => bail!("bash command could not be terminated after {timeout:?}"),
    };
    guard.disarm();
    // The child is gone, so both pipes are closed and the readers can finish.
    let (stdout_bytes, stdout_total) = join_capture(stdout_reader, "stdout");
    let (stderr_bytes, stderr_total) = join_capture(stderr_reader, "stderr");
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    let truncated = stdout_total > stdout_bytes.len() || stderr_total > stderr_bytes.len();
    let all = format!("{stdout}{stderr}");
    let code = status.code().unwrap_or(-1);
    let duration_ms = started.elapsed().as_millis();
    let payload = json!({
        "command": command,
        "cwd": root,
        "exitCode": code,
        "stdout": stdout,
        "stderr": stderr,
        "all": all,
        "timedOut": timed_out,
        "truncated": truncated,
        "stdoutBytes": stdout_total,
        "stderrBytes": stderr_total,
        "durationMs": duration_ms,
    });
    let mut event_payload = payload.clone();
    event_payload["tool"] = json!("bash");
    events.write("tool.finished", event_payload, Some(&step.id), Some(index))?;
    Ok((code == 0, payload))
}

/// Read a child pipe to EOF on its own thread, keeping at most `cap` bytes.
///
/// Draining past the cap is mandatory: a reader that stops early fills the pipe
/// buffer and blocks the child exactly like not reading at all.
fn capture<R: Read + Send + 'static>(mut reader: R, cap: usize) -> JoinHandle<(Vec<u8>, usize)> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut total = 0usize;
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    total += read;
                    if kept.len() < cap {
                        let room = cap - kept.len();
                        kept.extend_from_slice(&buffer[..room.min(read)]);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        (kept, total)
    })
}

fn join_capture(handle: JoinHandle<(Vec<u8>, usize)>, stream: &str) -> (Vec<u8>, usize) {
    handle.join().unwrap_or_else(|_| {
        eprintln!("warning: the {stream} reader thread panicked; output was lost");
        (Vec::new(), 0)
    })
}

/// Kill a whole process group (or process tree on Windows).
fn kill_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        // The shell was spawned as a new process-group leader, so the negative
        // pid targets the entire tree (shell + children).
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
}

fn kill_process_tree(child: &mut Child) {
    kill_group(child.id());
    // The group kill may already have reaped the child; a failure here only
    // means there is nothing left to kill.
    let _ = child.kill();
}

/// Kills the process group if it was not disarmed, so an early return or a
/// panic inside the harness cannot leak a running command.
struct ProcessGroupGuard {
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pid: u32) -> Self {
        Self { pid: Some(pid) }
    }

    fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            kill_group(pid);
        }
    }
}

fn read(
    events: &EventWriter<'_>,
    root: &Path,
    step: &StepSpec,
    index: usize,
    path: &str,
) -> Result<(bool, Value)> {
    if path.trim().is_empty() {
        bail!("read: path must not be empty")
    }
    assert_allowed(root, step.mode, "read", Some(path), None)?;
    events.write(
        "tool.started",
        json!({"tool":"read","input":{"path":path}}),
        Some(&step.id),
        Some(index),
    )?;
    let bytes = fs::read(resolve_path(root, path)?)?;
    let max = 64_000.min(bytes.len());
    let payload = json!({"path":path,"content":String::from_utf8_lossy(&bytes[..max]),"truncated":bytes.len()>max,"bytes":bytes.len()});
    events.write("tool.finished",json!({"tool":"read","path":path,"content":String::from_utf8_lossy(&bytes[..max]),"truncated":bytes.len()>max,"bytes":bytes.len()}),Some(&step.id),Some(index))?;
    Ok((true, payload))
}
fn search(
    events: &EventWriter<'_>,
    root: &Path,
    step: &StepSpec,
    index: usize,
    query: &str,
    limit: usize,
) -> Result<(bool, Value)> {
    assert_allowed(root, step.mode, "read", None, None)?;
    events.write(
        "tool.started",
        json!({"tool":"search","input":{"query":query,"limit":limit}}),
        Some(&step.id),
        Some(index),
    )?;
    let out = Command::new("rg")
        // `--` ends flag parsing so a query beginning with `-` (e.g. `--pre=...`)
        // is matched literally instead of being parsed as an rg option.
        .args(["--line-number", "--fixed-strings", "--", query, "."])
        .current_dir(root)
        .output();
    let (code, lines) = match out {
        Ok(out) => (
            out.status.code().unwrap_or(2),
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .take(limit)
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let lines = search_workspace_files(root, query, limit);
            (i32::from(lines.is_empty()), lines)
        }
        Err(error) => return Err(error).context("failed to run rg"),
    };
    let payload = json!({"query":query,"lines":lines,"exitCode":code});
    events.write(
        "tool.finished",
        json!({"tool":"search","query":query,"lines":lines,"exitCode":code}),
        Some(&step.id),
        Some(index),
    )?;
    Ok((code <= 1, payload))
}

fn search_workspace_files(root: &Path, query: &str, limit: usize) -> Vec<String> {
    let mut matches = Vec::new();
    for relative in workspace_files(root, false, usize::MAX) {
        let Ok(content) = fs::read_to_string(root.join(&relative)) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            if line.contains(query) {
                matches.push(format!("{relative}:{}:{line}", index + 1));
                if matches.len() == limit {
                    return matches;
                }
            }
        }
    }
    matches
}
fn write_file(
    events: &EventWriter<'_>,
    run: &RunPaths,
    root: &Path,
    step: &StepSpec,
    index: usize,
    path: &str,
    content: &str,
) -> Result<(bool, Value)> {
    if path.trim().is_empty() {
        bail!("write: path must not be empty")
    }
    assert_allowed(root, step.mode, "write", Some(path), None)?;
    if let Some(gate) = &events.gate
        && !gate.request("write", path)
    {
        events.write(
            "tool.denied",
            json!({"tool":"write","path":path}),
            Some(&step.id),
            Some(index),
        )?;
        bail!("user denied write to {path}")
    }
    events.write(
        "tool.started",
        json!({"tool":"write","path":path}),
        Some(&step.id),
        Some(index),
    )?;
    let target = resolve_path(root, path)?;
    let before = fs::read_to_string(&target).unwrap_or_default();
    let cp = create_checkpoint(root, &run.checkpoints, path)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(target, content)?;
    let diff = create_patch(&before, content).to_string();
    events.write(
        "checkpoint.created",
        json!({"checkpoint":cp}),
        Some(&step.id),
        Some(index),
    )?;
    let payload = json!({"path":path,"checkpointId":cp.id,"diff":diff});
    events.write(
        "tool.finished",
        json!({"tool":"write","path":path,"checkpointId":cp.id,"diff":diff}),
        Some(&step.id),
        Some(index),
    )?;
    Ok((true, payload))
}
#[allow(clippy::too_many_arguments)]
fn edit_file(
    events: &EventWriter<'_>,
    run: &RunPaths,
    root: &Path,
    step: &StepSpec,
    index: usize,
    path: &str,
    search: &str,
    replace: &str,
) -> Result<(bool, Value)> {
    if path.trim().is_empty() {
        bail!("edit: path must not be empty")
    }
    assert_allowed(root, step.mode, "write", Some(path), None)?;
    if let Some(gate) = &events.gate
        && !gate.request("edit", path)
    {
        events.write(
            "tool.denied",
            json!({"tool":"edit","path":path}),
            Some(&step.id),
            Some(index),
        )?;
        bail!("user denied edit to {path}")
    }
    events.write(
        "tool.started",
        json!({"tool":"edit","path":path}),
        Some(&step.id),
        Some(index),
    )?;
    let target = resolve_path(root, path)?;
    let before = fs::read_to_string(&target)?;
    if !before.contains(search) {
        bail!("search text not found in {path}")
    }
    let after = before.replacen(search, replace, 1);
    let cp = create_checkpoint(root, &run.checkpoints, path)?;
    fs::write(target, &after)?;
    let diff = create_patch(&before, &after).to_string();
    events.write(
        "checkpoint.created",
        json!({"checkpoint":cp}),
        Some(&step.id),
        Some(index),
    )?;
    let payload = json!({"path":path,"checkpointId":cp.id,"diff":diff});
    events.write(
        "tool.finished",
        json!({"tool":"edit","path":path,"checkpointId":cp.id,"diff":diff}),
        Some(&step.id),
        Some(index),
    )?;
    Ok((true, payload))
}

pub fn run_task(task: &TaskSpec, root: impl AsRef<Path>) -> Result<RunSummary> {
    run_task_inner(task, root, None, None, None)
}

/// Run a task with an interactive approval gate: `bash`, `write` and `edit`
/// tools pause and ask the gate for permission before executing.
pub fn run_task_with_approval(
    task: &TaskSpec,
    root: impl AsRef<Path>,
    gate: ApprovalGate,
) -> Result<RunSummary> {
    run_task_inner(task, root, Some(gate), None, None)
}

/// Run a task as one more turn of a conversation: the session's earlier turns
/// are replayed to the model as context, and this turn is appended afterwards.
pub fn run_task_in_session(
    task: &TaskSpec,
    root: impl AsRef<Path>,
    session_id: &str,
) -> Result<RunSummary> {
    run_task_inner(task, root, None, Some(session_id), None)
}

/// Same as [`run_task_in_session`], with the TUI approval gate.
pub fn run_task_in_session_with_approval(
    task: &TaskSpec,
    root: impl AsRef<Path>,
    session_id: &str,
    gate: ApprovalGate,
) -> Result<RunSummary> {
    run_task_inner(task, root, Some(gate), Some(session_id), None)
}

/// Run a TUI conversation turn while publishing persisted events to the UI.
pub fn run_task_in_session_with_updates(
    task: &TaskSpec,
    root: impl AsRef<Path>,
    session_id: &str,
    gate: ApprovalGate,
    sink: EventSink,
) -> Result<RunSummary> {
    run_task_inner(task, root, Some(gate), Some(session_id), Some(sink))
}

fn run_task_inner(
    task: &TaskSpec,
    root: impl AsRef<Path>,
    gate: Option<ApprovalGate>,
    session_id: Option<&str>,
    sink: Option<EventSink>,
) -> Result<RunSummary> {
    task.validate()?;
    let workspace = Workspace::open(root)?;
    let run_id = workspace::id();
    let run = workspace.prepare_run(&run_id)?;
    // Hold the run lock for the whole run. It is what tells other harnesses
    // (and the next startup) that this run is still alive, so it must be taken
    // before the run is announced to the database.
    let _lock = workspace.lock_run(&run_id)?;
    fs::write(&run.task, serde_json::to_vec_pretty(task)?)?;
    let started = now();
    workspace.create_run(&run_id, task, &started)?;
    // The prompt is part of the conversation even if the run fails, so record
    // the user turn before executing anything. History is read first so this
    // turn's prompt is not both replayed and re-sent.
    let history = match session_id {
        Some(session_id) => {
            let history = workspace.session_messages(session_id)?;
            workspace.append_session_message(session_id, &user_turn(task, &started, &run_id))?;
            history
        }
        None => Vec::new(),
    };
    let events = EventWriter {
        run_id: run_id.clone(),
        task,
        path: &run.events,
        workspace: &workspace,
        gate,
        sink,
        history,
    };
    events.write(
        "run.started",
        json!({"taskName":task.name,"sessionId":session_id}),
        None,
        None,
    )?;
    let mut succeeded = 0;
    let mut failure = None;
    for (index, step) in task.steps.iter().enumerate() {
        events.write(
            "step.started",
            json!({"instruction":step.instruction,"mode":step.mode}),
            Some(&step.id),
            Some(index),
        )?;
        match tool(&events, &run, &workspace.paths.root, step, index) {
            Ok((true, payload)) => {
                events.write(
                    "step.finished",
                    json!({"output":payload}),
                    Some(&step.id),
                    Some(index),
                )?;
                succeeded += 1
            }
            Ok((false, payload)) => {
                let msg = failure_message(&payload);
                failure = Some(fail(&events, step, index, msg)?);
                break;
            }
            Err(error) => {
                // `{:#}` keeps the source chain, so a failure reports "failed to
                // call the opencode-go API: connection refused" instead of only
                // the outermost context.
                failure = Some(fail(&events, step, index, format!("{error:#}"))?);
                break;
            }
        }
    }
    let finished = now();
    let summary = RunSummary {
        run_id: run_id.clone(),
        task_name: task.name.clone(),
        status: if failure.is_some() {
            "failed"
        } else {
            "finished"
        }
        .into(),
        steps_total: task.steps.len(),
        steps_succeeded: succeeded,
        steps_failed: usize::from(failure.is_some()),
        started_at: started,
        finished_at: finished,
        failure,
    };
    if summary.failure.is_none() {
        events.write("run.finished", json!({"summary":summary}), None, None)?;
    }
    fs::write(&run.summary, serde_json::to_vec_pretty(&summary)?)?;
    // The conversation keeps what the user saw, failure included, so the next
    // turn can refer to it.
    if let Some(session_id) = session_id
        && let Some(message) = assistant_turn(&workspace, &summary)?
    {
        workspace.append_session_message(session_id, &message)?;
    }
    Ok(summary)
}
/// Build the human-readable reason for a tool that reported failure, keeping a
/// timeout distinguishable from an ordinary non-zero exit.
fn failure_message(payload: &Value) -> String {
    let command = payload.get("command").and_then(Value::as_str).unwrap_or("");
    if payload
        .get("timedOut")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return format!(
            "command {command:?} timed out after {}ms and was killed",
            payload
                .get("durationMs")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        );
    }
    format!(
        "tool reported failure: command {command:?}, exit code {}{}",
        payload
            .get("exitCode")
            .and_then(Value::as_i64)
            .unwrap_or(-1),
        payload
            .get("stderr")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| format!(", {}", s.trim()))
            .unwrap_or_default()
    )
}

fn fail(
    events: &EventWriter<'_>,
    step: &StepSpec,
    index: usize,
    message: String,
) -> Result<Failure> {
    let error_type = classify_error(&message);
    let failure = Failure {
        error_type: error_type.into(),
        message,
        retryable: matches!(error_type, "TimeoutError" | "ModelError"),
        step_id: Some(step.id.clone()),
        details: HashMap::new(),
        cause: None,
    };
    events.write(
        "tool.failed",
        json!({"failure":failure}),
        Some(&step.id),
        Some(index),
    )?;
    events.write(
        "step.failed",
        json!({"failure":failure}),
        Some(&step.id),
        Some(index),
    )?;
    events.write("run.failed", json!({"failure":failure}), None, None)?;
    Ok(failure)
}
fn classify_error(message: &str) -> &'static str {
    if message.contains("plan mode is read-only")
        || message.contains("dangerous pattern")
        || message.contains("escapes workspace root")
        || message.contains("not allowed for step")
        || message.contains("user denied")
    {
        "PolicyError"
    } else if message.contains("timed out") || message.contains("could not be terminated") {
        "TimeoutError"
    } else if message.contains("DeepSeek") || message.contains("agent exceeded") {
        "ModelError"
    } else if message.contains("not found in")
        || message.contains("expected format")
        || message.contains("must not be empty")
    {
        "ToolError"
    } else {
        "Error"
    }
}

/// The user turn a run contributes to its conversation. The task name is the
/// prompt for a prompt-driven run, which is the only shape the TUI produces.
fn user_turn(task: &TaskSpec, started: &str, run_id: &str) -> SessionMessage {
    SessionMessage {
        role: "user".into(),
        content: task.name.clone(),
        timestamp: started.into(),
        run_id: Some(run_id.into()),
    }
}

/// The assistant turn for a finished run: the model's final answer when it
/// produced one, otherwise a line describing how the run ended. `None` only
/// when the run has neither, which cannot happen in practice.
fn assistant_turn(workspace: &Workspace, summary: &RunSummary) -> Result<Option<SessionMessage>> {
    let content = match latest_model_reply(workspace.paths.root.clone(), &summary.run_id) {
        Ok(Some(content)) if !content.trim().is_empty() => content,
        _ => match &summary.failure {
            Some(failure) => format!("Run {} failed: {}", summary.run_id, failure.message),
            None => format!("Run {} {}", summary.run_id, summary.status),
        },
    };
    Ok(Some(SessionMessage {
        role: "assistant".into(),
        content,
        timestamp: summary.finished_at.clone(),
        run_id: Some(summary.run_id.clone()),
    }))
}
pub fn prompt_to_task(prompt: &str, mode: AgentMode) -> TaskSpec {
    TaskSpec {
        id: None,
        name: prompt.chars().take(80).collect(),
        steps: vec![StepSpec {
            id: mode.as_str().into(),
            mode,
            instruction: prompt.into(),
            tools: None,
            timeout_ms: None,
            metadata: HashMap::new(),
        }],
        metadata: HashMap::new(),
    }
}
pub fn list_runs(root: impl AsRef<Path>, limit: usize) -> Result<Vec<RunRow>> {
    Workspace::open(root)?.list_runs(limit)
}
pub fn get_run_details(
    root: impl AsRef<Path>,
    id: &str,
) -> Result<(Option<RunRow>, Vec<HarnessEvent>)> {
    let ws = Workspace::open(root)?;
    Ok((ws.get_run(id)?, ws.events(id)?))
}

pub fn latest_model_reply(root: impl AsRef<Path>, run_id: &str) -> Result<Option<String>> {
    let workspace = Workspace::open(root)?;
    let events = workspace.events(run_id)?;
    Ok(events.iter().rev().find_map(|event| {
        if event.event_type != "model.finished" {
            return None;
        }
        event
            .payload
            .get("response")?
            .get("content")?
            .as_str()
            .map(str::to_owned)
    }))
}

pub fn latest_display_output(root: impl AsRef<Path>, run_id: &str) -> Result<Option<String>> {
    let workspace = Workspace::open(root)?;
    let events = workspace.events(run_id)?;
    for event in events.iter().rev() {
        if event.event_type == "model.finished"
            && let Some(content) = event
                .payload
                .get("response")
                .and_then(|value| value.get("content"))
                .and_then(Value::as_str)
        {
            return Ok(Some(content.to_owned()));
        }
        if event.event_type == "run.failed"
            && let Some(message) = event
                .payload
                .get("failure")
                .and_then(|value| value.get("message"))
                .and_then(Value::as_str)
        {
            return Ok(Some(format!("执行失败：{message}")));
        }
        if event.event_type == "tool.finished" {
            for field in ["stdout", "content", "diff"] {
                if let Some(text) = event.payload.get(field).and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    return Ok(Some(text.trim().to_owned()));
                }
            }
            if let Some(lines) = event.payload.get("lines").and_then(Value::as_array) {
                let text = lines
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(Some(if text.is_empty() {
                    "未找到匹配内容。".into()
                } else {
                    text
                }));
            }
            let tool = event
                .payload
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("工具");
            return Ok(Some(format!("{tool} 执行完成。")));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentMode;
    use std::collections::HashMap;

    fn gated_step(tools: Vec<&str>) -> StepSpec {
        StepSpec {
            id: "step".into(),
            mode: AgentMode::Build,
            instruction: "test".into(),
            tools: Some(tools.into_iter().map(str::to_owned).collect()),
            timeout_ms: None,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn agent_tool_calls_respect_step_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let run_id = workspace::id();
        let run = workspace.prepare_run(&run_id).unwrap();
        let task = prompt_to_task("test", AgentMode::Build);
        let events = EventWriter {
            run_id,
            task: &task,
            path: &run.events,
            workspace: &workspace,
            gate: None,
            sink: None,
            history: Vec::new(),
        };
        // `bash` is not in the allowlist and must be rejected even though the
        // model emitted the call itself.
        let step = gated_step(vec!["read"]);
        let observation = run_agent_tool(
            &events,
            &run,
            dir.path(),
            &step,
            0,
            "bash",
            r#"{"command":"echo hi"}"#,
        );
        assert!(observation.contains("not allowed"), "{observation}");
        // An allowed tool still goes through.
        let observation = run_agent_tool(
            &events,
            &run,
            dir.path(),
            &step,
            0,
            "read",
            r#"{"path":"some-file.txt"}"#,
        );
        assert!(!observation.contains("not allowed"), "{observation}");
    }

    /// Stopping at the cap would refill the pipe buffer and deadlock the child,
    /// so the reader must keep draining and only report how much it dropped.
    #[test]
    fn capture_drains_past_the_cap() {
        let data = vec![b'x'; 1024];
        let (kept, total) = capture(std::io::Cursor::new(data), 16).join().unwrap();
        assert_eq!(kept.len(), 16);
        assert_eq!(total, 1024);
    }

    #[test]
    fn failure_message_distinguishes_timeouts() {
        let timed_out = json!({"command": "cargo test", "timedOut": true, "durationMs": 300});
        let message = failure_message(&timed_out);
        assert!(message.contains("timed out"), "{message}");
        assert_eq!(classify_error(&message), "TimeoutError");

        let failed = json!({"command": "false", "exitCode": 1, "stderr": "boom"});
        let message = failure_message(&failed);
        assert!(message.contains("exit code 1"), "{message}");
        assert!(message.contains("boom"), "{message}");
        assert_eq!(classify_error(&message), "Error");
    }

    #[test]
    fn observations_keep_the_head_and_tail() {
        let value = format!("HEAD{}TAIL", "x".repeat(100));
        let truncated = truncate_head_tail(&value, 40);
        assert!(truncated.starts_with("HEAD"), "{truncated}");
        assert!(truncated.ends_with("TAIL"), "{truncated}");
        assert!(truncated.contains("[truncated]"), "{truncated}");
        assert!(truncated.len() <= 40);
    }

    #[test]
    fn built_in_search_finds_fixed_strings() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("sample.txt"), "first\n--literal value\n").unwrap();
        let matches = search_workspace_files(dir.path(), "--literal", 10);
        assert_eq!(matches, vec!["sample.txt:2:--literal value"]);
    }
}
