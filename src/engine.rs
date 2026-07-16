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
    deepseek::{DeepSeekConfig, chat},
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
    if let Some(target) = target {
        resolve_path(root, target)?;
    }
    if action == "write" && mode == AgentMode::Plan {
        bail!("plan mode is read-only")
    }
    if action == "bash" {
        let cmd = command.unwrap_or_default();
        if mode == AgentMode::Plan {
            bail!("bash requires confirmation in plan mode")
        };
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
        let replace = lines.next().unwrap_or("");
        return edit_file(events, run, root, step, index, path, search, replace);
    }
    model(events, root, step, index, instruction)
}

fn model(
    events: &EventWriter<'_>,
    root: &Path,
    step: &StepSpec,
    index: usize,
    prompt: &str,
) -> Result<(bool, Value)> {
    let config = DeepSeekConfig::from_env()?;
    events.write(
        "model.started",
        json!({"provider":"deepseek","model":config.model}),
        Some(&step.id),
        Some(index),
    )?;
    let context = workspace_context(root)?;
    let reply = chat(&config, prompt, step.mode, &context)?;
    let payload = serde_json::to_value(&reply)?;
    events.write(
        "model.finished",
        json!({"provider":"deepseek","response":payload}),
        Some(&step.id),
        Some(index),
    )?;
    Ok((true, payload))
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
        error_type: "Error".into(),
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
        (event.event_type == "model.finished")
            .then(|| {
                event
                    .payload
                    .get("response")?
                    .get("content")?
                    .as_str()
                    .map(str::to_owned)
            })
            .flatten()
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
