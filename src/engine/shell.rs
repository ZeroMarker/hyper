use super::*;

/// Default wall-clock budget for a `bash` step.
pub(super) const DEFAULT_BASH_TIMEOUT_MS: u64 = 120_000;
/// Bytes kept per stream before the rest is discarded. The reader keeps
/// draining regardless, so the cap never re-introduces a pipe stall.
pub(super) const MAX_CAPTURED_OUTPUT: usize = 256 * 1024;
/// Bytes of one stream kept for `artifacts/`, wider than the event's slice on
/// purpose: the log stays bounded per event while a build's whole tail stays
/// readable. Past this the reader stops storing and keeps draining.
pub(super) const MAX_ARTIFACT_OUTPUT: usize = 4 * 1024 * 1024;
/// How long to wait for a timed-out shell to actually die before giving up.
pub(super) const KILL_GRACE: Duration = Duration::from_secs(5);

pub(super) fn bash(
    events: &EventWriter<'_>,
    run: &RunPaths,
    root: &Path,
    step: &StepSpec,
    index: usize,
    command: &str,
) -> Result<(bool, Value)> {
    authorize(
        events,
        root,
        step,
        index,
        "bash",
        "bash",
        None,
        Some(command),
        None,
    )?;
    if events.execution_mode != ExecutionMode::Unrestricted {
        crate::state::validate_shell_boundary(root, &events.workspace.paths.dir)?;
    }
    let sandbox = if events.execution_mode == ExecutionMode::Unrestricted {
        None
    } else {
        Some(
            Sandbox::prepare(root, events.execution_mode)
                .context("could not prepare bash sandbox")?,
        )
    };
    #[cfg(not(target_os = "linux"))]
    let _ = &sandbox;
    let resource_budget =
        ResourceBudget::for_step(step).context("could not prepare bash resource limits")?;
    #[cfg(not(target_os = "linux"))]
    let _ = &resource_budget;
    events.write(
        "tool.started",
        json!({"tool":"bash","command":command,"resourceLimits":resource_budget}),
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
        // Tools are noninteractive; they must never compete with the TUI for
        // keyboard input, including the cancellation key.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if events.execution_mode != ExecutionMode::Unrestricted {
        let temp = root.join(".hyper-tmp");
        if events.execution_mode == ExecutionMode::WorkspaceWrite {
            fs::create_dir_all(&temp)?;
        }
        builder
            .env("TMPDIR", &temp)
            .env("TMP", &temp)
            .env("TEMP", &temp);
    }
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
        if let Some(sandbox) = sandbox {
            unsafe {
                builder.pre_exec(move || sandbox.apply_prepared_in_child());
            }
        }
        if let Some(budget) = resource_budget {
            unsafe {
                builder.pre_exec(move || budget.apply_in_child());
            }
        }
    }
    events.cancellation.check()?;
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
    let stdout_reader = capture(stdout, MAX_ARTIFACT_OUTPUT);
    let stderr_reader = capture(stderr, MAX_ARTIFACT_OUTPUT);
    let mut guard = ProcessGroupGuard::new(child.id());
    let timeout = Duration::from_millis(step.timeout_ms.unwrap_or(DEFAULT_BASH_TIMEOUT_MS));
    let waiting = Instant::now();
    let mut cancelled = false;
    let mut timed_out = false;
    loop {
        if events.cancellation.is_cancelled() {
            cancelled = true;
            break;
        }
        if waiting.elapsed() >= timeout {
            timed_out = true;
            break;
        }
        if child
            .wait_timeout(POLL_INTERVAL.min(timeout.saturating_sub(waiting.elapsed())))?
            .is_some()
        {
            break;
        }
    }
    if timed_out || cancelled {
        kill_process_tree(&mut child);
    }
    let status = match child.wait_timeout(KILL_GRACE)? {
        Some(status) => status,
        None => bail!("bash command could not be terminated after {timeout:?}"),
    };
    let code = status.code().unwrap_or(-1);
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    #[cfg(target_os = "linux")]
    let resource_limit = match signal.or_else(|| (code >= 128).then_some(code - 128)) {
        Some(libc::SIGXCPU) => Some("cpu"),
        Some(libc::SIGXFSZ) => Some("file"),
        _ => None,
    };
    #[cfg(not(target_os = "linux"))]
    let resource_limit: Option<&str> = None;
    if resource_limit.is_some() {
        // A shell can survive a child hitting its limit and can also leave
        // background children holding our output pipes open. End the group.
        kill_group(child.id());
    }
    kill_group(child.id());
    guard.disarm();
    // The child is gone, so both pipes are closed and the readers can finish.
    let (stdout_bytes, stdout_total) = join_capture(stdout_reader, "stdout");
    let (stderr_bytes, stderr_total) = join_capture(stderr_reader, "stderr");
    let full_stdout = String::from_utf8_lossy(&stdout_bytes);
    let full_stderr = String::from_utf8_lossy(&stderr_bytes);
    // The event keeps a bounded slice of each stream, so a command that prints
    // for minutes cannot bloat the audit log. What the reader kept beyond that
    // slice goes to `artifacts/` instead of being dropped, which is what makes
    // the tail of a failing build readable after the run.
    let stdout = truncate_utf8(&full_stdout, MAX_CAPTURED_OUTPUT);
    let stderr = truncate_utf8(&full_stderr, MAX_CAPTURED_OUTPUT);
    let truncated = stdout_total > stdout.len() || stderr_total > stderr.len();
    let invocation = workspace::id();
    let stdout_artifact = write_output_artifact(
        events,
        run,
        &artifact_name(&step.id, index, &invocation, "stdout"),
        &full_stdout,
        stdout_bytes.len(),
        stdout_total,
    )?;
    let stderr_artifact = write_output_artifact(
        events,
        run,
        &artifact_name(&step.id, index, &invocation, "stderr"),
        &full_stderr,
        stderr_bytes.len(),
        stderr_total,
    )?;
    let all = format!("{stdout}{stderr}");
    let duration_ms = started.elapsed().as_millis();
    let mut payload = json!({
        "command": command,
        "cwd": root,
        "exitCode": code,
        "stdout": stdout,
        "stderr": stderr,
        "all": all,
        "timedOut": timed_out,
        "cancelled": cancelled,
        "truncated": truncated,
        "stdoutBytes": stdout_total,
        "stderrBytes": stderr_total,
        "durationMs": duration_ms,
        "signal": signal,
        "resourceLimit": resource_limit,
        "resourceLimits": resource_budget,
    });
    // The model is told where the full output lives, so an observation cut
    // down to a few kilobytes still points at a file it can read back.
    if let Some(path) = stdout_artifact {
        payload["stdoutArtifact"] = json!(path);
    }
    if let Some(path) = stderr_artifact {
        payload["stderrArtifact"] = json!(path);
    }
    let mut event_payload = payload.clone();
    event_payload["tool"] = json!("bash");
    events.write(
        if cancelled {
            "tool.cancelled"
        } else {
            "tool.finished"
        },
        event_payload,
        Some(&step.id),
        Some(index),
    )?;
    events.cancellation.check()?;
    Ok((code == 0, payload))
}

