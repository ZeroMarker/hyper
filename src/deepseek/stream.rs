use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::{blocking::Response, header::CONTENT_TYPE};
use serde_json::{Value, json};

use super::{
    DeepSeekConfig, MAX_API_ATTEMPTS, ModelReply, RETRY_AFTER, ToolCall, ToolFunction, Usage,
    endpoint, retry_delay, retryable_status,
};

const MAX_SSE_EVENT: usize = 1024 * 1024;

/// Status failures can be retried before any SSE payload is consumed. A broken
/// stream cannot be retried: doing so would duplicate deltas already audited.
fn open(config: &DeepSeekConfig, body: &Value) -> Result<Response> {
    let url = endpoint(&config.base_url, config.protocol);
    for attempt in 0..MAX_API_ATTEMPTS {
        let response = match config.client.post(&url).json(body).send() {
            Ok(response) => response,
            Err(_) if attempt + 1 < MAX_API_ATTEMPTS => {
                std::thread::sleep(retry_delay(attempt, None));
                continue;
            }
            Err(error) => {
                return Err(error).context(format!("failed to call the {} API", config.provider));
            }
        };
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs);
        let response_body = response.text()?;
        if retryable_status(status) && attempt + 1 < MAX_API_ATTEMPTS {
            std::thread::sleep(retry_delay(attempt, retry_after));
            continue;
        }
        bail!("{} API returned {status}: {response_body}", config.provider);
    }
    bail!("{} API request exhausted all attempts", config.provider)
}

/// Read one SSE frame at a time, including multiline data fields. EOF without
/// a terminal frame is an error in each protocol parser below.
fn frames(response: Response, mut on_frame: impl FnMut(&str, &str) -> Result<bool>) -> Result<()> {
    let mut reader = BufReader::new(response);
    let mut line = String::new();
    let mut event = String::new();
    let mut data = String::new();
    loop {
        line.clear();
        let count = reader
            .read_line(&mut line)
            .context("SSE stream interrupted")?;
        if count == 0 {
            if !data.is_empty() && on_frame(&event, data.trim_end_matches('\n'))? {
                return Ok(());
            }
            bail!("SSE stream ended before completion");
        }
        if line == "\n" || line == "\r\n" {
            if !data.is_empty() && on_frame(&event, data.trim_end_matches('\n'))? {
                return Ok(());
            }
            event.clear();
            data.clear();
            continue;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(value) = trimmed.strip_prefix("event:") {
            event = value.trim_start().to_owned();
        } else if let Some(value) = trimmed.strip_prefix("data:") {
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
            data.push('\n');
        }
        if data.len() > MAX_SSE_EVENT {
            bail!("SSE event exceeds {MAX_SSE_EVENT} bytes");
        }
    }
}

fn value(data: &str) -> Result<Value> {
    serde_json::from_str(data).context("invalid SSE JSON event")
}
fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}
fn call(id: String, name: String, arguments: String) -> ToolCall {
    ToolCall {
        id,
        call_type: "function".into(),
        function: ToolFunction { name, arguments },
    }
}

fn validate_calls(calls: &[ToolCall]) -> Result<()> {
    for call in calls {
        if call.id.is_empty() || call.function.name.is_empty() {
            bail!("incomplete streamed tool call id or name");
        }
        let args: Value = serde_json::from_str(&call.function.arguments)
            .context("incomplete streamed tool arguments")?;
        if !args.is_object() {
            bail!("streamed tool arguments must be a JSON object");
        }
    }
    Ok(())
}

