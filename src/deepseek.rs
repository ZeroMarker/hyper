use std::{env, fs, io::IsTerminal, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AgentMode;

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-v4-flash";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct StoredConfig {
    deepseek_api_key: String,
}

#[derive(Clone, Debug)]
pub struct DeepSeekConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub timeout: Duration,
}

impl DeepSeekConfig {
    pub fn from_env() -> Result<Self> {
        let api_key = env::var("DEEPSEEK_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .or_else(|| read_stored_key().ok().flatten())
            .context("DeepSeek API key is not configured; run `hy config`")?;
        if api_key.trim().is_empty() {
            bail!("DEEPSEEK_API_KEY must not be empty")
        }
        Ok(Self {
            api_key,
            base_url: env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into()),
            model: env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
            timeout: Duration::from_secs(120),
        })
    }
}

pub fn ensure_api_key(force: bool) -> Result<()> {
    if !force && DeepSeekConfig::from_env().is_ok() {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "DeepSeek API key is not configured; run `hy config` in a terminal or set DEEPSEEK_API_KEY"
        )
    }
    println!("首次使用需要配置 DeepSeek API Key。");
    println!("密钥将保存在本机用户配置目录，输入内容不会显示。");
    let key = rpassword::prompt_password("DeepSeek API Key: ")?;
    let key = key.trim();
    if key.is_empty() {
        bail!("API key must not be empty")
    }
    save_key(key)?;
    println!("DeepSeek API Key 已保存到 {}", config_path()?.display());
    Ok(())
}

fn config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("could not determine the user configuration directory")?;
    Ok(dir.join("hyper").join("config.json"))
}

fn read_stored_key() -> Result<Option<String>> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let config: StoredConfig = serde_json::from_slice(&fs::read(path)?)?;
    Ok((!config.deepseek_api_key.trim().is_empty()).then_some(config.deepseek_api_key))
}

fn save_key(key: &str) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_vec_pretty(&StoredConfig {
        deepseek_api_key: key.into(),
    })?;
    fs::write(&path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelReply {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub model: String,
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Deserialize)]
struct ChatResponse {
    model: String,
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
    reasoning_content: Option<String>,
}

pub fn chat(
    config: &DeepSeekConfig,
    prompt: &str,
    mode: AgentMode,
    workspace_context: &str,
) -> Result<ModelReply> {
    let system = match mode {
        AgentMode::Plan => {
            "You are the planning model for Hyper, a local coding agent. The current project's context is provided in the user message. Analyze that project directly and return a concise, read-only implementation plan. Never ask for a project path when context is present. Do not invent tool prefixes or claim to have modified files."
        }
        AgentMode::Build => {
            "You are the default coding model for Hyper, a local Rust coding agent. The current project's context is provided in the user message. Use it directly when answering. Never ask for a project path when context is present, and never invent unsupported tool syntax. Be concrete and concise."
        }
    };
    let endpoint = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let response = Client::builder()
        .timeout(config.timeout)
        .build()?
        .post(endpoint)
        .bearer_auth(&config.api_key)
        .json(&json!({
            "model": config.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": format!(
                    "<workspace_context>\n{}\n</workspace_context>\n\n<request>\n{}\n</request>",
                    workspace_context,
                    prompt
                )}
            ],
            "stream": false
        }))
        .send()
        .context("failed to call DeepSeek API")?;
    let status = response.status();
    let body = response.text()?;
    if !status.is_success() {
        bail!("DeepSeek API returned {status}: {body}")
    }
    let parsed: ChatResponse = serde_json::from_str(&body)
        .with_context(|| format!("invalid DeepSeek response: {body}"))?;
    let message = parsed
        .choices
        .into_iter()
        .next()
        .context("DeepSeek response contained no choices")?
        .message;
    Ok(ModelReply {
        content: message.content.unwrap_or_default(),
        reasoning_content: message.reasoning_content,
        model: parsed.model,
        usage: parsed.usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_current_deepseek_api() {
        assert_eq!(DEFAULT_BASE_URL, "https://api.deepseek.com");
        assert_eq!(DEFAULT_MODEL, "deepseek-v4-flash");
    }
}
