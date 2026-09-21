use std::{
    env, fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::{
    StatusCode,
    blocking::Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue, RETRY_AFTER},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{AgentMode, workspace};

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-v4-flash";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_API_ATTEMPTS: usize = 3;
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
/// Identifies the agent rather than the HTTP library it is built on, which is
/// what OpenCode Go asks of the clients it validates.
const USER_AGENT: &str = concat!("hyper/", env!("CARGO_PKG_VERSION"));
/// OpenCode Go rejects a request without it (400 `MissingSessionID`) and uses it
/// to route and prompt-cache the turns that belong to one conversation.
const SESSION_HEADER: &str = "x-opencode-session";
/// The Messages format is the only one that authenticates with a key header
/// instead of a bearer token, and it requires the version header on every call.
const API_KEY_HEADER: &str = "x-api-key";
const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Both non-chat formats want an output budget: the gateway rejects Messages
/// without one, and a budget that is too small is spent on thinking tokens
/// before any answer or tool call comes back.
const MAX_OUTPUT_TOKENS: u64 = 8192;

/// The wire protocol a model is served over. On OpenCode Go all three hang off
/// one base URL, so the protocol is a property of the model rather than of the
/// endpoint, and the request and reply translation below depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Protocol {
    /// OpenAI chat completions, which DeepSeek itself and most gateways speak.
    #[default]
    Chat,
    /// OpenAI Responses, used by the Grok, GPT and Muse models on OpenCode Go.
    Responses,
    /// Anthropic Messages, used by the MiniMax and Qwen models on OpenCode Go.
    Messages,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }

    /// Read a configured protocol name. An unrecognised name is rejected rather
    /// than ignored, so a typo cannot silently select another wire format.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "chat" => Some(Self::Chat),
            "responses" => Some(Self::Responses),
            "messages" => Some(Self::Messages),
            _ => None,
        }
    }

    /// The path the protocol is served under, relative to the base URL.
    pub fn path(self) -> &'static str {
        match self {
            Self::Chat => "chat/completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }
}

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

/// The user configuration written by `hyper config`. Everything is optional so a
/// file an older version wrote (which only held the key) still loads, and the
/// environment keeps overriding whatever is stored here.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct StoredConfig {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    deepseek_api_key: String,
    /// Points the client at another OpenAI-compatible service, such as
    /// `https://opencode.ai/zen/go/v1` for OpenCode Go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    /// Pins the wire protocol (`chat` | `responses` | `messages`). Left absent
    /// when the model name is a good enough signal, which is only true on
    /// OpenCode Go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protocol: Option<String>,
}

/// The effective provider settings: the API key, the base URL its requests go to
/// and the model they ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Settings {
    api_key: String,
    base_url: String,
    model: String,
    protocol: Protocol,
}

#[derive(Clone, Debug)]
pub struct DeepSeekConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// How the request and the reply are shaped on the wire.
    pub protocol: Protocol,
    /// The service `base_url` points at, for the audit log: a run against
    /// OpenCode Go must not be recorded as a DeepSeek call.
    pub provider: String,
    pub timeout: Duration,
    /// Reused across the turns of an agent loop so the TLS handshake and the
    /// connection pool are paid for once per step instead of once per turn.
    client: Client,
}

impl DeepSeekConfig {
    pub fn from_env() -> Result<Self> {
        let stored = read_stored_config(&config_path()?)?;
        let settings = resolve_settings(|key| env::var(key).ok(), stored)?;
        Self::new(settings)
    }

    fn new(settings: Settings) -> Result<Self> {
        Self::with_timeout(settings, REQUEST_TIMEOUT)
    }

    fn with_timeout(settings: Settings, timeout: Duration) -> Result<Self> {
        // One client serves one step, so this id groups exactly the turns of one
        // conversation, which is what providers like OpenCode Go route on.
        let session = format!("hyper-{}", workspace::id());
        let mut headers = HeaderMap::new();
        headers.insert(SESSION_HEADER, HeaderValue::from_str(&session)?);
        // Kept out of anything that logs headers, which `bearer_auth` does too.
        let credential = |value: String| -> Result<HeaderValue> {
            let mut header = HeaderValue::from_str(&value)?;
            header.set_sensitive(true);
            Ok(header)
        };
        match settings.protocol {
            // The two OpenAI formats authenticate the same way.
            Protocol::Chat | Protocol::Responses => {
                headers.insert(
                    AUTHORIZATION,
                    credential(format!("Bearer {}", settings.api_key))?,
                );
            }
            Protocol::Messages => {
                // A bearer token is answered with `Missing API key` here.
                headers.insert(API_KEY_HEADER, credential(settings.api_key.clone())?);
                headers.insert(
                    ANTHROPIC_VERSION_HEADER,
                    HeaderValue::from_static(ANTHROPIC_VERSION),
                );
            }
        }
        Ok(Self {
            provider: provider_name(&settings.base_url),
            api_key: settings.api_key,
            base_url: settings.base_url,
            model: settings.model,
            protocol: settings.protocol,
            timeout,
            client: Client::builder()
                .timeout(timeout)
                .user_agent(USER_AGENT)
                .default_headers(headers)
                .build()?,
        })
    }
}

