use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::header::CONTENT_TYPE;
use serde_json::{Value, json};

use super::{
    DeepSeekConfig, MAX_API_ATTEMPTS, ModelReply, RETRY_AFTER, ToolCall, ToolFunction, Usage,
    endpoint, retry_delay, retryable_status,
};

use super::completion::{
    CompletionDiagnostics, CompletionError, TextPublicationError, safe_reason,
};

const MAX_SSE_EVENT: usize = 1024 * 1024;

/// An async response exposed to the existing synchronous protocol parsers.
/// Each socket wait selects cancellation, including JSON fallback body reads.
struct Response {
    response: reqwest::Response,
    pending: Vec<u8>,
    offset: usize,
    cancellation: crate::CancellationToken,
}
impl Response {
    fn headers(&self) -> &reqwest::header::HeaderMap {
        self.response.headers()
    }
}
impl Read for Response {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.cancellation.check().map_err(std::io::Error::other)?;
        if output.is_empty() {
            return Ok(0);
        }
        while self.offset == self.pending.len() {
            let chunk = self
                .cancellation
                .io(|| self.response.chunk())
                .map_err(std::io::Error::other)?;
            let Some(chunk) = chunk else {
                return Ok(0);
            };
            self.pending = chunk.to_vec();
            self.offset = 0;
        }
        let length = output.len().min(self.pending.len() - self.offset);
        output[..length].copy_from_slice(&self.pending[self.offset..self.offset + length]);
        self.offset += length;
        Ok(length)
    }
}

/// Status failures can be retried before any SSE payload is consumed. A broken
/// stream cannot be retried: doing so would duplicate deltas already audited.
fn open(config: &DeepSeekConfig, body: &Value) -> Result<Response> {
    let url = endpoint(&config.base_url, config.protocol);
    for attempt in 0..MAX_API_ATTEMPTS {
        let response = match config
            .cancellation
            .io(|| config.async_client.post(&url).json(body).send())
        {
            Ok(response) => response,
            Err(error) if config.cancellation.is_cancelled() => return Err(error),
            Err(_) if attempt + 1 < MAX_API_ATTEMPTS => {
                config.cancellation.wait(retry_delay(attempt, None))?;
                continue;
            }
            Err(error) => {
                return Err(error).context(format!("failed to call the {} API", config.provider));
            }
        };
        let status = response.status();
        if status.is_success() {
            return Ok(Response {
                response,
                pending: Vec::new(),
                offset: 0,
                cancellation: config.cancellation.clone(),
            });
        }
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs);
        let response_body = config.cancellation.io(|| response.text())?;
        if retryable_status(status) && attempt + 1 < MAX_API_ATTEMPTS {
            config
                .cancellation
                .wait(retry_delay(attempt, retry_after))?;
            continue;
        }
        bail!("{} API returned {status}: {response_body}", config.provider);
    }
    bail!("{} API request exhausted all attempts", config.provider)
}

