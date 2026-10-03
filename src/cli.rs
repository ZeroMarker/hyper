use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::Value;

use crate::{
    AgentMode, CancellationToken, Checkpoint, EventSink, ExecutionMode, Permission, RunOptions,
    RunSummary, TaskSpec, ToolPermissions, Workspace,
    cancellation::{self, Cancelled},
    deepseek::ensure_api_key,
    get_run_details, latest_model_reply, list_runs, prompt_to_task, replay_messages,
    restore_checkpoint, run_task_with_control, tui,
};

#[derive(Parser)]
#[command(
    name = "hyper",
    version,
    about = "Terminal-first agent harness for local coding workflows"
)]
struct Cli {
    /// Stream complete persisted events as JSONL (run/plan/build/direct prompt)
    #[arg(long, global = true)]
    jsonl: bool,
    /// Shell isolation: workspace-write (default), read-only, or unrestricted
    #[arg(long, global = true, value_enum)]
    sandbox: Option<ExecutionMode>,
    /// Decisions for bash/write/edit: allow, ask (default), or deny.
    /// CLI ask fails closed; use the TUI to answer per-invocation questions.
    #[arg(long, global = true, value_enum)]
    approval: Option<Permission>,
    /// Explicit JSON tool decisions; missing entries retain safe defaults
    #[arg(long, global = true, value_name = "FILE")]
    permissions: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Commands>,
    /// Run a natural-language task directly (defaults to build mode)
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
    /// Use plan mode for a direct prompt
    #[arg(short, long)]
    plan: bool,
    /// Continue a conversation: same as `--session`, for a direct prompt
    #[arg(long, value_name = "SESSION_ID")]
    session: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Configure the provider API key, base URL and model
    Config,
    Init,
    /// Print authoritative audit storage paths as JSON
    State,
    /// Import legacy .harness or an external state backup into a fresh workspace
    MigrateState {
        #[arg(long)]
        from: PathBuf,
    },
    #[command(visible_alias = "v")]
    Validate {
        task: PathBuf,
    },
    #[command(visible_alias = "r")]
    Run {
        task: PathBuf,
    },
    #[command(visible_alias = "p")]
    Plan {
        /// Prompt words; unquoted multi-word prompts are joined with spaces
        #[arg(required = true, num_args = 1..)]
        prompt: Vec<String>,
        /// Continue the given conversation instead of starting a new one
        #[arg(long, value_name = "SESSION_ID")]
        session: Option<String>,
    },
    #[command(visible_alias = "b")]
    Build {
        #[arg(required = true, num_args = 1..)]
        prompt: Vec<String>,
        /// Continue the given conversation instead of starting a new one
        #[arg(long, value_name = "SESSION_ID")]
        session: Option<String>,
    },
    #[command(visible_alias = "ls")]
    Runs {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    #[command(visible_alias = "s")]
    Show {
        run_id: String,
    },
    /// List conversations (session id, turns, runs, last update)
    Sessions {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Print a conversation's transcript
    Session {
        session_id: String,
    },
    /// Delete a conversation; the runs it produced are kept
    Forget {
        session_id: String,
    },
    Tui,
    /// Open the TUI continuing the given conversation
    Resume {
        session_id: String,
    },
    Diff {
        run_id: String,
    },
    Artifacts {
        run_id: String,
    },
    Checkpoints {
        run_id: String,
    },
    Restore {
        run_id: String,
        checkpoint_id: String,
    },
    Undo {
        run_id: String,
    },
    /// Rebuild the messages a run sent to the model, printed as JSON
    Replay {
        run_id: String,
    },
    /// Delete the oldest conversations or runs beyond a keep count
    Prune {
        /// How many of the most recent ones to keep
        #[arg(long, value_name = "N")]
        keep: usize,
        /// Prune runs — with their events, artifacts and checkpoints —
        /// instead of conversations
        #[arg(long)]
        runs: bool,
        /// Report what would be deleted without deleting it
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let supports_jsonl = match cli.command.as_ref() {
        Some(Commands::Run { .. } | Commands::Plan { .. } | Commands::Build { .. }) => true,
        None => !cli.prompt.is_empty(),
        _ => false,
    };
    if cli.jsonl && !supports_jsonl {
        bail!("--jsonl requires run, plan, build, or a direct prompt");
    }
    let root = std::env::current_dir()?;
    let execution_mode = match cli.sandbox {
        Some(mode) => mode,
        None => ExecutionMode::from_env()?,
    };
    let mut permissions = if cli.permissions.is_some() || cli.approval.is_some() {
        ToolPermissions::default()
    } else {
        ToolPermissions::from_env()?
    };
    if let Some(path) = &cli.permissions {
        permissions = serde_json::from_slice(
            &fs::read(path)
                .with_context(|| format!("cannot read permissions: {}", path.display()))?,
        )
        .context("invalid permissions file")?;
        permissions.source = format!("file:{}", path.display());
    }
    if let Some(decision) = cli.approval {
        permissions.set_mutations(decision);
        permissions.source.push_str(" + --approval");
    }
    if cli.command.is_none() {
        if !cli.jsonl {
            ensure_api_key(false)?;
        }
        if cli.prompt.is_empty() {
            return tui::run_with_permissions(root, None, execution_mode, permissions.clone());
        }
        let prompt = cli.prompt.join(" ");
        let mode = if cli.plan {
            AgentMode::Plan
        } else {
            AgentMode::Build
        };
        print_prompt_result(
            &root,
            &prompt,
            mode,
            cli.session.as_deref(),
            execution_mode,
            cli.jsonl,
            permissions.clone(),
        )?;
        return Ok(());
    }
    match cli.command.expect("command checked above") {
        Commands::Config => ensure_api_key(true)?,
        Commands::Init => {
            let ws = Workspace::open(&root)?;
            println!("initialized {}", ws.paths.dir.display())
        }
        Commands::State => {
            let workspace = Workspace::open(&root)?;
            println!(
                "{}",
                serde_json::json!({"layoutVersion":1,"workspaceRoot":workspace.paths.root,
                "storageDir":workspace.paths.dir,"runsDir":workspace.paths.runs,"sessionsDir":workspace.paths.sessions})
            );
        }
        Commands::MigrateState { from } => {
            let dir = crate::state::migrate(&root, &from)?;
            let workspace = Workspace::open(&root)?;
            println!(
                "{}",
                serde_json::json!({"storageDir":dir,"runs":workspace.list_runs(usize::MAX)?.len()})
            );
        }
        Commands::Validate { task } => {
            let task = read_task(&task)?;
            println!("valid task: {} ({} steps)", task.name, task.steps.len())
        }
        Commands::Run { task } => {
            let task = read_task(&task)?;
            let cancellation = CancellationToken::new();
            let _signals = cancellation::signals(&cancellation)?;
            let summary = run_task_with_control(
                &task,
                &root,
                RunOptions {
                    cancellation: cancellation.clone(),
                    execution_mode,
                    permissions: permissions.clone(),
                    sink: if cli.jsonl {
                        Some(EventSink::cancellable_stdout(cancellation.clone())?)
                    } else {
                        None
                    },
                    ..RunOptions::default()
                },
            )?;
            if !cli.jsonl {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            }
            ensure_success(&summary)?;
        }
        Commands::Plan { prompt, session } => {
            if !cli.jsonl {
                ensure_api_key(false)?;
            }
            print_prompt_result(
                &root,
                &prompt.join(" "),
                AgentMode::Plan,
                session.as_deref(),
                execution_mode,
                cli.jsonl,
                permissions.clone(),
            )?
        }
        Commands::Build { prompt, session } => {
            if !cli.jsonl {
                ensure_api_key(false)?;
            }
            print_prompt_result(
                &root,
                &prompt.join(" "),
                AgentMode::Build,
                session.as_deref(),
                execution_mode,
                cli.jsonl,
                permissions.clone(),
            )?
        }
        Commands::Runs { limit } => {
            for run in list_runs(&root, limit)? {
                println!(
                    "{}\t{}\t{}\t{}",
                    run.run_id, run.status, run.task_name, run.started_at
                )
            }
        }
        Commands::Show { run_id } => {
            let (run, events) = get_run_details(&root, &run_id)?;
            if run.is_none() {
                bail!("run not found: {run_id}")
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({"run":run,"events":events}))?
            )
        }
        Commands::Sessions { limit } => {
            let workspace = Workspace::open(&root)?;
            for session in workspace.list_sessions(limit)? {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    session.session_id,
                    session.messages,
                    session.runs,
                    session.updated_at,
                    session.title
                )
            }
        }
        Commands::Session { session_id } => {
            let workspace = Workspace::open(&root)?;
            let messages = workspace.session_messages(&session_id)?;
            if messages.is_empty() {
                bail!("session not found or empty: {session_id}")
            }
            for message in messages {
                println!("[{}] {}", message.role, message.timestamp);
                println!("{}", message.content);
                println!();
            }
        }
        Commands::Forget { session_id } => {
            let workspace = Workspace::open(&root)?;
            if !workspace.delete_session(&session_id)? {
                bail!("session not found: {session_id}")
            }
            println!("forgot session {session_id} (its runs are kept)")
        }
        Commands::Tui => {
            ensure_api_key(false)?;
            tui::run_with_permissions(root, None, execution_mode, permissions.clone())?
        }
        Commands::Resume { session_id } => {
            ensure_api_key(false)?;
            let workspace = Workspace::open(&root)?;
            let session = workspace
                .session(&session_id)?
                .with_context(|| format!("session not found: {session_id}"))?;
            tui::run_with_permissions(
                root,
                Some(session.session_id),
                execution_mode,
                permissions.clone(),
            )?
        }
        Commands::Diff { run_id } => diff(&root, &run_id)?,
        Commands::Artifacts { run_id } => artifacts(&root, &run_id)?,
        Commands::Checkpoints { run_id } => {
            let workspace = Workspace::open(&root)?;
            let checkpoints = workspace.list_checkpoints(&run_id)?;
            if checkpoints.is_empty() {
                bail!("run {run_id} has no checkpoints")
            }
            for checkpoint in checkpoints {
                println!(
                    "{}\t{}\t{}",
                    checkpoint.id, checkpoint.target_path, checkpoint.created_at
                )
            }
        }
        Commands::Restore {
            run_id,
            checkpoint_id,
        } => {
            let workspace = Workspace::open(&root)?;
            let checkpoints = workspace.list_checkpoints(&run_id)?;
            let checkpoint = checkpoints
                .into_iter()
                .find(|checkpoint| checkpoint.id == checkpoint_id)
                .with_context(|| format!("no checkpoint {checkpoint_id} for run {run_id}"))?;
            restore_checkpoint(&root, &checkpoint)?;
            println!(
                "restored {} from checkpoint {}",
                checkpoint.target_path, checkpoint.id
            );
        }
        Commands::Undo { run_id } => undo(&root, &run_id)?,
        Commands::Replay { run_id } => {
            let steps = replay_messages(&root, &run_id)?;
            if steps.is_empty() {
                bail!("run {run_id} never reached the model");
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "runId": run_id,
                    "steps": steps
                }))?
            );
        }
        Commands::Prune {
            keep,
            runs,
            dry_run,
        } => {
            let workspace = Workspace::open(&root)?;
            let (pruned, noun) = if runs {
                (workspace.prune_runs(keep, dry_run)?, "runs")
            } else {
                (workspace.prune_sessions(keep, dry_run)?, "conversations")
            };
            if pruned.is_empty() {
                println!("nothing to prune: keeping the {keep} most recent {noun}");
            } else {
                let verb = if dry_run { "would prune" } else { "pruned" };
                println!("{verb} {} {noun}: {}", pruned.len(), pruned.join(", "));
            }
        }
    }
    Ok(())
}

