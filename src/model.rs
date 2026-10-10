use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Metadata = HashMap<String, Value>;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    Plan,
    #[default]
    Build,
}

impl AgentMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Build => "build",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepSpec {
    pub id: String,
    #[serde(default)]
    pub mode: AgentMode,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<BashResourceLimits>,
    /// Explicit lint/test commands run after the model step, with the failure
    /// fed back to the model for a bounded number of retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifySpec>,
    #[serde(default)]
    pub metadata: Metadata,
}

/// A step's explicit verification: commands that must all exit zero once the
/// model is done. They run through the same policy, approval, sandbox, timeout
/// and resource limits as a `bash` tool call.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifySpec {
    /// Commands run in order; the first non-zero exit stops the attempt.
    pub commands: Vec<String>,
    /// How many times a failing verification is handed back to the model. The
    /// total number of attempts is `retries + 1`.
    #[serde(default = "default_verify_retries")]
    pub retries: usize,
}

fn default_verify_retries() -> usize {
    1
}

/// The most retries a step may configure, so a verification loop stays bounded.
pub const MAX_VERIFY_RETRIES: usize = 5;

/// Instruction prefixes that call a tool directly instead of the model loop.
const DIRECT_TOOL_PREFIXES: [&str; 5] = ["bash:", "read:", "search:", "write:", "edit:"];

/// Optional per-step overrides for the shell's Linux process limits. Each
/// unspecified value retains the default limit.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BashResourceLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub steps: Vec<StepSpec>,
    #[serde(default)]
    pub metadata: Metadata,
}

impl TaskSpec {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("task name must not be empty")
        }
        if self.steps.is_empty() {
            bail!("task must contain at least one step")
        }
        let mut ids = HashSet::new();
        for step in &self.steps {
            if step.id.trim().is_empty() || step.instruction.trim().is_empty() {
                bail!("step id and instruction must not be empty")
            }
            if !ids.insert(&step.id) {
                bail!("duplicate step id: {}", step.id)
            }
            if let Some(limits) = &step.limits {
                for (name, value) in [
                    ("memoryMb", limits.memory_mb),
                    ("fileMb", limits.file_mb),
                    ("cpuSeconds", limits.cpu_seconds),
                ] {
                    if value == Some(0) {
                        bail!("step {}: {name} must be greater than zero", step.id)
                    }
                }
                for (name, value) in [("memoryMb", limits.memory_mb), ("fileMb", limits.file_mb)] {
                    if value.is_some_and(|mb| mb.checked_mul(1024 * 1024).is_none()) {
                        bail!("step {}: {name} is too large", step.id)
                    }
                }
            }
            if let Some(verify) = &step.verify {
                if verify.commands.is_empty() {
                    bail!("step {}: verify must list at least one command", step.id)
                }
                if verify
                    .commands
                    .iter()
                    .any(|command| command.trim().is_empty())
                {
                    bail!("step {}: verify commands must not be empty", step.id)
                }
                if verify.retries > MAX_VERIFY_RETRIES {
                    bail!(
                        "step {}: verify retries must be at most {MAX_VERIFY_RETRIES}",
                        step.id
                    )
                }
                if step.mode == AgentMode::Plan {
                    bail!("step {}: verify requires a build step", step.id)
                }
                // Verification feeds a failure back to the model, so a step that
                // runs one tool directly has nowhere to send it.
                if DIRECT_TOOL_PREFIXES
                    .iter()
                    .any(|prefix| step.instruction.trim_start().starts_with(prefix))
                {
                    bail!(
                        "step {}: verify requires a model step; remove the tool prefix from instruction",
                        step.id
                    )
                }
            }
        }
        Ok(())
    }
    pub fn task_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub error_type: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(default)]
    pub details: Metadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessEvent {
    pub event_id: String,
    pub run_id: String,
    pub task_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub timestamp: String,
    pub step_id: Option<String>,
    pub step_index: Option<usize>,
    pub payload: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub run_id: String,
    pub task_name: String,
    pub status: String,
    pub steps_total: usize,
    pub steps_succeeded: usize,
    pub steps_failed: usize,
    pub started_at: String,
    pub finished_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRow {
    pub run_id: String,
    pub task_id: String,
    pub task_name: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
}

/// One turn of a conversation, in the neutral shape the model APIs use. The
/// session transcript is what a later run replays as context, so it holds the
/// user's prompt and the model's answer rather than the internal tool calls:
/// those live in the run's `events.jsonl`, where the whole trace is recorded.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    /// `user` or `assistant`.
    pub role: String,
    pub content: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub session_id: String,
    /// The first prompt, which is the only summary of a conversation we have
    /// without asking a model for one.
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: usize,
    /// How many runs the conversation has produced.
    pub runs: usize,
}