/// Precedence: the environment overrides the stored file, which overrides the
/// built-in defaults. `env` is a parameter rather than `std::env::var` so the
/// rule can be tested without touching the process environment.
fn resolve_settings(
    env: impl Fn(&str) -> Option<String>,
    stored: Option<StoredConfig>,
) -> Result<Settings> {
    let stored = stored.unwrap_or_default();
    let value = |value: Option<String>| {
        value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let api_key = value(env("DEEPSEEK_API_KEY"))
        .or_else(|| value(Some(stored.deepseek_api_key)))
        .context("DeepSeek API key is not configured; run `hyper config`")?;
    let base_url = value(env("DEEPSEEK_BASE_URL"))
        .or_else(|| value(stored.base_url))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
    let model = value(env("DEEPSEEK_MODEL"))
        .or_else(|| value(stored.model))
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
    let protocol = resolve_protocol(
        value(env("DEEPSEEK_PROTOCOL")).or_else(|| value(stored.protocol.clone())),
        &base_url,
        &model,
    )?;
    Ok(Settings {
        api_key,
        base_url,
        model,
        protocol,
    })
}

/// Pick the wire protocol: an explicit choice — the environment, then the stored
/// file — wins over what the base URL and the model name imply.
fn resolve_protocol(configured: Option<String>, base_url: &str, model: &str) -> Result<Protocol> {
    match configured
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        Some(name) => Protocol::parse(&name).with_context(|| {
            format!("unknown protocol {name:?}; expected chat, responses or messages")
        }),
        None => Ok(detect_protocol(base_url, model)),
    }
}

/// Which protocol a model is served over. On OpenCode Go all three formats hang
/// off one base URL and `/v1/models` does not say which one a model speaks, so
/// the model name is the only signal there is; anywhere else the chat-completions
/// endpoint is the only thing this client can assume.
fn detect_protocol(base_url: &str, model: &str) -> Protocol {
    let (host, _) = split_base_url(base_url);
    if !host.ends_with("opencode.ai") {
        return Protocol::Chat;
    }
    let model = model.trim().to_ascii_lowercase();
    let starts_with = |prefixes: &[&str]| prefixes.iter().any(|prefix| model.starts_with(prefix));
    if starts_with(&["minimax-", "qwen3.6-plus", "qwen3.7-", "qwen3.8-"]) {
        Protocol::Messages
    } else if starts_with(&["grok-", "gpt-", "muse-spark-"]) {
        Protocol::Responses
    } else {
        Protocol::Chat
    }
}

/// Split a base URL into its lowercased host and the path under it, without
/// pulling in a URL crate the client does not otherwise need.
fn split_base_url(base_url: &str) -> (String, &str) {
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    (host, path)
}

/// The service a base URL belongs to, as named in the audit log. Unknown hosts
/// are reported as themselves so the log never claims a provider it cannot
/// recognise.
fn provider_name(base_url: &str) -> String {
    let (host, path) = split_base_url(base_url);
    if host.ends_with("deepseek.com") {
        "deepseek".into()
    } else if host.ends_with("opencode.ai") {
        // Zen serves the Go subscription under /zen/go and pay-as-you-go under
        // /zen, and OpenCode Go requires the session header described above.
        if path.starts_with("zen/go") {
            "opencode-go".into()
        } else {
            "opencode-zen".into()
        }
    } else {
        host
    }
}

/// Build the request URL for a base URL, tolerating a trailing slash. The path
/// depends on the protocol the model speaks.
pub fn endpoint(base_url: &str, protocol: Protocol) -> String {
    format!("{}/{}", base_url.trim_end_matches('/'), protocol.path())
}

/// Interactively collect the provider settings, or check the ones already in
/// place. The key stays hidden while it is typed; the base URL and the model are
/// shown with the defaults a bare Enter accepts.
pub fn ensure_api_key(force: bool) -> Result<()> {
    if !force && DeepSeekConfig::from_env().is_ok() {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "DeepSeek API key is not configured; run `hyper config` in a terminal or set DEEPSEEK_API_KEY"
        )
    }
    println!("首次使用需要配置模型服务的 API Key。");
    println!("配置将保存在本机用户配置目录，输入内容不会显示。");
    let key = rpassword::prompt_password("API Key: ")?;
    let key = key.trim();
    if key.is_empty() {
        bail!("API key must not be empty")
    }
    let stored = read_stored_config(&config_path()?)?.unwrap_or_default();
    let base_url = prompt_line(
        "API base URL",
        stored.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL),
        "https://opencode.ai/zen/go/v1 用于 OpenCode Go",
    )?;
    let model = prompt_line(
        "Model",
        stored.model.as_deref().unwrap_or(DEFAULT_MODEL),
        "例如 deepseek-v4-pro",
    )?;
    // The wizard stays at three questions, so the protocol is reported rather
    // than asked for: a pinned one keeps, otherwise the model name decides.
    let configured = env::var("DEEPSEEK_PROTOCOL")
        .ok()
        .or_else(|| stored.protocol.clone())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let protocol = resolve_protocol(configured.clone(), &base_url, &model)?;
    let path = config_path()?;
    save_config(
        &path,
        &StoredConfig {
            deepseek_api_key: key.into(),
            base_url: Some(base_url),
            model: Some(model),
            // An explicit choice survives a reconfiguration.
            protocol: stored.protocol,
        },
    )?;
    println!("配置已保存到 {}", path.display());
    println!(
        "协议 {}（{}）",
        protocol.as_str(),
        if configured.is_some() {
            "配置指定"
        } else {
            "自动探测"
        }
    );
    Ok(())
}

/// Read one line, falling back to `default` when the user just presses Enter.
fn prompt_line(label: &str, default: &str, hint: &str) -> Result<String> {
    print!("{label} [{default}]（{hint}）: ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    Ok(if line.is_empty() {
        default.to_owned()
    } else {
        line.to_owned()
    })
}

fn config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("could not determine the user configuration directory")?;
    Ok(dir.join("hyper").join("config.json"))
}

fn read_stored_config(path: &Path) -> Result<Option<StoredConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let config: StoredConfig = serde_json::from_slice(&fs::read(path)?)
        .with_context(|| format!("invalid configuration file {}", path.display()))?;
    Ok(Some(config))
}