/// Keep machine-readable stdout clean and give cancellation a distinct exit code.
pub fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::ExitCode::from(if error.is::<Cancelled>() { 130 } else { 1 })
        }
    }
}

fn diff(root: &std::path::Path, run_id: &str) -> Result<()> {
    let (run, events) = get_run_details(root, run_id)?;
    if run.is_none() {
        bail!("run not found: {run_id}")
    }
    let mut printed = 0;
    for event in &events {
        if event.event_type != "tool.finished" {
            continue;
        }
        let Some(diff) = event.payload.get("diff").and_then(Value::as_str) else {
            continue;
        };
        let path = event
            .payload
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("?");
        println!("--- {path} ---");
        println!("{diff}");
        printed += 1;
    }
    if printed == 0 {
        println!("(no file diffs recorded for run {run_id})");
    }
    Ok(())
}

fn artifacts(root: &std::path::Path, run_id: &str) -> Result<()> {
    let dir = Workspace::open(root)?
        .paths
        .runs
        .join(run_id)
        .join("artifacts");
    let entries = fs::read_dir(&dir)
        .with_context(|| format!("run {run_id} has no artifacts directory"))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        println!("(no artifacts for run {run_id})");
        return Ok(());
    }
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        println!("{name}\t{size}");
    }
    Ok(())
}

