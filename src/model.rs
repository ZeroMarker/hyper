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
    #[serde(default)]
    pub metadata: Metadata,
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