/// The name an artifact is filed under: the step it belongs to, its position
/// in the task, and which stream it holds.
///
/// A step id is authored in `task.json`, so it is reduced to characters that
/// cannot form a path: the name stays one file name inside `artifacts/`.
pub(super) fn artifact_name(step_id: &str, index: usize, invocation: &str, stream: &str) -> String {
    let cleaned: String = step_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let cleaned = if cleaned.is_empty() { "step" } else { &cleaned };
    format!("{cleaned}-{index}-{invocation}-bash-{stream}.log")
}

/// Write one stream's full output where `ha artifacts` can list it.
///
/// Returns the issued artifact path, or `None` when the stream
/// was empty — an artifact directory full of zero-byte files would hide the
/// ones that matter.
pub(super) fn write_output_artifact(
    events: &EventWriter<'_>,
    run: &RunPaths,
    name: &str,
    text: &str,
    kept: usize,
    total: usize,
) -> Result<Option<String>> {
    if text.is_empty() {
        return Ok(None);
    }
    let body = if kept >= total {
        text.to_owned()
    } else {
        format!("{text}\n... [truncated after {total} bytes] ...\n")
    };
    let path = run.artifacts.join(name);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(body.as_bytes())?;
    let reference = path.to_string_lossy().replace('\\', "/");
    events
        .artifacts
        .borrow_mut()
        .insert(reference.clone(), file);
    Ok(Some(reference))
}

/// Read a child pipe to EOF on its own thread, keeping at most `cap` bytes.
///
/// Draining past the cap is mandatory: a reader that stops early fills the pipe
/// buffer and blocks the child exactly like not reading at all.
pub(super) fn capture<R: Read + Send + 'static>(
    mut reader: R,
    cap: usize,
) -> JoinHandle<(Vec<u8>, usize)> {
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

pub(super) fn join_capture(handle: JoinHandle<(Vec<u8>, usize)>, stream: &str) -> (Vec<u8>, usize) {
    handle.join().unwrap_or_else(|_| {
        eprintln!("warning: the {stream} reader thread panicked; output was lost");
        (Vec::new(), 0)
    })
}

/// Kill a whole process group (or process tree on Windows).
pub(super) fn kill_group(pid: u32) {
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

pub(super) fn kill_process_tree(child: &mut Child) {
    kill_group(child.id());
    // The group kill may already have reaped the child; a failure here only
    // means there is nothing left to kill.
    let _ = child.kill();
}

/// Kills the process group if it was not disarmed, so an early return or a
/// panic inside the harness cannot leak a running command.
pub(super) struct ProcessGroupGuard {
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