/// Run a direct prompt and print what the model answered. With a session id the
/// run continues that conversation, and the new turn is appended to it; without
/// one the run is standalone, as before.
fn print_prompt_result(
    root: &std::path::Path,
    prompt: &str,
    mode: AgentMode,
    session_id: Option<&str>,
    execution_mode: ExecutionMode,
    jsonl: bool,
    permissions: ToolPermissions,
) -> Result<()> {
    let task = prompt_to_task(prompt, mode);
    let cancellation = CancellationToken::new();
    let _signals = cancellation::signals(&cancellation)?;
    let summary = run_task_with_control(
        &task,
        root,
        RunOptions {
            cancellation: cancellation.clone(),
            execution_mode,
            permissions,
            session_id: session_id.map(str::to_owned),
            sink: if jsonl {
                Some(EventSink::cancellable_stdout(cancellation.clone())?)
            } else {
                None
            },
            ..RunOptions::default()
        },
    )?;
    if jsonl || summary.status == "cancelled" {
        return ensure_success(&summary);
    }
    if let Some(content) = latest_model_reply(root, &summary.run_id)? {
        println!("{content}");
    } else {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    }
    if let Some(session_id) = session_id {
        eprintln!("session {session_id}");
    }
    ensure_success(&summary)
}

/// Fail the process when a run did not finish, so scripts and CI can rely on the
/// exit status instead of parsing stdout.
fn ensure_success(summary: &RunSummary) -> Result<()> {
    if summary.status == "cancelled" {
        return Err(Cancelled.into());
    }
    if summary.status == "finished" {
        return Ok(());
    }
    bail!(
        "run {} {}: {}",
        summary.run_id,
        summary.status,
        summary
            .failure
            .as_ref()
            .map(|failure| failure.message.as_str())
            .unwrap_or("no failure was recorded")
    )
}

