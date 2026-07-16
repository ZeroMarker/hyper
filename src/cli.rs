use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::{
    AgentMode, Checkpoint, TaskSpec, Workspace, deepseek::ensure_api_key, get_run_details,
    latest_model_reply, list_runs, prompt_to_task, restore_checkpoint, run_task, tui,
};

#[derive(Parser)]
#[command(
    name = "hyper",
    version,
    about = "Terminal-first agent harness for local coding workflows"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    /// Run a natural-language task directly (defaults to build mode)
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
    /// Use plan mode for a direct prompt
    #[arg(short, long)]
    plan: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Configure the DeepSeek API key
    Config,
    Init,
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
        prompt: String,
    },
    #[command(visible_alias = "b")]
    Build {
        prompt: String,
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
    Tui,
    Artifacts {
        run_id: String,
    },
    Undo {
        run_id: String,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let root = std::env::current_dir()?;
    if cli.command.is_none() {
        ensure_api_key(false)?;
        if cli.prompt.is_empty() {
            return tui::run(root);
        }
        let prompt = cli.prompt.join(" ");
        let mode = if cli.plan {
            AgentMode::Plan
        } else {
            AgentMode::Build
        };
        print_prompt_result(&root, &prompt, mode)?;
        return Ok(());
    }
    match cli.command.expect("command checked above") {
        Commands::Config => ensure_api_key(true)?,
        Commands::Init => {
            let ws = Workspace::open(&root)?;
            println!("initialized {}", ws.paths.dir.display())
        }
        Commands::Validate { task } => {
            let task = read_task(&task)?;
            println!("valid task: {} ({} steps)", task.name, task.steps.len())
        }
        Commands::Run { task } => println!(
            "{}",
            serde_json::to_string_pretty(&run_task(&read_task(&task)?, &root)?)?
        ),
        Commands::Plan { prompt } => {
            ensure_api_key(false)?;
            print_prompt_result(&root, &prompt, AgentMode::Plan)?
        }
        Commands::Build { prompt } => {
            ensure_api_key(false)?;
            print_prompt_result(&root, &prompt, AgentMode::Build)?
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
        Commands::Tui => {
            ensure_api_key(false)?;
            tui::run(root)?
        }
        Commands::Artifacts { run_id } => println!(
            "{}",
            root.join(".harness/runs")
                .join(run_id)
                .join("artifacts")
                .display()
        ),
        Commands::Undo { run_id } => undo(&root, &run_id)?,
    }
    Ok(())
}

fn print_prompt_result(root: &std::path::Path, prompt: &str, mode: AgentMode) -> Result<()> {
    let summary = run_task(&prompt_to_task(prompt, mode), root)?;
    if let Some(content) = latest_model_reply(root, &summary.run_id)? {
        println!("{content}");
    } else {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    }
    Ok(())
}

fn read_task(path: &PathBuf) -> Result<TaskSpec> {
    let task: TaskSpec = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
    )?;
    task.validate()?;
    Ok(task)
}

fn undo(root: &std::path::Path, run_id: &str) -> Result<()> {
    let dir = root.join(".harness/runs").join(run_id).join("checkpoints");
    let mut files = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|x| x.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect::<Vec<_>>();
    files.sort();
    let path = files.last().context("no checkpoints for run")?;
    let cp: Checkpoint = serde_json::from_slice(&fs::read(path)?)?;
    restore_checkpoint(root, &cp)?;
    println!("restored {} from checkpoint {}", cp.target_path, cp.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_prompt_defaults_to_build() {
        let cli = Cli::try_parse_from(["hy", "fix", "the", "bug"]).unwrap();
        assert!(cli.command.is_none());
        assert!(!cli.plan);
        assert_eq!(cli.prompt.join(" "), "fix the bug");
    }

    #[test]
    fn short_plan_flag_accepts_direct_prompt() {
        let cli = Cli::try_parse_from(["hy", "-p", "inspect code"]).unwrap();
        assert!(cli.command.is_none());
        assert!(cli.plan);
        assert_eq!(cli.prompt, ["inspect code"]);
    }

    #[test]
    fn legacy_subcommands_and_aliases_still_parse() {
        let cli = Cli::try_parse_from(["hy", "b", "explain this"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Build { .. })));
    }
}