fn json_fallback(
    response: &mut Response,
    config: &DeepSeekConfig,
    on_text: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Option<ModelReply>> {
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("application/json"));
    if !is_json {
        return Ok(None);
    }
    let mut raw = String::new();
    std::io::Read::read_to_string(response, &mut raw)?;
    let reply = match config.protocol {
        super::Protocol::Chat => {
            let parsed: super::ChatResponse = serde_json::from_str(&raw)?;
            let message = parsed
                .choices
                .into_iter()
                .next()
                .context("chat response contained no choices")?
                .message;
            ModelReply {
                content: message.content.unwrap_or_default(),
                reasoning_content: message.reasoning_content,
                model: parsed.model,
                usage: parsed.usage,
                tool_calls: message.tool_calls.unwrap_or_default(),
            }
        }
        super::Protocol::Responses => {
            let parsed: super::ResponsesResponse = serde_json::from_str(&raw)?;
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
                    "function_call" => tool_calls.push(call(
                        item.call_id.unwrap_or_default(),
                        item.name.unwrap_or_default(),
                        item.arguments.unwrap_or_default(),
                    )),
                    _ => {}
                }
            }
            ModelReply {
                content,
                reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
                model: parsed.model,
                usage: parsed.usage.map(|u| Usage {
                    prompt_tokens: u.input_tokens,
                    completion_tokens: u.output_tokens,
                    total_tokens: u.total_tokens,
                }),
                tool_calls,
            }
        }
        super::Protocol::Messages => {
            let parsed: super::MessagesResponse = serde_json::from_str(&raw)?;
            let mut content = String::new();
            let mut reasoning = String::new();
            let mut tool_calls = Vec::new();
            for block in parsed.content {
                match block.block_type.as_str() {
                    "text" => content.push_str(block.text.as_deref().unwrap_or_default()),
                    "thinking" => reasoning.push_str(block.thinking.as_deref().unwrap_or_default()),
                    "tool_use" => tool_calls.push(call(
                        block.id.unwrap_or_default(),
                        block.name.unwrap_or_default(),
                        block.input.unwrap_or_else(|| json!({})).to_string(),
                    )),
                    _ => {}
                }
            }
            ModelReply {
                content,
                reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
                model: parsed.model,
                usage: parsed.usage.map(|u| Usage {
                    prompt_tokens: u.input_tokens,
                    completion_tokens: u.output_tokens,
                    total_tokens: u.input_tokens + u.output_tokens,
                }),
                tool_calls,
            }
        }
    };
    on_text(&reply.content)?;
    validate_calls(&reply.tool_calls)?;
    Ok(Some(reply))
}