fn save_config(path: &Path, config: &StoredConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_vec_pretty(config)?;
    fs::write(path, content)?;
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

/// One item of a Responses `output` array. The three kinds this client reads are
/// told apart by `type`; the fields the other kinds use stay absent.
#[derive(Deserialize)]
struct ResponsesItem {
    #[serde(rename = "type")]
    item_type: String,
    /// `message` items carry the assistant text.
    #[serde(default)]
    content: Vec<ResponsesPart>,
    /// `reasoning` items carry the summary the API exposes in place of the raw
    /// chain of thought.
    #[serde(default)]
    summary: Vec<ResponsesPart>,
    /// `function_call` items carry these, and `call_id` is what the follow-up
    /// `function_call_output` has to quote.
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesPart {
    #[serde(rename = "type")]
    part_type: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesResponse {
    model: String,
    #[serde(default)]
    output: Vec<ResponsesItem>,
    usage: Option<ResponsesUsage>,
}

/// The Responses format counts tokens under names of its own.
#[derive(Deserialize)]
struct ResponsesUsage {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

/// One block of a Messages `content` array. `thinking` blocks are read for the
/// reasoning display but never replayed, which the API accepts.
#[derive(Deserialize)]
struct MessagesBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
    thinking: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct MessagesResponse {
    model: String,
    content: Vec<MessagesBlock>,
    usage: Option<MessagesUsage>,
}

/// The Messages format reports no total, so this client adds up the two counts.
#[derive(Deserialize)]
struct MessagesUsage {
    input_tokens: u64,
    output_tokens: u64,
}

/// The text a neutral message carries, which is always a plain string; a history
/// may leave it out, and an empty string is then the right answer.
fn message_text(message: &serde_json::Value) -> String {
    message["content"].as_str().unwrap_or_default().to_owned()
}

/// Decode the JSON string the neutral shape stores tool arguments in. A history
/// written by hand may hold something else, and an empty object keeps the
/// request valid instead of failing the turn.
fn decode_arguments(arguments: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(arguments.as_str().unwrap_or_default())
        .ok()
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Send a request with an arbitrary message history. When `tools` is provided,
/// the request advertises function calling and the reply may carry `tool_calls`
/// for the caller to execute. The history and the reply are always in that one
/// neutral, OpenAI-chat-shaped vocabulary, and the protocol the provider speaks
/// is translated here and nowhere else.
pub fn chat_messages(
    config: &DeepSeekConfig,
    messages: &[serde_json::Value],
    tools: Option<&[ToolSpec]>,
) -> Result<ModelReply> {
    match config.protocol {
        Protocol::Chat => openai_chat(config, messages, tools),
        Protocol::Responses => openai_responses(config, messages, tools),
        Protocol::Messages => anthropic_messages(config, messages, tools),
    }
}

fn openai_chat(
    config: &DeepSeekConfig,
    messages: &[serde_json::Value],
    tools: Option<&[ToolSpec]>,
) -> Result<ModelReply> {
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
    let response = post(config, &body)?;
    let parsed: ChatResponse = serde_json::from_str(&response)
        .with_context(|| format!("invalid {} response: {response}", config.provider))?;
    let message = parsed
        .choices
        .into_iter()
        .next()
        .with_context(|| format!("{} response contained no choices", config.provider))?
        .message;
    Ok(ModelReply {
        content: message.content.unwrap_or_default(),
        reasoning_content: message.reasoning_content,
        model: parsed.model,
        usage: parsed.usage,
        tool_calls: message.tool_calls.unwrap_or_default(),
    })
}

/// Translate the neutral history into a Responses `input` array plus the
/// `instructions` string its system messages collapse into. The gateway needs
/// the previous turn's `function_call` replayed ahead of its
/// `function_call_output`, or the model is handed output for a call it never
/// made.
fn responses_input(messages: &[serde_json::Value]) -> (String, Vec<serde_json::Value>) {
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for message in messages {
        match message["role"].as_str().unwrap_or_default() {
            "system" => instructions.push(message_text(message)),
            "assistant" => {
                // An assistant turn that only called tools has no text; the
                // `function_call` items below carry everything it said.
                let text = message_text(message);
                if !text.is_empty() {
                    input.push(json!({ "role": "assistant", "content": text }));
                }
                for call in message["tool_calls"].as_array().into_iter().flatten() {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call["id"],
                        "name": call["function"]["name"],
                        "arguments": call["function"]["arguments"],
                    }));
                }
            }
            "tool" => input.push(json!({
                "type": "function_call_output",
                "call_id": message["tool_call_id"],
                "output": message_text(message),
            })),
            _ => input.push(json!({ "role": "user", "content": message_text(message) })),
        }
    }
    (instructions.join("\n\n"), input)
}

fn openai_responses(
    config: &DeepSeekConfig,
    messages: &[serde_json::Value],
    tools: Option<&[ToolSpec]>,
) -> Result<ModelReply> {
    let (instructions, input) = responses_input(messages);
    let mut body = json!({
        "model": config.model,
        "input": input,
        "max_output_tokens": MAX_OUTPUT_TOKENS,
    });
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    // This format flattens the OpenAI tool wrapper: the name and the schema sit
    // beside `type` instead of under `function`.
    if let Some(tools) = tools {
        body["tools"] = json!(
            tools
                .iter()
                .map(|tool| json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }))
                .collect::<Vec<_>>()
        );
    }
    let response = post(config, &body)?;
    let parsed: ResponsesResponse = serde_json::from_str(&response)
        .with_context(|| format!("invalid {} response: {response}", config.provider))?;
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    for item in parsed.output {
        match item.item_type.as_str() {
            "message" => {
                for part in item.content {
                    if part.part_type == "output_text" {
                        content.push_str(part.text.as_deref().unwrap_or_default());
                    }
                }
            }
            "reasoning" => {
                for part in item.summary {
                    reasoning.push_str(part.text.as_deref().unwrap_or_default());
                }
            }
            "function_call" => tool_calls.push(ToolCall {
                id: item.call_id.unwrap_or_default(),
                call_type: "function".into(),
                function: ToolFunction {
                    name: item.name.unwrap_or_default(),
                    arguments: item.arguments.unwrap_or_default(),
                },
            }),
            _ => {}
        }
    }
    Ok(ModelReply {
        content,
        reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
        model: parsed.model,
        usage: parsed.usage.map(|usage| Usage {
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
        }),
        tool_calls,
    })
}

