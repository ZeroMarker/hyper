use std::{env, fs, io::IsTerminal, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::{StatusCode, blocking::Client, header::RETRY_AFTER};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AgentMode;

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-v4-flash";
const MAX_API_ATTEMPTS: usize = 3;
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

pub fn system_prompt(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Plan => {
            "You are the planning agent for Hyper, a local coding agent. The current project's context is provided in the user message, and you can call the read-only tools `read` and `search` to inspect files. Analyze the project directly and return a concise, read-only implementation plan. Never ask for a project path when context is present, never invent tool syntax, and never claim to have modified files."
        }
        AgentMode::Build => {
            "You are the coding agent for Hyper, a local Rust coding agent. The current project's context is provided in the user message, and you can call tools to accomplish the task: `read`, `search`, `bash`, `write`, `edit`. Inspect files before editing, prefer `edit` for small changes, and verify your work when possible. When the task is complete, reply with a concise summary of what you did. Never ask for a project path when context is present, and never invent tool syntax."
        }
    }
}

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
    /// Reused across the turns of an agent loop so the TLS handshake and the
    /// connection pool are paid for once per step instead of once per turn.
    client: Client,
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
        let timeout = Duration::from_secs(120);
        Ok(Self {
            api_key,
            base_url: env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into()),
            model: env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
            timeout,
            client: Client::builder().timeout(timeout).build()?,
        })
    }
}

/// Build the chat-completions URL for a base URL, tolerating a trailing slash.
pub fn endpoint(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
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
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolFunction,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolFunction {
    pub name: String,
    pub arguments: String,
}

/// Schema description of a callable tool, sent to the model as an
/// OpenAI-compatible `tools` entry.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelReply {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub model: String,
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
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
    tool_calls: Option<Vec<ToolCall>>,
}

/// Send a chat request with an arbitrary message history. When `tools` is
/// provided, the request advertises OpenAI-compatible function calling and
/// the reply may carry `tool_calls` for the caller to execute.
pub fn chat_messages(
    config: &DeepSeekConfig,
    messages: &[serde_json::Value],
    tools: Option<&[ToolSpec]>,
) -> Result<ModelReply> {
    let url = endpoint(&config.base_url);
    let mut body = json!({ "model": config.model, "messages": messages, "stream": false });
    if let Some(tools) = tools {
        body["tools"] = json!(
            tools
                .iter()
                .map(|tool| json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }
                }))
                .collect::<Vec<_>>()
        );
    }
    let mut successful_body = None;
    for attempt in 0..MAX_API_ATTEMPTS {
        let response = match config
            .client
            .post(&url)
            .bearer_auth(&config.api_key)
            .json(&body)
            .send()
        {
            Ok(response) => response,
            Err(_error) if attempt + 1 < MAX_API_ATTEMPTS => {
                std::thread::sleep(retry_delay(attempt, None));
                continue;
            }
            Err(error) => return Err(error).context("failed to call DeepSeek API"),
        };
        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs);
        let response_body = response.text()?;
        if status.is_success() {
            successful_body = Some(response_body);
            break;
        }
        if retryable_status(status) && attempt + 1 < MAX_API_ATTEMPTS {
            std::thread::sleep(retry_delay(attempt, retry_after));
            continue;
        }
        bail!("DeepSeek API returned {status}: {response_body}")
    }
    let body = successful_body.context("DeepSeek API request exhausted all attempts")?;
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
        tool_calls: message.tool_calls.unwrap_or_default(),
    })
}

fn retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

fn retry_delay(attempt: usize, retry_after: Option<Duration>) -> Duration {
    retry_after
        .unwrap_or_else(|| INITIAL_RETRY_DELAY.saturating_mul(1_u32 << attempt.min(4)))
        .min(MAX_RETRY_DELAY)
}

/// One-shot chat request without tool calling (kept for simple prompts).
pub fn chat(
    config: &DeepSeekConfig,
    prompt: &str,
    mode: AgentMode,
    workspace_context: &str,
) -> Result<ModelReply> {
    let messages = vec![
        json!({ "role": "system", "content": system_prompt(mode) }),
        json!({ "role": "user", "content": format!(
            "<workspace_context>\n{workspace_context}\n</workspace_context>\n\n<request>\n{prompt}\n</request>"
        ) }),
    ];
    chat_messages(config, &messages, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[test]
    fn defaults_match_current_deepseek_api() {
        assert_eq!(DEFAULT_BASE_URL, "https://api.deepseek.com");
        assert_eq!(DEFAULT_MODEL, "deepseek-v4-flash");
    }

    /// Asserting the constants against themselves would prove nothing: check the
    /// URL the client actually builds, including the trailing-slash override
    /// users are told to set through `DEEPSEEK_BASE_URL`.
    #[test]
    fn chat_completions_url_is_built_from_the_base_url() {
        assert_eq!(
            endpoint(DEFAULT_BASE_URL),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            endpoint("https://api.deepseek.com/"),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:8080/v1"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
    }

    #[test]
    fn only_transient_api_statuses_are_retried() {
        assert!(retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(retryable_status(StatusCode::REQUEST_TIMEOUT));
        assert!(!retryable_status(StatusCode::BAD_REQUEST));
        assert!(!retryable_status(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn retry_after_overrides_exponential_delay() {
        assert_eq!(retry_delay(0, None), Duration::from_millis(250));
        assert_eq!(retry_delay(1, None), Duration::from_millis(500));
        assert_eq!(
            retry_delay(0, Some(Duration::from_secs(2))),
            Duration::from_secs(2)
        );
        assert_eq!(
            retry_delay(0, Some(Duration::from_secs(120))),
            MAX_RETRY_DELAY
        );
    }

    #[test]
    fn transient_response_is_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let responses = [
                (
                    "503 Service Unavailable",
                    r#"{"error":{"message":"try again"}}"#,
                    "Retry-After: 0\r\n",
                ),
                (
                    "200 OK",
                    r#"{"model":"test-model","choices":[{"message":{"content":"ok"}}]}"#,
                    "",
                ),
            ];
            for (status, body, extra_headers) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{extra_headers}\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let timeout = Duration::from_secs(2);
        let config = DeepSeekConfig {
            api_key: "test-key".into(),
            base_url: format!("http://{address}"),
            model: "test-model".into(),
            timeout,
            client: Client::builder().timeout(timeout).build().unwrap(),
        };

        let reply = chat_messages(&config, &[json!({"role":"user","content":"hi"})], None).unwrap();
        assert_eq!(reply.content, "ok");
        server.join().unwrap();
    }
}