pub(super) fn stream_chat(
    config: &DeepSeekConfig,
    body: &Value,
    on_text: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<ModelReply> {
    let mut response = open(config, body)?;
    if let Some(reply) = json_fallback(&mut response, config, on_text)? {
        return Ok(reply);
    }
    let mut reply = ModelReply {
        content: String::new(),
        reasoning_content: None,
        model: config.model.clone(),
        usage: None,
        tool_calls: vec![],
    };
    let mut calls: BTreeMap<u64, ToolCall> = BTreeMap::new();
    let mut finished = false;
    frames(response, |_, data| {
        if data == "[DONE]" {
            finished = true;
            return Ok(true);
        }
        let frame = value(data)?;
        if let Some(model) = frame["model"].as_str() {
            reply.model = model.to_owned();
        }
        if !frame["usage"].is_null() {
            reply.usage = Some(serde_json::from_value(frame["usage"].clone())?);
        }
        for choice in frame["choices"].as_array().into_iter().flatten() {
            let delta = &choice["delta"];
            if let Some(text) = delta["content"].as_str() {
                reply.content.push_str(text);
                on_text(text)?;
            }
            if let Some(text) = delta["reasoning_content"].as_str() {
                reply
                    .reasoning_content
                    .get_or_insert_with(String::new)
                    .push_str(text);
            }
            for part in delta["tool_calls"].as_array().into_iter().flatten() {
                let index = part["index"]
                    .as_u64()
                    .context("chat tool delta missing index")?;
                let entry = calls
                    .entry(index)
                    .or_insert_with(|| call(String::new(), String::new(), String::new()));
                if let Some(id) = part["id"].as_str() {
                    entry.id.push_str(id);
                }
                if let Some(name) = part["function"]["name"].as_str() {
                    entry.function.name.push_str(name);
                }
                if let Some(args) = part["function"]["arguments"].as_str() {
                    entry.function.arguments.push_str(args);
                }
            }
        }
        Ok(false)
    })?;
    if !finished {
        bail!("chat stream ended before [DONE]");
    }
    reply.tool_calls = calls.into_values().collect();
    validate_calls(&reply.tool_calls)?;
    Ok(reply)
}

pub(super) fn stream_responses(
    config: &DeepSeekConfig,
    body: &Value,
    on_text: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<ModelReply> {
    let mut response = open(config, body)?;
    if let Some(reply) = json_fallback(&mut response, config, on_text)? {
        return Ok(reply);
    }
    let mut completed = None;
    frames(response, |event, data| {
        let frame = value(data)?;
        let kind = if event.is_empty() {
            string(&frame, "type")
        } else {
            event
        };
        match kind {
            "response.output_text.delta" => {
                if let Some(text) = frame["delta"].as_str() {
                    on_text(text)?;
                }
            }
            "response.completed" => {
                completed = Some(frame["response"].clone());
                return Ok(true);
            }
            "response.failed" | "response.incomplete" => bail!("response stream {kind}: {frame}"),
            "error" => bail!("response stream error: {frame}"),
            _ => {}
        }
        Ok(false)
    })?;
    let full = completed.context("response stream ended before response.completed")?;
    // The final response is authoritative: it includes usage and complete tool
    // arguments even if the gateway omitted an intermediate delta.
    let parsed: super::ResponsesResponse = serde_json::from_value(full)?;
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
            "function_call" => tool_calls.push(call(
                item.call_id.unwrap_or_default(),
                item.name.unwrap_or_default(),
                item.arguments.unwrap_or_default(),
            )),
            _ => {}
        }
    }
    validate_calls(&tool_calls)?;
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

#[derive(Default)]
struct MessageBlock {
    kind: String,
    text: String,
    id: String,
    name: String,
    input: String,
    initial_input: Option<Value>,
}

pub(super) fn stream_messages(
    config: &DeepSeekConfig,
    body: &Value,
    on_text: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<ModelReply> {
    let mut response = open(config, body)?;
    if let Some(reply) = json_fallback(&mut response, config, on_text)? {
        return Ok(reply);
    }
    let mut model = config.model.clone();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut blocks: BTreeMap<u64, MessageBlock> = BTreeMap::new();
    let mut stopped = false;
    frames(response, |event, data| {
        let frame = value(data)?;
        let kind = if event.is_empty() {
            string(&frame, "type")
        } else {
            event
        };
        match kind {
            "message_start" => {
                if let Some(name) = frame["message"]["model"].as_str() {
                    model = name.to_owned();
                }
                input_tokens = frame["message"]["usage"]["input_tokens"]
                    .as_u64()
                    .unwrap_or(0);
            }
            "content_block_start" => {
                let index = frame["index"]
                    .as_u64()
                    .context("Messages block missing index")?;
                let block = &frame["content_block"];
                blocks.insert(
                    index,
                    MessageBlock {
                        kind: string(block, "type").to_owned(),
                        id: string(block, "id").to_owned(),
                        name: string(block, "name").to_owned(),
                        initial_input: block.get("input").cloned(),
                        ..Default::default()
                    },
                );
                if let Some(text) = block["text"].as_str() {
                    blocks.get_mut(&index).unwrap().text.push_str(text);
                    on_text(text)?;
                }
            }
            "content_block_delta" => {
                let index = frame["index"]
                    .as_u64()
                    .context("Messages delta missing index")?;
                let block = blocks
                    .get_mut(&index)
                    .context("Messages delta before block start")?;
                let delta = &frame["delta"];
                match string(delta, "type") {
                    "text_delta" => {
                        let text = string(delta, "text");
                        block.text.push_str(text);
                        on_text(text)?;
                    }
                    "thinking_delta" => block.text.push_str(string(delta, "thinking")),
                    "input_json_delta" => block.input.push_str(string(delta, "partial_json")),
                    _ => {}
                }
            }
            "message_delta" => {
                output_tokens = frame["usage"]["output_tokens"]
                    .as_u64()
                    .unwrap_or(output_tokens);
            }
            "message_stop" => {
                stopped = true;
                return Ok(true);
            }
            "error" => bail!("Messages stream error: {frame}"),
            _ => {}
        }
        Ok(false)
    })?;
    if !stopped {
        bail!("Messages stream ended before message_stop");
    }
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    for block in blocks.into_values() {
        match block.kind.as_str() {
            "text" => content.push_str(&block.text),
            "thinking" => reasoning.push_str(&block.text),
            "tool_use" => {
                let input = if block.input.is_empty() {
                    block.initial_input.unwrap_or_else(|| json!({}))
                } else {
                    serde_json::from_str::<Value>(&block.input)
                        .context("incomplete Messages tool arguments")?
                };
                tool_calls.push(call(block.id, block.name, input.to_string()));
            }
            _ => {}
        }
    }
    validate_calls(&tool_calls)?;
    Ok(ModelReply {
        content,
        reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
        model,
        usage: Some(Usage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
        }),
        tool_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deepseek::{Protocol, Settings};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
    };

    fn streamed(
        protocol: Protocol,
        first: &'static str,
        rest: &'static str,
        expected_text: &str,
    ) -> (ModelReply, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (seen_tx, seen_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut bytes = [0u8; 8192];
            let count = socket.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]).into_owned();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{first}").unwrap();
            socket.flush().unwrap();
            seen_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("first delta arrived before stream ended");
            write!(socket, "{rest}").unwrap();
            socket.flush().unwrap();
            request
        });
        let config = DeepSeekConfig::new(Settings {
            api_key: "key".into(),
            base_url: url,
            model: "test-model".into(),
            protocol,
        })
        .unwrap();
        let mut seen = String::new();
        let reply = super::super::chat_messages_stream(
            &config,
            &[json!({"role":"user","content":"hi"})],
            None,
            &mut |text| {
                seen.push_str(text);
                if seen == expected_text {
                    seen_tx.send(()).unwrap();
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(seen, reply.content);
        (reply, server.join().unwrap())
    }

    #[test]
    fn chat_streams_text_and_joins_tool_arguments() {
        let first =
            "data: {\"model\":\"chat-model\",\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n";
        let rest = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\",\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"pa\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"th\\\":\\\"a\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n"
        );
        let (reply, request) = streamed(Protocol::Chat, first, rest, "hel");
        assert_eq!(reply.content, "hello");
        assert_eq!(reply.model, "chat-model");
        assert_eq!(reply.usage.unwrap().total_tokens, 5);
        assert_eq!(reply.tool_calls[0].function.arguments, r#"{"path":"a"}"#);
        assert!(request.contains("\"stream\":true"), "{request}");
    }

    #[test]
    fn responses_streams_text_and_takes_completed_tool_call() {
        let first = "event: response.output_text.delta\ndata: {\"delta\":\"hi\"}\n\n";
        let rest = "event: response.completed\ndata: {\"response\":{\"model\":\"grok\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]},{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"a\\\"}\"}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n";
        let (reply, request) = streamed(Protocol::Responses, first, rest, "hi");
        assert_eq!(reply.tool_calls[0].function.name, "read");
        assert_eq!(reply.usage.unwrap().total_tokens, 3);
        assert!(request.contains("\"stream\":true"));
    }

    #[test]
    fn messages_streams_text_and_joins_json_fragments() {
        let first = concat!(
            "event: message_start\ndata: {\"message\":{\"model\":\"mini\",\"usage\":{\"input_tokens\":4}}}\n\n",
            "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n"
        );
        let rest = concat!(
            "event: content_block_start\ndata: {\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"read\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\"}}\n\n",
            "event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\":\\\"a\\\"}\"}}\n\n",
            "event: message_delta\ndata: {\"usage\":{\"output_tokens\":6}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );
        let (reply, request) = streamed(Protocol::Messages, first, rest, "hi");
        assert_eq!(reply.tool_calls[0].function.arguments, r#"{"path":"a"}"#);
        assert_eq!(reply.usage.unwrap().total_tokens, 10);
        assert!(request.contains("\"stream\":true"));
    }

    #[test]
    fn incomplete_stream_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request);
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"half\"}}]}\n\n";
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        let config = DeepSeekConfig::new(Settings {
            api_key: "k".into(),
            base_url: url,
            model: "m".into(),
            protocol: Protocol::Chat,
        })
        .unwrap();
        let error = super::super::chat_messages_stream(
            &config,
            &[json!({"role":"user","content":"hi"})],
            None,
            &mut |_| Ok(()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("before completion"), "{error:#}");
        server.join().unwrap();
    }
}
