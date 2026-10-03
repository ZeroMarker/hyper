use anyhow::{Context, Result, bail};

use crate::model::SessionMessage;

const DEFAULT_HISTORY_TOKENS: usize = 16_000;
const MESSAGE_OVERHEAD: usize = 8;

#[derive(Clone, Copy, Debug)]
pub(crate) struct RequestBudget {
    pub context_tokens: usize,
    pub output_tokens: usize,
}

impl Default for RequestBudget {
    fn default() -> Self {
        Self {
            context_tokens: 128_000,
            output_tokens: 8192,
        }
    }
}

impl RequestBudget {
    pub fn from_env() -> Result<Self> {
        let read = |key| match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(error) => Err(error).with_context(|| format!("invalid {key}")),
        };
        let context = read("HYPER_CONTEXT_TOKENS")?;
        let output = read("HYPER_OUTPUT_TOKENS")?;
        Self::resolve(|key| match key {
            "HYPER_CONTEXT_TOKENS" => context.clone(),
            "HYPER_OUTPUT_TOKENS" => output.clone(),
            _ => None,
        })
    }

    fn resolve(env: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let defaults = Self::default();
        let value = |key: &str, default| -> Result<usize> {
            env(key).map_or(Ok(default), |value| {
                value
                    .parse::<usize>()
                    .with_context(|| format!("{key} must be a positive integer"))
            })
        };
        let budget = Self {
            context_tokens: value("HYPER_CONTEXT_TOKENS", defaults.context_tokens)?,
            output_tokens: value("HYPER_OUTPUT_TOKENS", defaults.output_tokens)?,
        };
        if budget.output_tokens == 0 || budget.context_tokens <= budget.output_tokens {
            bail!("HYPER_CONTEXT_TOKENS must exceed HYPER_OUTPUT_TOKENS, which must be positive");
        }
        Ok(budget)
    }

    pub fn input_tokens(self) -> usize {
        self.context_tokens.saturating_sub(self.output_tokens)
    }

    pub fn check(self, estimated: usize) -> Result<()> {
        if estimated > self.input_tokens() {
            bail!(
                "context budget exceeded: request estimates {estimated} input tokens, budget is {} ({} total minus {} output reserved); reduce the input or set HYPER_CONTEXT_TOKENS for your model",
                self.input_tokens(),
                self.context_tokens,
                self.output_tokens
            );
        }
        Ok(())
    }
}

pub(crate) fn history_budget() -> Result<usize> {
    match std::env::var("HYPER_HISTORY_TOKENS") {
        Ok(value) => parse_budget(&value),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_HISTORY_TOKENS),
        Err(error) => Err(error).context("invalid HYPER_HISTORY_TOKENS"),
    }
}

fn parse_budget(value: &str) -> Result<usize> {
    value
        .parse()
        .context("HYPER_HISTORY_TOKENS must be a non-negative integer")
}

/// A deliberately conservative estimate, independent of provider tokenizers:
/// one token per UTF-8 byte plus framing. This is a history budget only; input,
/// tool definitions and the current agent loop have separate costs.
pub(crate) fn estimated_tokens(message: &SessionMessage) -> usize {
    message.content.len().saturating_add(MESSAGE_OVERHEAD)
}

/// Keep a contiguous suffix of whole user-led turns. Never split a large turn
/// or skip it to fill the budget with older (and less relevant) conversation.
pub(crate) fn bounded_history(history: &[SessionMessage], budget: usize) -> &[SessionMessage] {
    let mut start = history.len();
    let mut end = history.len();
    let mut remaining = budget;
    for index in (0..history.len()).rev() {
        if history[index].role != "user" {
            continue;
        }
        let cost = history[index..end].iter().fold(0usize, |sum, message| {
            sum.saturating_add(estimated_tokens(message))
        });
        if cost > remaining {
            break;
        }
        remaining -= cost;
        start = index;
        end = index;
    }
    &history[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_budget_reserves_output_and_validates_configuration() {
        let defaults = RequestBudget::resolve(|_| None).unwrap();
        assert_eq!(defaults.input_tokens(), 119_808);
        let budget = RequestBudget::resolve(|key| {
            Some(if key == "HYPER_CONTEXT_TOKENS" {
                "100".into()
            } else {
                "20".into()
            })
        })
        .unwrap();
        assert!(budget.check(80).is_ok());
        assert!(
            budget
                .check(81)
                .unwrap_err()
                .to_string()
                .contains("context budget exceeded")
        );
        for (context, output) in [
            ("0", "1"),
            ("10", "0"),
            ("10", "10"),
            ("10", "11"),
            ("invalid", "1"),
            ("10", "-1"),
        ] {
            assert!(
                RequestBudget::resolve(|key| Some(if key == "HYPER_CONTEXT_TOKENS" {
                    context.into()
                } else {
                    output.into()
                }))
                .is_err()
            );
        }
    }

    fn message(role: &str, content: &str) -> SessionMessage {
        SessionMessage {
            role: role.into(),
            content: content.into(),
            timestamp: String::new(),
            run_id: None,
        }
    }

    #[test]
    fn budget_preserves_whole_recent_turns() {
        let history = vec![
            message("user", "old"),
            message("assistant", "old answer"),
            message("user", "新问题"),
            message("assistant", "新回答"),
        ];
        let cost: usize = history[2..].iter().map(estimated_tokens).sum();
        assert_eq!(bounded_history(&history, cost).len(), 2);
        assert_eq!(bounded_history(&history, cost)[0].content, "新问题");
        assert!(bounded_history(&history, cost - 1).is_empty());
        assert!(bounded_history(&history, 0).is_empty());
        assert_eq!(bounded_history(&history, usize::MAX).len(), 4);
        assert_eq!(estimated_tokens(&history[2]), 9 + MESSAGE_OVERHEAD);
    }

    #[test]
    fn oversized_latest_turn_does_not_restore_older_turns() {
        let history = vec![
            message("user", "a"),
            message("assistant", "b"),
            message("user", "c"),
            message("assistant", &"x".repeat(100)),
        ];
        assert!(bounded_history(&history, 30).is_empty());
    }

    #[test]
    fn leading_orphan_assistant_is_excluded() {
        let history = vec![
            message("assistant", "orphan"),
            message("user", "question"),
            message("assistant", "answer"),
        ];
        assert_eq!(bounded_history(&history, usize::MAX).len(), 2);
        assert!(bounded_history(&[], 100).is_empty());
    }

    #[test]
    fn budgets_are_validated() {
        assert_eq!(parse_budget("0").unwrap(), 0);
        assert_eq!(parse_budget("16000").unwrap(), DEFAULT_HISTORY_TOKENS);
        for value in ["", "-1", "abc", "1.5", "999999999999999999999999999999"] {
            assert!(parse_budget(value).is_err());
        }
    }
}
