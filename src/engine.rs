use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use diffy::create_patch;
use serde_json::{Value, json};
use wait_timeout::ChildExt;

use crate::{
    deepseek::{DeepSeekConfig, ToolSpec, chat_messages, system_prompt},
    model::*,
    workspace::{self, RunPaths, Workspace, create_checkpoint, now, resolve_path},
};

struct EventWriter<'a> {
    run_id: String,
    task: &'a TaskSpec,
    path: &'a Path,
    workspace: &'a Workspace,
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
        let cmd = command.unwrap_or_default();
        for dangerous in ["rm -rf", "sudo", "chmod -R", "chown -R", "/dev/sd", "dd "] {
            if cmd.contains(dangerous) {
                bail!("command matches dangerous pattern")
            }
        }
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
    {
        if !tool_allowed(step, name) {
            bail!("tool '{name}' is not allowed for step '{}'", step.id)
        }
    }
    if let Some(command) = instruction.strip_prefix("bash:") {
        return bash(events, root, step, index, command.trim());
    }
    if let Some(path) = instruction.strip_prefix("read:") {
        return read(events, root, step, index, path.trim());
    }
    if let Some(query) = instruction.strip_prefix("search:") {
        return search(events, root, step, index, query.trim(), 100);
    }
    if let Some(body) = instruction.strip_prefix("write:") {
        let (path, content) = body.split_once('\n').unwrap_or((body, ""));
        return write_file(events, run, root, step, index, path.trim(), content);
    }
    if let Some(body) = instruction.strip_prefix("edit:") {
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
        json!({"provider":"deepseek","model":config.model,"agent":true,"maxTurns":MAX_TURNS}),
        Some(&step.id),
        Some(index),
    )?;
    let context = workspace_context(root)?;
    let mut messages = vec![
        json!({ "role": "system", "content": system_prompt(step.mode) }),
        json!({ "role": "user", "content": format!(
            "<workspace_context>\n{context}\n</workspace_context>\n\n<request>\n{prompt}\n</request>"
        ) }),
    ];
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
                json!({"provider":"deepseek","response":payload}),
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

fn arg_str(args: &Value, key: &str) -> &str {
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
    let result = match name {
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
    truncate_utf8(&text, MAX_OBSERVATION).to_owned()
}

fn workspace_context(root: &Path) -> Result<String> {
    const MAX_TOTAL: usize = 64_000;
    const MAX_FILE: usize = 6_000;
    let output = Command::new("rg")
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
        .context("failed to enumerate workspace files with rg")?;
    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .take(300)
        .map(str::to_owned)
        .collect::<Vec<_>>();
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

fn bash(
    events: &EventWriter<'_>,
    root: &Path,
    step: &StepSpec,
    index: usize,
    command: &str,
) -> Result<(bool, Value)> {
    assert_allowed(root, step.mode, "bash", None, Some(command))?;
    events.write(
        "tool.started",
        json!({"tool":"bash","command":command}),
        Some(&step.id),
        Some(index),
    )?;
    let started = Instant::now();
    let mut child = Command::new("sh")
        .args(["-c", command])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let timeout = Duration::from_millis(step.timeout_ms.unwrap_or(120_000));
    let status = child.wait_timeout(timeout)?;
    if status.is_none() {
        child.kill()?;
    }
    let output = child.wait_with_output()?;
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let all = format!("{stdout}{stderr}");
    let payload = json!({"command":command,"cwd":root,"exitCode":code,"stdout":stdout,"stderr":stderr,"all":all,"durationMs":started.elapsed().as_millis()});
    events.write("tool.finished",json!({"tool":"bash", "command":command,"cwd":root,"exitCode":code,"stdout":stdout,"stderr":stderr,"all":all,"durationMs":started.elapsed().as_millis()}),Some(&step.id),Some(index))?;
    Ok((code == 0, payload))
}
fn read(
    events: &EventWriter<'_>,
    root: &Path,
    step: &StepSpec,
    index: usize,
    path: &str,
) -> Result<(bool, Value)> {
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
        .args(["--line-number", "--fixed-strings", query, "."])
        .current_dir(root)
        .output()
        .context("failed to run rg")?;
    let code = out.status.code().unwrap_or(2);
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .take(limit)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let payload = json!({"query":query,"lines":lines,"exitCode":code});
    events.write(
        "tool.finished",
        json!({"tool":"search","query":query,"lines":lines,"exitCode":code}),
        Some(&step.id),
        Some(index),
    )?;
    Ok((code <= 1, payload))
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
    assert_allowed(root, step.mode, "write", Some(path), None)?;
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
    assert_allowed(root, step.mode, "write", Some(path), None)?;
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
    task.validate()?;
    let workspace = Workspace::open(root)?;
    let run_id = workspace::id();
    let run = workspace.prepare_run(&run_id)?;
    fs::write(&run.task, serde_json::to_vec_pretty(task)?)?;
    let started = now();
    workspace.create_run(&run_id, task, &started)?;
    let events = EventWriter {
        run_id: run_id.clone(),
        task,
        path: &run.events,
        workspace: &workspace,
    };
    events.write("run.started", json!({"taskName":task.name}), None, None)?;
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
                let msg = format!(
                    "tool reported failure: command {:?}, exit code {}{}",
                    payload.get("command").and_then(Value::as_str).unwrap_or(""),
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
                );
                failure = Some(fail(&events, step, index, msg)?);
                break;
            }
            Err(error) => {
                failure = Some(fail(&events, step, index, error.to_string())?);
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
    save_session(&workspace, &summary)?;
    Ok(summary)
}
fn fail(
    events: &EventWriter<'_>,
    step: &StepSpec,
    index: usize,
    message: String,
) -> Result<Failure> {
    let failure = Failure {
        error_type: classify_error(&message).into(),
        message,
        retryable: false,
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
    {
        "PolicyError"
    } else if message.contains("DeepSeek") || message.contains("agent exceeded") {
        "ModelError"
    } else if message.contains("not found in") || message.contains("expected format") {
        "ToolError"
    } else {
        "Error"
    }
}

fn save_session(workspace: &Workspace, summary: &RunSummary) -> Result<()> {
    let id = workspace::id();
    let messages = [
        json!({"role":"user","content":summary.task_name,"timestamp":summary.started_at,"metadata":{"runId":summary.run_id}}),
        json!({"role":"assistant","content":format!("Run {} {}",summary.run_id,summary.status),"timestamp":summary.finished_at,"metadata":{}}),
    ];
    let text = messages
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(workspace.paths.sessions.join(format!("{id}.jsonl")), text)?;
    Ok(())
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