/// Read one SSE frame at a time, including multiline data fields. EOF without
/// a terminal frame is an error in each protocol parser below.
fn frames(response: Response, mut on_frame: impl FnMut(&str, &str) -> Result<bool>) -> Result<()> {
    let cancellation = response.cancellation.clone();
    let mut reader = BufReader::new(response);
    let mut line = String::new();
    let mut event = String::new();
    let mut data = String::new();
    loop {
        cancellation.check()?;
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

fn emit_text(
    text: &str,
    audit: &mut CompletionDiagnostics,
    on_text: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<()> {
    audit.text_bytes += text.len();
    on_text(text).map_err(|error| error.context(TextPublicationError))
}

fn annotate(
    error: anyhow::Error,
    audit: &CompletionDiagnostics,
    config: &DeepSeekConfig,
) -> anyhow::Error {
    if config.cancellation.is_cancelled() || error.is::<TextPublicationError>() {
        return error;
    }
    if let Some(completion) = error.downcast_ref::<CompletionError>() {
        let mut audit = audit.clone();
        if audit.argument_error.is_none() {
            audit.argument_error = completion.diagnostics.argument_error.clone();
        }
        return CompletionError {
            kind: completion.kind,
            diagnostics: audit,
        }
        .into();
    }
    audit.reject(if error.to_string().contains("before completion") {
        "missing_terminal"
    } else {
        "stream_interrupted"
    })
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
    let mut audit = CompletionDiagnostics::new(config.protocol, "json");
    let mut raw = String::new();
    std::io::Read::read_to_string(response, &mut raw)
        .map_err(|error| annotate(error.into(), &audit, config))?;
    let raw_value: Value =
        serde_json::from_str(&raw).map_err(|_| audit.reject("malformed_frame"))?;
    audit.terminal_received = true;
    audit.finish_reason = match config.protocol {
        super::Protocol::Chat => safe_reason(&raw_value["choices"][0]["finish_reason"]),
        super::Protocol::Responses => safe_reason(&raw_value["incomplete_details"]["reason"])
            .or_else(|| safe_reason(&raw_value["status"])),
        super::Protocol::Messages => safe_reason(&raw_value["stop_reason"]),
    };
    let reply = match config.protocol {
        super::Protocol::Chat => {
            let parsed: super::ChatResponse =
                serde_json::from_str(&raw).map_err(|_| audit.reject("malformed_frame"))?;
            let message = parsed
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| audit.reject("missing_choice"))?
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
            let parsed: super::ResponsesResponse =
                serde_json::from_str(&raw).map_err(|_| audit.reject("malformed_frame"))?;
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
            let parsed: super::MessagesResponse =
                serde_json::from_str(&raw).map_err(|_| audit.reject("malformed_frame"))?;
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
                usage: parsed.usage.and_then(|u| {
                    u.input_tokens
                        .checked_add(u.output_tokens)
                        .map(|total| Usage {
                            prompt_tokens: u.input_tokens,
                            completion_tokens: u.output_tokens,
                            total_tokens: total,
                        })
                }),
                tool_calls,
            }
        }
    };
    audit.reported_usage = reply.usage.clone();
    audit.observe_tools(
        reply
            .tool_calls
            .iter()
            .enumerate()
            .map(|(index, call)| (index as u64, call, 1)),
    );
    emit_text(&reply.content, &mut audit, on_text)?;
    if config.protocol == super::Protocol::Chat {
        audit.check_chat_reason()?;
    }
    if config.protocol == super::Protocol::Responses {
        if let Some(status) = raw_value["status"].as_str()
            && status != "completed"
        {
            return Err(audit.reject("incomplete_response"));
        }
        if !raw_value["incomplete_details"].is_null() {
            return Err(audit.reject("incomplete_response"));
        }
    }
    audit.validate_calls(&reply.tool_calls)?;
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
    let mut fragments: BTreeMap<u64, usize> = BTreeMap::new();
    let mut audit = CompletionDiagnostics::new(config.protocol, "sse");
    let mut rejected_reason = None;
    let result = frames(response, |_, data| {
        audit.frames_received += 1;
        if data == "[DONE]" {
            audit.terminal_received = true;
            return Ok(true);
        }
        let frame = value(data).map_err(|_| audit.reject("malformed_frame"))?;
        if let Some(model) = frame["model"].as_str() {
            reply.model = model.to_owned();
        }
        if !frame["usage"].is_null() {
            // Incomplete or malformed counters are unknown, never zero.
            reply.usage = serde_json::from_value(frame["usage"].clone()).ok();
            audit.reported_usage = reply.usage.clone();
        }
        for choice in frame["choices"].as_array().into_iter().flatten() {
            if choice["index"].as_u64().is_some_and(|index| index != 0) {
                return Err(audit.reject("unexpected_choice"));
            }
            if let Some(reason) = safe_reason(&choice["finish_reason"]) {
                audit.finish_reason = Some(reason);
                if reason == "length" || reason == "content_filter" {
                    rejected_reason = Some(reason);
                }
            }
            let delta = &choice["delta"];
            if let Some(text) = delta["content"].as_str() {
                reply.content.push_str(text);
                emit_text(text, &mut audit, on_text)?;
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
                    .ok_or_else(|| audit.reject("malformed_frame"))?;
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
                    *fragments.entry(index).or_default() += 1;
                }
            }
        }
        Ok(false)
    });
    audit.observe_tools(
        calls
            .iter()
            .map(|(index, call)| (*index, call, *fragments.get(index).unwrap_or(&0))),
    );
    result.map_err(|error| annotate(error, &audit, config))?;
    if !audit.terminal_received {
        return Err(audit.reject("missing_terminal"));
    }
    if let Some(reason) = rejected_reason {
        audit.finish_reason = Some(reason);
    }
    audit.check_chat_reason()?;
    reply.tool_calls = calls.into_values().collect();
    audit.validate_calls(&reply.tool_calls)?;
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
    let mut audit = CompletionDiagnostics::new(config.protocol, "sse");
    let result = frames(response, |event, data| {
        audit.frames_received += 1;
        let frame = value(data).map_err(|_| audit.reject("malformed_frame"))?;
        let kind = if event.is_empty() {
            string(&frame, "type")
        } else {
            event
        };
        if matches!(
            kind,
            "response.completed" | "response.failed" | "response.incomplete"
        ) {
            let calls: Vec<_> = frame["response"]["output"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter(|(_, item)| item["type"] == "function_call")
                .map(|(index, item)| {
                    (
                        index as u64,
                        call(
                            string(item, "call_id").to_owned(),
                            string(item, "name").to_owned(),
                            string(item, "arguments").to_owned(),
                        ),
                    )
                })
                .collect();
            audit.observe_tools(calls.iter().map(|(index, call)| (*index, call, 1)));
        }
        match kind {
            "response.output_text.delta" => {
                if let Some(text) = frame["delta"].as_str() {
                    emit_text(text, &mut audit, on_text)?;
                }
            }
            "response.completed" => {
                audit.terminal_received = true;
                audit.finish_reason = safe_reason(&frame["response"]["status"]);
                completed = Some(frame["response"].clone());
                return Ok(true);
            }
            "response.failed" | "response.incomplete" => {
                audit.terminal_received = true;
                audit.finish_reason =
                    safe_reason(&frame["response"]["incomplete_details"]["reason"])
                        .or_else(|| safe_reason(&frame["response"]["status"]));
                audit.reported_usage = serde_json::from_value::<super::ResponsesUsage>(
                    frame["response"]["usage"].clone(),
                )
                .ok()
                .map(|u| Usage {
                    prompt_tokens: u.input_tokens,
                    completion_tokens: u.output_tokens,
                    total_tokens: u.total_tokens,
                });
                return Err(audit.reject(if kind == "response.failed" {
                    "provider_failed"
                } else {
                    "incomplete_response"
                }));
            }
            "error" => return Err(audit.reject("provider_failed")),
            _ => {}
        }
        Ok(false)
    });
    result.map_err(|error| annotate(error, &audit, config))?;
    let full = completed.ok_or_else(|| audit.reject("missing_terminal"))?;
    audit.reported_usage = serde_json::from_value::<super::ResponsesUsage>(full["usage"].clone())
        .ok()
        .map(|u| Usage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.total_tokens,
        });
    if let Some(status) = full["status"].as_str()
        && status != "completed"
    {
        return Err(audit.reject("incomplete_response"));
    }
    if !full["incomplete_details"].is_null() {
        return Err(audit.reject("incomplete_response"));
    }
    // The final response is authoritative: it includes usage and complete tool
    // arguments even if the gateway omitted an intermediate delta.
    let parsed: super::ResponsesResponse =
        serde_json::from_value(full).map_err(|_| audit.reject("malformed_frame"))?;
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
    audit.validate_calls(&tool_calls)?;
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
    fragments: usize,
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
    let mut input_tokens = None;
    let mut output_tokens = None;
    let mut blocks: BTreeMap<u64, MessageBlock> = BTreeMap::new();
    let mut stopped = false;
    let mut audit = CompletionDiagnostics::new(config.protocol, "sse");
    let result = frames(response, |event, data| {
        audit.frames_received += 1;
        let frame = value(data).map_err(|_| audit.reject("malformed_frame"))?;
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
                input_tokens = frame["message"]["usage"]["input_tokens"].as_u64();
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
                    emit_text(text, &mut audit, on_text)?;
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
                        emit_text(text, &mut audit, on_text)?;
                    }
                    "thinking_delta" => block.text.push_str(string(delta, "thinking")),
                    "input_json_delta" => {
                        block.input.push_str(string(delta, "partial_json"));
                        block.fragments += 1;
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                output_tokens = frame["usage"]["output_tokens"].as_u64().or(output_tokens);
                if let Some(reason) = safe_reason(&frame["delta"]["stop_reason"]) {
                    audit.finish_reason = Some(reason);
                }
            }
            "message_stop" => {
                stopped = true;
                audit.terminal_received = true;
                return Ok(true);
            }
            "error" => return Err(audit.reject("provider_failed")),
            _ => {}
        }
        Ok(false)
    });
    audit.reported_usage = input_tokens.zip(output_tokens).and_then(|(input, output)| {
        input.checked_add(output).map(|total| Usage {
            prompt_tokens: input,
            completion_tokens: output,
            total_tokens: total,
        })
    });
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut pending = Vec::new();
    for (index, block) in blocks {
        match block.kind.as_str() {
            "text" => content.push_str(&block.text),
            "thinking" => reasoning.push_str(&block.text),
            "tool_use" => {
                let arguments = if block.input.is_empty() {
                    block.initial_input.unwrap_or_else(|| json!({})).to_string()
                } else {
                    block.input
                };
                pending.push((
                    index,
                    call(block.id, block.name, arguments),
                    block.fragments,
                ));
            }
            _ => {}
        }
    }
    audit.observe_tools(
        pending
            .iter()
            .map(|(index, call, fragments)| (*index, call, *fragments)),
    );
    result.map_err(|error| annotate(error, &audit, config))?;
    if !stopped {
        return Err(audit.reject("missing_terminal"));
    }
    let tool_calls: Vec<_> = pending.into_iter().map(|(_, call, _)| call).collect();
    audit.validate_calls(&tool_calls)?;
    Ok(ModelReply {
        content,
        reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
        model,
        usage: audit.reported_usage,
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
    fn fixture(protocol: Protocol, content_type: &str, body: String) -> Result<ModelReply> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let content_type = content_type.to_owned();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 8192];
            let _ = socket.read(&mut request);
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let config = DeepSeekConfig::new(Settings {
            api_key: "test".into(),
            base_url: url,
            model: "test".into(),
            protocol,
        })
        .unwrap();
        let result = super::super::chat_messages_stream(
            &config,
            &[json!({"role":"user","content":"hi"})],
            None,
            &mut |_| Ok(()),
        );
        server.join().unwrap();
        result
    }

    fn chat_event(reason: &str, arguments: &str) -> Value {
        json!({"model":"test","choices":[{"index":0,"finish_reason":reason,
            "delta":{"tool_calls":[{"index":0,"id":"secret-id",
                "function":{"name":"secret-tool","arguments":arguments}}]}}],
            "usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}})
    }

    #[test]
    fn chat_terminal_does_not_accept_truncated_or_filtered_valid_calls() {
        for (reason, kind) in [
            ("length", "output_truncated"),
            ("content_filter", "content_filtered"),
        ] {
            let body = format!("data: {}\n\ndata: [DONE]\n\n", chat_event(reason, "{}"));
            let error = fixture(Protocol::Chat, "text/event-stream", body).unwrap_err();
            let error = error.downcast_ref::<CompletionError>().unwrap();
            assert_eq!(error.kind, kind);
            assert!(error.diagnostics.terminal_received);
            assert_eq!(error.diagnostics.finish_reason, Some(reason));
            assert_eq!(
                error
                    .diagnostics
                    .reported_usage
                    .as_ref()
                    .unwrap()
                    .total_tokens,
                5
            );
        }
    }

    #[test]
    fn invalid_arguments_have_bounded_diagnostics_without_argument_text() {
        let frame = chat_event("tool_calls", "{\"secret-value\":");
        let body = format!("data: {frame}\n\ndata: [DONE]\n\n");
        let error = fixture(Protocol::Chat, "text/event-stream", body).unwrap_err();
        let error = error.downcast_ref::<CompletionError>().unwrap();
        assert_eq!(error.kind, "invalid_tool_arguments");
        assert_eq!(
            error.diagnostics.argument_error.as_ref().unwrap().category,
            "eof"
        );
        assert_eq!(error.diagnostics.tools[0].argument_fragments, 1);
        let printed = format!(
            "{error:?} {error} {}",
            serde_json::to_string(&error.diagnostics).unwrap()
        );
        assert!(!printed.contains("secret-"));
    }

    #[test]
    fn interleaved_calls_are_all_validated_before_returning_a_reply() {
        let frame = json!({"choices":[{"delta":{"tool_calls":[
            {"index":2,"id":"a","function":{"name":"write","arguments":"{}"}},
            {"index":7,"id":"b","function":{"name":"write","arguments":"{"}}
        ]}}]});
        let delta = json!({"choices":[{"delta":{"tool_calls":[{"index":7,"function":{"arguments":"\"x\":"}}]},"finish_reason":"tool_calls"}]});
        let error = fixture(
            Protocol::Chat,
            "text/event-stream",
            format!("data: {frame}\n\ndata: {delta}\n\ndata: [DONE]\n\n"),
        )
        .unwrap_err();
        let error = error.downcast_ref::<CompletionError>().unwrap();
        assert_eq!(error.diagnostics.tool_call_count, 2);
        assert_eq!(error.diagnostics.tools[1].index, 7);
        assert_eq!(error.diagnostics.tools[1].argument_fragments, 2);
        assert_eq!(error.diagnostics.argument_error.as_ref().unwrap().index, 1);
    }

    #[test]
    fn missing_terminal_retains_counts_and_unknown_finish_reason() {
        let error = fixture(
            Protocol::Chat,
            "text/event-stream",
            format!("data: {}\n\n", chat_event("private-provider-reason", "{}")),
        )
        .unwrap_err();
        let error = error.downcast_ref::<CompletionError>().unwrap();
        assert_eq!(error.kind, "missing_terminal");
        assert!(!error.diagnostics.terminal_received);
        assert_eq!(error.diagnostics.finish_reason, Some("other"));
        assert_eq!(error.diagnostics.tool_call_count, 1);
    }

    #[test]
    fn responses_reject_incomplete_and_inconsistent_completed_status() {
        for event in ["response.incomplete", "response.completed"] {
            let body = format!(
                "event: {event}\ndata: {}\n\n",
                json!({"response":{
                    "status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},
                    "model":"test","output":[],"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}
                }})
            );
            let error = fixture(Protocol::Responses, "text/event-stream", body).unwrap_err();
            let error = error.downcast_ref::<CompletionError>().unwrap();
            assert_eq!(error.kind, "incomplete_response");
            assert!(error.diagnostics.terminal_received);
            assert_eq!(
                error
                    .diagnostics
                    .reported_usage
                    .as_ref()
                    .unwrap()
                    .total_tokens,
                5
            );
        }
    }

    #[test]
    fn json_fallback_rejects_explicit_incomplete_status() {
        let chat = json!({"model":"test","choices":[{"finish_reason":"length","message":{"content":"partial"}}]});
        let responses = json!({"model":"test","status":"incomplete","output":[]});
        for (protocol, body, kind) in [
            (Protocol::Chat, chat, "output_truncated"),
            (Protocol::Responses, responses, "incomplete_response"),
        ] {
            let error = fixture(protocol, "application/json", body.to_string()).unwrap_err();
            let error = error.downcast_ref::<CompletionError>().unwrap();
            assert_eq!(error.kind, kind);
            assert_eq!(error.diagnostics.transport, "json");
        }
    }

    #[test]
    fn messages_missing_usage_stays_unknown_and_partial_calls_keep_block_indices() {
        let reply = fixture(Protocol::Messages, "text/event-stream",
            "event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":4}}}\n\nevent: message_stop\ndata: {}\n\n".into()).unwrap();
        assert!(reply.usage.is_none());
        let error = fixture(Protocol::Messages, "text/event-stream",
            concat!("event: content_block_start\ndata: {\"index\":7,\"content_block\":{\"type\":\"tool_use\",\"id\":\"a\",\"name\":\"write\"}}\n\n",
            "event: content_block_delta\ndata: {\"index\":7,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\"}}\n\n",
            "event: message_stop\ndata: {}\n\n").into()).unwrap_err();
        let error = error.downcast_ref::<CompletionError>().unwrap();
        assert_eq!(error.diagnostics.tools[0].index, 7);
        assert_eq!(error.diagnostics.tools[0].argument_fragments, 1);
        assert_eq!(error.kind, "invalid_tool_arguments");
    }
}