/// Translate the neutral history into a Messages `system` string plus the
/// `messages` turns, whose tool results live inside user turns.
fn messages_turns(messages: &[serde_json::Value]) -> (String, Vec<serde_json::Value>) {
    let mut system = Vec::new();
    let mut turns = Vec::new();
    for message in messages {
        match message["role"].as_str().unwrap_or_default() {
            "system" => system.push(message_text(message)),
            "assistant" => {
                let mut blocks = Vec::new();
                let text = message_text(message);
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                for call in message["tool_calls"].as_array().into_iter().flatten() {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call["id"],
                        "name": call["function"]["name"],
                        // This format wants the decoded object, while the
                        // neutral shape carries the arguments as a JSON string.
                        "input": decode_arguments(&call["function"]["arguments"]),
                    }));
                }
                if !blocks.is_empty() {
                    turns.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            "tool" => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": message["tool_call_id"],
                    "content": message_text(message),
                });
                // Results belong to a user turn, and the API expects the results
                // of one parallel round inside a single turn, so consecutive
                // `tool` messages extend the turn rather than open a new one.
                let extend = turns
                    .last_mut()
                    .filter(|turn| {
                        turn["role"] == "user" && turn["content"][0]["type"] == "tool_result"
                    })
                    .and_then(|turn| turn["content"].as_array_mut());
                match extend {
                    Some(blocks) => blocks.push(block),
                    None => turns.push(json!({ "role": "user", "content": [block] })),
                }
            }
            _ => turns.push(json!({
                "role": "user",
                "content": [{ "type": "text", "text": message_text(message) }],
            })),
        }
    }
    (system.join("\n\n"), turns)
}

fn anthropic_messages(
    config: &DeepSeekConfig,
    messages: &[serde_json::Value],
    tools: Option<&[ToolSpec]>,
) -> Result<ModelReply> {
    let (system, turns) = messages_turns(messages);
    let mut body = json!({
        "model": config.model,
        "messages": turns,
        // Required: without it the gateway rejects the request outright.
        "max_tokens": MAX_OUTPUT_TOKENS,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    // Here the schema field is `input_schema`, with no `function` wrapper.
    if let Some(tools) = tools {
        body["tools"] = json!(
            tools
                .iter()
                .map(|tool| json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters,
                }))
                .collect::<Vec<_>>()
        );
    }
    let response = post(config, &body)?;
    let parsed: MessagesResponse = serde_json::from_str(&response)
        .with_context(|| format!("invalid {} response: {response}", config.provider))?;
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    for block in parsed.content {
        match block.block_type.as_str() {
            "text" => content.push_str(block.text.as_deref().unwrap_or_default()),
            "thinking" => reasoning.push_str(block.thinking.as_deref().unwrap_or_default()),
            "tool_use" => tool_calls.push(ToolCall {
                id: block.id.unwrap_or_default(),
                call_type: "function".into(),
                function: ToolFunction {
                    name: block.name.unwrap_or_default(),
                    // The neutral shape stores arguments as a JSON string, and
                    // `Value` renders exactly what it just deserialised.
                    arguments: block.input.unwrap_or_else(|| json!({})).to_string(),
                },
            }),
            _ => {}
        }
    }
    Ok(ModelReply {
        content,
        reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
        model: parsed.model,
        usage: parsed.usage.map(|usage| Usage {
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: usage.input_tokens + usage.output_tokens,
        }),
        tool_calls,
    })
}

