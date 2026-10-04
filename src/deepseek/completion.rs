//! Bounded completion diagnostics; never include tool argument or provider error text.
use super::{Protocol, ToolCall, Usage};
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::fmt;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompletionDiagnostics {
    pub protocol: &'static str,
    pub transport: &'static str,
    pub terminal_received: bool,
    pub finish_reason: Option<&'static str>,
    pub frames_received: usize,
    pub text_bytes: usize,
    pub tool_call_count: usize,
    pub tools: Vec<ToolDiagnostics>,
    pub reported_usage: Option<Usage>,
    pub argument_error: Option<ArgumentError>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolDiagnostics {
    pub index: u64,
    pub argument_bytes: usize,
    pub argument_fragments: usize,
    pub has_id: bool,
    pub has_name: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArgumentError {
    pub index: usize,
    pub category: &'static str,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug)]
pub(crate) struct CompletionError {
    pub kind: &'static str,
    pub diagnostics: CompletionDiagnostics,
}
impl fmt::Display for CompletionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            "missing_terminal" => write!(f, "model stream ended before completion"),
            "invalid_tool_arguments" => {
                write!(f, "incomplete streamed tool arguments")?;
                if let Some(error) = &self.diagnostics.argument_error {
                    write!(
                        f,
                        ": {} at line {} column {}",
                        error.category, error.line, error.column
                    )?;
                }
                Ok(())
            }
            _ => write!(f, "model completion rejected: {}", self.kind),
        }
    }
}
impl std::error::Error for CompletionError {}

// Provider-controlled values are mapped into an allowlist before audit logging.
pub(crate) fn safe_reason(value: &Value) -> Option<&'static str> {
    Some(match value.as_str()? {
        "stop" => "stop",
        "tool_calls" => "tool_calls",
        "function_call" => "function_call",
        "length" => "length",
        "content_filter" => "content_filter",
        "completed" => "completed",
        "incomplete" => "incomplete",
        "failed" => "failed",
        "in_progress" => "in_progress",
        "max_output_tokens" => "max_output_tokens",
        "max_tokens" => "max_tokens",
        "cancelled" => "cancelled",
        "end_turn" => "end_turn",
        "tool_use" => "tool_use",
        "stop_sequence" => "stop_sequence",
        _ => "other",
    })
}

impl CompletionDiagnostics {
    pub fn new(protocol: Protocol, transport: &'static str) -> Self {
        Self {
            protocol: protocol.as_str(),
            transport,
            terminal_received: false,
            finish_reason: None,
            frames_received: 0,
            text_bytes: 0,
            tool_call_count: 0,
            tools: vec![],
            reported_usage: None,
            argument_error: None,
        }
    }
    pub fn reject(&self, kind: &'static str) -> anyhow::Error {
        CompletionError {
            kind,
            diagnostics: self.clone(),
        }
        .into()
    }
    pub fn observe_tools<'a>(&mut self, calls: impl Iterator<Item = (u64, &'a ToolCall, usize)>) {
        self.tools.clear();
        self.tool_call_count = 0;
        for (index, call, fragments) in calls {
            self.tool_call_count += 1;
            if self.tools.len() < 16 {
                self.tools.push(ToolDiagnostics {
                    index,
                    argument_bytes: call.function.arguments.len(),
                    argument_fragments: fragments,
                    has_id: !call.id.is_empty(),
                    has_name: !call.function.name.is_empty(),
                });
            }
        }
    }
    pub fn validate_calls(&mut self, calls: &[ToolCall]) -> Result<()> {
        if self.tools.is_empty() {
            self.observe_tools(
                calls
                    .iter()
                    .enumerate()
                    .map(|(index, call)| (index as u64, call, 1)),
            );
        }
        for (index, call) in calls.iter().enumerate() {
            if call.id.is_empty() || call.function.name.is_empty() {
                return Err(self.reject("incomplete_tool_call"));
            }
            let args =
                serde_json::from_str::<Value>(&call.function.arguments).map_err(|error| {
                    self.argument_error = Some(ArgumentError {
                        index,
                        category: match error.classify() {
                            serde_json::error::Category::Eof => "eof",
                            serde_json::error::Category::Syntax => "syntax",
                            serde_json::error::Category::Data => "data",
                            serde_json::error::Category::Io => "io",
                        },
                        line: error.line(),
                        column: error.column(),
                    });
                    self.reject("invalid_tool_arguments")
                })?;
            if !args.is_object() {
                return Err(self.reject("invalid_tool_arguments"));
            }
        }
        Ok(())
    }
    pub fn check_chat_reason(&self) -> Result<()> {
        match self.finish_reason {
            Some("length") => Err(self.reject("output_truncated")),
            Some("content_filter") => Err(self.reject("content_filtered")),
            _ => Ok(()), // Omitted legacy reason remains unknown, not fabricated.
        }
    }
}

#[derive(Debug)]
pub(crate) struct TextPublicationError;
impl fmt::Display for TextPublicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to publish model text")
    }
}
impl std::error::Error for TextPublicationError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_tool_list_is_bounded_without_losing_total_count() {
        let calls: Vec<_> = (0..20)
            .map(|_| ToolCall {
                id: "private-id".into(),
                call_type: "function".into(),
                function: super::super::ToolFunction {
                    name: "private-name".into(),
                    arguments: "{}".into(),
                },
            })
            .collect();
        let mut audit = CompletionDiagnostics::new(Protocol::Chat, "sse");
        audit.observe_tools(
            calls
                .iter()
                .enumerate()
                .map(|(index, call)| (index as u64, call, 2)),
        );
        assert_eq!(audit.tool_call_count, 20);
        assert_eq!(audit.tools.len(), 16);
        assert!(!serde_json::to_string(&audit).unwrap().contains("private"));
    }
}