fn read_task(path: &PathBuf) -> Result<TaskSpec> {
    let task: TaskSpec = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
    )?;
    task.validate()?;
    Ok(task)
}

fn undo(root: &std::path::Path, run_id: &str) -> Result<()> {
    let dir = Workspace::open(root)?
        .paths
        .runs
        .join(run_id)
        .join("checkpoints");
    let files = fs::read_dir(&dir)
        .with_context(|| format!("run {run_id} has no checkpoint directory"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|x| x == "json"))
        .collect::<Vec<_>>();
    let mut checkpoints = Vec::new();
    for file in &files {
        if let Ok(checkpoint) = serde_json::from_slice::<Checkpoint>(&fs::read(file)?) {
            checkpoints.push((file.clone(), checkpoint));
        }
    }
    if checkpoints.is_empty() {
        bail!("run {run_id} has no checkpoints")
    }
    // Checkpoint file names are random ids, so the latest checkpoint must be
    // picked by creation time, not by filename sort.
    checkpoints.sort_by(|(a_path, a), (b_path, b)| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| {
                let modified = |path: &std::path::Path| {
                    fs::metadata(path)
                        .and_then(|meta| meta.modified())
                        .unwrap_or(std::time::UNIX_EPOCH)
                };
                modified(a_path).cmp(&modified(b_path))
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    let cp = checkpoints
        .last()
        .map(|(_, checkpoint)| checkpoint.clone())
        .expect("checkpoint list is non-empty after the empty check above");
    restore_checkpoint(root, &cp)?;
    println!("restored {} from checkpoint {}", cp.target_path, cp.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StepSpec;
    use crate::run_task;
    use std::{collections::HashMap, fs, thread, time::Duration};
    use tempfile::tempdir;

    fn task(name: &str, instruction: &str) -> TaskSpec {
        TaskSpec {
            id: None,
            name: name.into(),
            steps: vec![StepSpec {
                id: "step".into(),
                mode: AgentMode::Build,
                instruction: instruction.into(),
                tools: None,
                timeout_ms: None,
                limits: None,
                metadata: HashMap::new(),
            }],
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn undo_restores_most_recent_checkpoint() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("demo.txt");
        // First write creates a checkpoint of the empty state, then the file is "A".
        run_task(&task("first", "write:demo.txt\nA"), dir.path()).unwrap();
        thread::sleep(Duration::from_millis(10));
        // Second write creates a checkpoint of "A", then the file becomes "B".
        let summary = run_task(&task("second", "write:demo.txt\nB"), dir.path()).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "B");

        undo(dir.path(), &summary.run_id).unwrap();
        // Undo must restore the newest checkpoint (the pre-second-write state "A"),
        // regardless of random checkpoint file names.
        assert_eq!(fs::read_to_string(&file).unwrap(), "A");
    }

    #[test]
    fn direct_prompt_defaults_to_build() {
        let cli = Cli::try_parse_from(["ha", "fix", "the", "bug"]).unwrap();
        assert!(cli.command.is_none());
        assert!(!cli.plan);
        assert_eq!(cli.prompt.join(" "), "fix the bug");
    }

    #[test]
    fn short_plan_flag_accepts_direct_prompt() {
        let cli = Cli::try_parse_from(["ha", "-p", "inspect code"]).unwrap();
        assert!(cli.command.is_none());
        assert!(cli.plan);
        assert_eq!(cli.prompt, ["inspect code"]);
    }

    #[test]
    fn legacy_subcommands_and_aliases_still_parse() {
        let cli = Cli::try_parse_from(["ha", "b", "explain this"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Build { .. })));
    }

    #[test]
    fn multiword_subcommand_prompts_are_accepted_unquoted() {
        let cli = Cli::try_parse_from(["ha", "plan", "fix", "the", "bug"]).unwrap();
        match cli.command {
            Some(Commands::Plan { prompt, .. }) => assert_eq!(prompt.join(" "), "fix the bug"),
            _ => panic!("expected the plan subcommand"),
        }
    }

    #[test]
    fn sandbox_flag_parses_for_direct_and_subcommand_runs() {
        let direct = Cli::try_parse_from(["ha", "--sandbox", "read-only", "inspect"]).unwrap();
        assert_eq!(direct.sandbox, Some(ExecutionMode::ReadOnly));
        let task =
            Cli::try_parse_from(["ha", "run", "task.json", "--sandbox", "unrestricted"]).unwrap();
        assert_eq!(task.sandbox, Some(ExecutionMode::Unrestricted));
    }

    #[test]
    fn failed_runs_are_reported_as_process_failures() {
        let dir = tempdir().unwrap();
        let summary = run_task(&task("bad", "bash:exit 3"), dir.path()).unwrap();
        assert_eq!(summary.status, "failed");
        assert!(ensure_success(&summary).is_err());

        let summary = run_task(&task("good", "bash:echo ok"), dir.path()).unwrap();
        assert_eq!(summary.status, "finished");
        assert!(ensure_success(&summary).is_ok());
    }
}