/// POST one JSON body to the configured endpoint and return the response body of
/// the first attempt that succeeds. Every wire format shares this transport, so
/// the retry rules stay in one place instead of three.
fn post(config: &DeepSeekConfig, body: &serde_json::Value) -> Result<String> {
    // The path depends on the protocol the model speaks, so it is built from the
    // config the caller is holding rather than cached on the client.
    let url = endpoint(&config.base_url, config.protocol);
    let mut successful_body = None;
    for attempt in 0..MAX_API_ATTEMPTS {
        let response = match config.client.post(&url).json(body).send() {
            Ok(response) => response,
            Err(_error) if attempt + 1 < MAX_API_ATTEMPTS => {
                std::thread::sleep(retry_delay(attempt, None));
                continue;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to call the {} API", config.provider));
            }
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
        bail!("{} API returned {status}: {response_body}", config.provider)
    }
    successful_body
        .with_context(|| format!("{} API request exhausted all attempts", config.provider))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    use tempfile::tempdir;

    /// Answers one request with `body`, handing back the raw request it read, so
    /// a test can inspect the headers and the JSON the client really sent.
    fn serve_one(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            request
        });
        (format!("http://{address}"), handle)
    }

    /// Read a whole request. The client sends the body after the headers, so a
    /// single read can return a prefix of it, and a test that inspected such a
    /// prefix would pass or fail by luck.
    fn read_request(stream: &mut impl Read) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected = None;
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&request);
            let Some((headers, body)) = text.split_once("\r\n\r\n") else {
                continue;
            };
            let length = *expected.get_or_insert_with(|| {
                headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0)
            });
            if body.len() >= length {
                break;
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    /// The JSON body of a request captured by [`serve_one`].
    fn request_body(request: &str) -> serde_json::Value {
        let body = request.split_once("\r\n\r\n").map_or("", |(_, body)| body);
        serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("no JSON body in {request:?}: {error}"))
    }

    fn settings(protocol: Protocol, api_key: &str, base_url: &str, model: &str) -> Settings {
        Settings {
            api_key: api_key.into(),
            base_url: base_url.into(),
            model: model.into(),
            protocol,
        }
    }

    #[test]
    fn defaults_match_current_deepseek_api() {
        assert_eq!(DEFAULT_BASE_URL, "https://api.deepseek.com");
        assert_eq!(DEFAULT_MODEL, "deepseek-v4-flash");
    }

    /// Asserting the constants against themselves would prove nothing: check the
    /// URL the client actually builds for each protocol, including the
    /// trailing-slash override users are told to set through `DEEPSEEK_BASE_URL`.
    #[test]
    fn the_request_path_follows_the_protocol() {
        assert_eq!(
            endpoint(DEFAULT_BASE_URL, Protocol::Chat),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            endpoint("https://api.deepseek.com/", Protocol::Chat),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            endpoint("https://opencode.ai/zen/go/v1/", Protocol::Responses),
            "https://opencode.ai/zen/go/v1/responses"
        );
        assert_eq!(
            endpoint("https://opencode.ai/zen/go/v1", Protocol::Messages),
            "https://opencode.ai/zen/go/v1/messages"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:8080/v1", Protocol::Chat),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
    }

    #[test]
    fn protocol_names_round_trip_and_reject_typos() {
        for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            assert_eq!(Protocol::parse(protocol.as_str()), Some(protocol));
        }
        // The name is typed by hand into an environment variable.
        assert_eq!(Protocol::parse(" Messages "), Some(Protocol::Messages));
        assert_eq!(Protocol::parse("responses"), Some(Protocol::Responses));
        assert_eq!(Protocol::parse("respones"), None);
        assert_eq!(Protocol::default(), Protocol::Chat);
    }

    /// OpenCode Go serves three wire formats under one base URL and `/v1/models`
    /// does not say which one a model speaks, so the model name has to.
    #[test]
    fn detect_protocol_follows_the_opencode_model_family() {
        let at_opencode = |model: &str| detect_protocol("https://opencode.ai/zen/go/v1", model);
        for model in [
            "minimax-m3",
            "minimax-m2.7",
            "minimax-m2.5",
            "qwen3.6-plus",
            "qwen3.7-max",
            "qwen3.7-plus",
            "qwen3.8-flash",
        ] {
            assert_eq!(at_opencode(model), Protocol::Messages, "{model}");
        }
        for model in [
            "grok-4.6",
            "gpt-5.6-luna",
            "muse-spark-1.2",
            "muse-spark-1.3",
        ] {
            assert_eq!(at_opencode(model), Protocol::Responses, "{model}");
        }
        for model in [
            "glm-5",
            "kimi-k2",
            "longcat-flash",
            "mimo-7b",
            "deepseek-v4-pro",
            "hy-1",
            "unknown-model",
        ] {
            assert_eq!(at_opencode(model), Protocol::Chat, "{model}");
        }
    }

    /// Everywhere else the chat-completions endpoint is the only thing this
    /// client can assume, whatever the model is called.
    #[test]
    fn detect_protocol_keeps_other_hosts_on_chat() {
        for base_url in [
            "https://api.deepseek.com",
            "https://api.deepseek.com/v1",
            "http://127.0.0.1:8080/v1",
            "https://opencode.example/zen/go/v1",
        ] {
            assert_eq!(
                detect_protocol(base_url, "grok-4.6"),
                Protocol::Chat,
                "{base_url}"
            );
            assert_eq!(
                detect_protocol(base_url, "minimax-m2.5"),
                Protocol::Chat,
                "{base_url}"
            );
        }
    }

    /// An explicit choice must win, or a model that is not in the detection
    /// table could never be reached over a protocol the user knows it speaks.
    #[test]
    fn an_explicit_protocol_overrides_detection() {
        let stored = StoredConfig {
            deepseek_api_key: "stored-key".into(),
            base_url: Some("https://opencode.ai/zen/go/v1".into()),
            // Detection alone would answer `messages` for this one.
            model: Some("minimax-m2.5".into()),
            protocol: Some("CHAT".into()),
        };
        let from_env = |key: &str| match key {
            "DEEPSEEK_API_KEY" => Some("env-key".into()),
            "DEEPSEEK_PROTOCOL" => Some("responses".into()),
            _ => None,
        };
        assert_eq!(
            resolve_settings(from_env, Some(stored.clone()))
                .unwrap()
                .protocol,
            Protocol::Responses
        );
        let key_only = |key: &str| match key {
            "DEEPSEEK_API_KEY" => Some("env-key".into()),
            _ => None,
        };
        assert_eq!(
            resolve_settings(key_only, Some(stored)).unwrap().protocol,
            Protocol::Chat
        );
        // Nothing pinned: the model name decides.
        let detected = StoredConfig {
            deepseek_api_key: "stored-key".into(),
            base_url: Some("https://opencode.ai/zen/go/v1".into()),
            model: Some("minimax-m2.5".into()),
            protocol: None,
        };
        assert_eq!(
            resolve_settings(key_only, Some(detected)).unwrap().protocol,
            Protocol::Messages
        );
    }

    /// Falling back to chat on a typo would send a Messages request body to the
    /// chat endpoint, which fails in a way that says nothing about the typo.
    #[test]
    fn an_unknown_protocol_name_is_rejected() {
        let stored = StoredConfig {
            deepseek_api_key: "stored-key".into(),
            base_url: Some("https://opencode.ai/zen/go/v1".into()),
            model: Some("minimax-m2.5".into()),
            protocol: None,
        };
        let typo = |key: &str| match key {
            "DEEPSEEK_API_KEY" => Some("env-key".into()),
            "DEEPSEEK_PROTOCOL" => Some("messages-api".into()),
            _ => None,
        };
        let error = resolve_settings(typo, Some(stored))
            .unwrap_err()
            .to_string();
        assert!(error.contains("messages-api"), "{error}");
        assert!(error.contains("chat, responses or messages"), "{error}");
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

    /// OpenCode Go answers 400 `MissingSessionID` without this header, so a
    /// request that omits it cannot reach the model at all.
    #[test]
    fn requests_carry_a_session_header_and_identify_the_agent() {
        let (base_url, server) =
            serve_one(r#"{"model":"test-model","choices":[{"message":{"content":"ok"}}]}"#);
        let config = DeepSeekConfig::new(settings(
            Protocol::Chat,
            "secret-key",
            &base_url,
            "test-model",
        ))
        .unwrap();
        chat_messages(&config, &[json!({"role":"user","content":"hi"})], None).unwrap();

        let request = server.join().unwrap();
        let headers = request.to_ascii_lowercase();
        assert!(
            headers.contains("x-opencode-session: hyper-"),
            "request must carry a session id: {request}"
        );
        assert!(
            headers.contains(&format!("user-agent: {USER_AGENT}")),
            "request must identify the agent: {request}"
        );
        assert!(
            headers.contains("authorization: bearer secret-key"),
            "request must carry the configured key: {request}"
        );
    }

    /// Two configurations are two conversations, so providers must not be handed
    /// the same session id for both.
    #[test]
    fn each_conversation_gets_its_own_session_id() {
        let session_of = || {
            let (base_url, server) =
                serve_one(r#"{"model":"m","choices":[{"message":{"content":"ok"}}]}"#);
            let config =
                DeepSeekConfig::new(settings(Protocol::Chat, "k", &base_url, "m")).unwrap();
            chat_messages(&config, &[json!({"role":"user","content":"hi"})], None).unwrap();
            let request = server.join().unwrap().to_ascii_lowercase();
            request
                .lines()
                .find_map(|line| line.strip_prefix("x-opencode-session: ").map(str::to_owned))
                .expect("every request carries a session id")
        };
        assert_ne!(session_of(), session_of());
    }

    #[test]
    fn provider_names_follow_the_base_url() {
        assert_eq!(provider_name("https://api.deepseek.com"), "deepseek");
        assert_eq!(
            provider_name("https://opencode.ai/zen/go/v1"),
            "opencode-go"
        );
        assert_eq!(provider_name("https://opencode.ai/zen/v1"), "opencode-zen");
        assert_eq!(provider_name("http://127.0.0.1:8080/v1"), "127.0.0.1");
    }

    #[test]
    fn environment_overrides_the_stored_configuration() {
        let stored = StoredConfig {
            deepseek_api_key: "stored-key".into(),
            base_url: Some("https://stored.example/v1".into()),
            model: Some("stored-model".into()),
            protocol: None,
        };
        let from_env = |key: &str| match key {
            "DEEPSEEK_API_KEY" => Some("env-key".into()),
            "DEEPSEEK_BASE_URL" => Some("https://opencode.ai/zen/go/v1".into()),
            _ => None,
        };
        assert_eq!(
            resolve_settings(from_env, Some(stored.clone())).unwrap(),
            settings(
                Protocol::Chat,
                "env-key",
                "https://opencode.ai/zen/go/v1",
                "stored-model"
            )
        );
        // Blank environment values are not an answer, so the file still wins.
        assert_eq!(
            resolve_settings(|_| Some("  ".into()), Some(stored.clone())).unwrap(),
            settings(
                Protocol::Chat,
                "stored-key",
                "https://stored.example/v1",
                "stored-model"
            )
        );
        // A stored key alone keeps the built-in DeepSeek endpoint and model.
        let key_only = StoredConfig {
            deepseek_api_key: "stored-key".into(),
            ..StoredConfig::default()
        };
        assert_eq!(
            resolve_settings(|_| None, Some(key_only)).unwrap(),
            settings(
                Protocol::Chat,
                "stored-key",
                DEFAULT_BASE_URL,
                DEFAULT_MODEL
            )
        );
        assert!(resolve_settings(|_| None, None).is_err());
    }

    #[test]
    fn stored_configuration_round_trips_and_reads_older_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hyper").join("config.json");
        assert!(read_stored_config(&path).unwrap().is_none());

        let config = StoredConfig {
            deepseek_api_key: "key".into(),
            base_url: Some("https://opencode.ai/zen/go/v1".into()),
            model: Some("deepseek-v4-pro".into()),
            protocol: Some("messages".into()),
        };
        save_config(&path, &config).unwrap();
        let read = read_stored_config(&path).unwrap().unwrap();
        assert_eq!(read.deepseek_api_key, "key");
        assert_eq!(
            read.base_url.as_deref(),
            Some("https://opencode.ai/zen/go/v1")
        );
        assert_eq!(read.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(read.protocol.as_deref(), Some("messages"));

        // A file written before the provider settings existed still loads.
        fs::write(&path, r#"{"deepseek_api_key":"old"}"#).unwrap();
        let read = read_stored_config(&path).unwrap().unwrap();
        assert_eq!(read.deepseek_api_key, "old");
        assert_eq!(read.base_url, None);
        assert_eq!(read.protocol, None);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the key must stay owner-only");
        }
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
        let config = DeepSeekConfig::with_timeout(
            settings(
                Protocol::Chat,
                "test-key",
                &format!("http://{address}"),
                "test-model",
            ),
            Duration::from_secs(2),
        )
        .unwrap();

        let reply = chat_messages(&config, &[json!({"role":"user","content":"hi"})], None).unwrap();
        assert_eq!(reply.content, "ok");
        server.join().unwrap();
    }

    fn read_tool() -> ToolSpec {
        ToolSpec {
            name: "read",
            description: "Read a text file",
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
            }),
        }
    }

    /// The chat format is what the other two are translated into, so it must
    /// keep travelling exactly as the OpenAI shape describes it.
    #[test]
    fn chat_protocol_round_trips_text_tool_calls_and_usage() {
        let (base_url, server) = serve_one(
            r#"{"model":"deepseek-v4-flash","choices":[{"message":{"content":"done","reasoning_content":"thinking","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"demo.rs\"}"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
        );
        let config = DeepSeekConfig::new(settings(
            Protocol::Chat,
            "secret-key",
            &base_url,
            "deepseek-v4-flash",
        ))
        .unwrap();
        let reply = chat_messages(
            &config,
            &[json!({"role":"user","content":"hi"})],
            Some(&[read_tool()]),
        )
        .unwrap();

        assert_eq!(reply.content, "done");
        assert_eq!(reply.reasoning_content.as_deref(), Some("thinking"));
        let usage = reply.usage.expect("usage is reported");
        assert_eq!(
            (
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens
            ),
            (10, 5, 15)
        );
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].id, "call_1");
        assert_eq!(reply.tool_calls[0].call_type, "function");
        assert_eq!(reply.tool_calls[0].function.name, "read");
        assert_eq!(
            reply.tool_calls[0].function.arguments,
            r#"{"path":"demo.rs"}"#
        );

        let request = server.join().unwrap();
        assert!(
            request.starts_with("POST /chat/completions HTTP/1.1"),
            "{request}"
        );
        let headers = request.to_ascii_lowercase();
        assert!(
            headers.contains("authorization: bearer secret-key"),
            "{request}"
        );
        assert!(headers.contains("x-opencode-session: hyper-"), "{request}");
        assert!(!headers.contains("x-api-key:"), "{request}");
        let body = request_body(&request);
        assert_eq!(body["model"], "deepseek-v4-flash");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert_eq!(body["stream"], false);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert_eq!(
            body["tools"][0]["function"]["parameters"]["required"][0],
            "path"
        );
    }

    /// The Responses format carries the whole history as typed items, including
    /// the previous turn's call, which the gateway demands ahead of its output.
    #[test]
    fn responses_protocol_translates_the_history_and_reads_function_calls() {
        let (base_url, server) = serve_one(
            r#"{"id":"resp_1","object":"response","status":"completed","model":"grok-4.6","error":null,"output":[{"id":"rs_1","type":"reasoning","summary":[{"type":"summary_text","text":"checking"}]},{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]},{"type":"function_call","name":"read","call_id":"call-1","arguments":"{\"path\":\"demo.rs\"}"}],"usage":{"input_tokens":292,"output_tokens":180,"total_tokens":472}}"#,
        );
        let config = DeepSeekConfig::new(settings(
            Protocol::Responses,
            "secret-key",
            &base_url,
            "grok-4.6",
        ))
        .unwrap();
        let history = [
            json!({"role":"system","content":"be brief"}),
            json!({"role":"user","content":"What is in demo.rs? Use the read tool."}),
            json!({"role":"assistant","content":"","tool_calls":[{"id":"call-1","type":"function","function":{"name":"read","arguments":"{\"path\":\"demo.rs\"}"}}]}),
            json!({"role":"tool","tool_call_id":"call-1","content":"fn main() { println!(\"hi\"); }"}),
        ];
        let reply = chat_messages(&config, &history, None).unwrap();

        assert_eq!(reply.content, "done");
        assert_eq!(reply.reasoning_content.as_deref(), Some("checking"));
        let usage = reply.usage.expect("usage is reported");
        assert_eq!(
            (
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens
            ),
            (292, 180, 472)
        );
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].id, "call-1");
        assert_eq!(reply.tool_calls[0].call_type, "function");
        assert_eq!(reply.tool_calls[0].function.name, "read");
        assert_eq!(
            reply.tool_calls[0].function.arguments,
            r#"{"path":"demo.rs"}"#
        );

        let request = server.join().unwrap();
        assert!(request.starts_with("POST /responses HTTP/1.1"), "{request}");
        let headers = request.to_ascii_lowercase();
        assert!(
            headers.contains("authorization: bearer secret-key"),
            "{request}"
        );
        assert!(headers.contains("x-opencode-session: hyper-"), "{request}");
        assert!(!headers.contains("x-api-key:"), "{request}");
        let body = request_body(&request);
        assert_eq!(body["model"], "grok-4.6");
        assert_eq!(body["instructions"], "be brief");
        assert_eq!(body["max_output_tokens"], MAX_OUTPUT_TOKENS);
        // The empty assistant text is dropped, leaving only the call it made.
        assert_eq!(
            body["input"][1],
            json!({"type":"function_call","call_id":"call-1","name":"read","arguments":"{\"path\":\"demo.rs\"}"})
        );
        assert_eq!(
            body["input"][2],
            json!({"type":"function_call_output","call_id":"call-1","output":"fn main() { println!(\"hi\"); }"})
        );
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"].as_array().map(Vec::len), Some(3));
    }

    /// The Messages format authenticates differently, wants a schema field named
    /// `input_schema`, carries tool arguments as an object, and keeps every tool
    /// result of one round inside a single user turn.
    #[test]
    fn messages_protocol_translates_the_history_and_reads_tool_use() {
        let (base_url, server) = serve_one(
            r#"{"id":"msg_1","type":"message","role":"assistant","model":"minimax-m2.5","content":[{"type":"thinking","thinking":"checking","signature":"abc"},{"type":"text","text":"done"},{"type":"tool_use","id":"toolu_9","name":"read","input":{"path":"demo.rs"}}],"usage":{"input_tokens":245,"output_tokens":200}}"#,
        );
        let config = DeepSeekConfig::new(settings(
            Protocol::Messages,
            "secret-key",
            &base_url,
            "minimax-m2.5",
        ))
        .unwrap();
        let history = [
            json!({"role":"system","content":"be brief"}),
            json!({"role":"user","content":"What is in demo.rs? Use the read tool."}),
            json!({"role":"assistant","content":"","tool_calls":[
                {"id":"toolu_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"demo.rs\"}"}},
                {"id":"toolu_2","type":"function","function":{"name":"read","arguments":"{\"path\":\"lib.rs\"}"}}
            ]}),
            json!({"role":"tool","tool_call_id":"toolu_1","content":"fn main() {}"}),
            json!({"role":"tool","tool_call_id":"toolu_2","content":"pub mod x;"}),
        ];
        let reply = chat_messages(&config, &history, Some(&[read_tool()])).unwrap();

        assert_eq!(reply.content, "done");
        assert_eq!(reply.reasoning_content.as_deref(), Some("checking"));
        let usage = reply.usage.expect("usage is reported");
        assert_eq!(
            (
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens
            ),
            (245, 200, 445)
        );
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].id, "toolu_9");
        assert_eq!(reply.tool_calls[0].call_type, "function");
        assert_eq!(reply.tool_calls[0].function.name, "read");
        assert_eq!(
            reply.tool_calls[0].function.arguments,
            r#"{"path":"demo.rs"}"#
        );

        let request = server.join().unwrap();
        assert!(request.starts_with("POST /messages HTTP/1.1"), "{request}");
        let headers = request.to_ascii_lowercase();
        // A bearer token here is answered with `Missing API key`.
        assert!(headers.contains("x-api-key: secret-key"), "{request}");
        assert!(
            headers.contains("anthropic-version: 2023-06-01"),
            "{request}"
        );
        assert!(headers.contains("x-opencode-session: hyper-"), "{request}");
        assert!(!headers.contains("authorization:"), "{request}");
        let body = request_body(&request);
        assert_eq!(body["model"], "minimax-m2.5");
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["max_tokens"], MAX_OUTPUT_TOKENS);
        assert_eq!(
            body["messages"][0],
            json!({"role":"user","content":[{"type":"text","text":"What is in demo.rs? Use the read tool."}]})
        );
        assert_eq!(
            body["messages"][1],
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"toolu_1","name":"read","input":{"path":"demo.rs"}},
                {"type":"tool_use","id":"toolu_2","name":"read","input":{"path":"lib.rs"}}
            ]})
        );
        assert_eq!(
            body["messages"][2],
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"toolu_1","content":"fn main() {}"},
                {"type":"tool_result","tool_use_id":"toolu_2","content":"pub mod x;"}
            ]})
        );
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(3));
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(
            body["tools"][0]["input_schema"]["properties"]["path"]["type"],
            "string"
        );
        assert!(body["tools"][0].get("function").is_none(), "{body}");
    }

    /// An unparsable argument string must not fail the turn: the request has to
    /// go out, and the model is answered with the text it produced.
    #[test]
    fn messages_protocol_survives_unparsable_tool_arguments() {
        let (base_url, server) = serve_one(
            r#"{"id":"msg_1","type":"message","role":"assistant","model":"minimax-m2.5","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1,"output_tokens":2}}"#,
        );
        let config = DeepSeekConfig::new(settings(
            Protocol::Messages,
            "secret-key",
            &base_url,
            "minimax-m2.5",
        ))
        .unwrap();
        let history = [
            json!({"role":"user","content":"go"}),
            json!({"role":"assistant","content":"","tool_calls":[{"id":"toolu_1","type":"function","function":{"name":"read","arguments":"not json"}}]}),
            json!({"role":"tool","tool_call_id":"toolu_1","content":"x"}),
        ];
        chat_messages(&config, &history, None).unwrap();

        let body = request_body(&server.join().unwrap());
        assert_eq!(body["messages"][1]["content"][0]["input"], json!({}));
        // The result still lands in a turn of its own after the assistant one.
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
    }

    /// A Responses item whose `summary_text` never arrives must not turn into an
    /// empty reasoning string, which the caller displays as a thought.
    #[test]
    fn responses_protocol_omits_reasoning_when_the_model_produced_none() {
        let (base_url, server) = serve_one(
            r#"{"id":"resp_1","model":"grok-4.6","output":[{"id":"msg_1","type":"message","content":[{"type":"output_text","text":"OK"}]}],"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}"#,
        );
        let config =
            DeepSeekConfig::new(settings(Protocol::Responses, "k", &base_url, "grok-4.6")).unwrap();
        let reply =
            chat_messages(&config, &[json!({"role":"user","content":"Say OK"})], None).unwrap();
        server.join().unwrap();

        assert_eq!(reply.content, "OK");
        assert_eq!(reply.reasoning_content, None);
        assert!(reply.tool_calls.is_empty());
        let usage = reply.usage.expect("usage is reported");
        assert_eq!(usage.total_tokens, 3);
    }
}
