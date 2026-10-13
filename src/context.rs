use anyhow::{Context, Result, bail};

use crate::model::{Compaction, SessionMessage};

/// Marks a synthetic summary message in the projected history. It is a fixed,
/// labeled block so the recorded request and a replay carry the same bytes and
/// the model can tell a summary apart from the user's own words.
pub(crate) const SUMMARY_OPEN: &str = "<session_summary>";
pub(crate) const SUMMARY_CLOSE: &str = "</session_summary>";
/// The same, for a summary that replaces older agent-loop turns. Kept distinct
/// from the session marker so a replay and an audit can tell them apart.
pub(crate) const LOOP_SUMMARY_OPEN: &str = "<loop_summary>";
pub(crate) const LOOP_SUMMARY_CLOSE: &str = "</loop_summary>";

pub(crate) fn loop_summary_content(summary: &str) -> String {
    format!("{LOOP_SUMMARY_OPEN}\n{summary}\n{LOOP_SUMMARY_CLOSE}")
}

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

/// Whether a run may summarize an over-budget conversation on its own. Off by
/// default, so an existing conversation and its evaluation cost are unchanged;
/// set `HYPER_AUTOCOMPACT` to a true value to enable it.
pub(crate) fn auto_compaction_enabled() -> Result<bool> {
    match std::env::var("HYPER_AUTOCOMPACT") {
        Ok(value) => parse_flag(&value),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(error) => Err(error).context("invalid HYPER_AUTOCOMPACT"),
    }
}

fn parse_flag(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Ok(true),
        "" | "0" | "false" | "off" | "no" => Ok(false),
        other => bail!("HYPER_AUTOCOMPACT must be a boolean, got {other:?}"),
    }
}

/// How many of the most recent conversation turns automatic compaction leaves
/// unsummarized. When the history is over budget, everything older is folded
/// into one summary.
pub(crate) fn auto_compaction_keep_turns() -> Result<usize> {
    match std::env::var("HYPER_AUTOCOMPACT_KEEP_TURNS") {
        Ok(value) => value
            .parse()
            .context("HYPER_AUTOCOMPACT_KEEP_TURNS must be a non-negative integer"),
        Err(std::env::VarError::NotPresent) => Ok(1),
        Err(error) => Err(error).context("invalid HYPER_AUTOCOMPACT_KEEP_TURNS"),
    }
}

/// Whether an agent loop may summarize its own older tool turns when the next
/// request would exceed the input budget. Off by default; `HYPER_LOOP_COMPACT`
/// enables it.
pub(crate) fn loop_compaction_enabled() -> Result<bool> {
    match std::env::var("HYPER_LOOP_COMPACT") {
        Ok(value) => parse_flag(&value),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(error) => Err(error).context("invalid HYPER_LOOP_COMPACT"),
    }
}

/// How many of the loop's most recent turns (an assistant turn with its tool
/// results) in-loop compaction leaves unsummarized.
pub(crate) fn loop_compaction_keep_groups() -> Result<usize> {
    match std::env::var("HYPER_LOOP_COMPACT_KEEP") {
        Ok(value) => value
            .parse()
            .context("HYPER_LOOP_COMPACT_KEEP must be a non-negative integer"),
        Err(std::env::VarError::NotPresent) => Ok(2),
        Err(error) => Err(error).context("invalid HYPER_LOOP_COMPACT_KEEP"),
    }
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

/// The summary message a compaction contributes to the projected history.
pub(crate) fn summary_message(summary: &str, timestamp: &str) -> SessionMessage {
    SessionMessage {
        role: "user".into(),
        content: format!("{SUMMARY_OPEN}\n{summary}\n{SUMMARY_CLOSE}"),
        timestamp: timestamp.into(),
        run_id: None,
    }
}

fn compaction_summary_message(compaction: &Compaction) -> SessionMessage {
    summary_message(&compaction.summary, &compaction.created_at)
}

/// Project a transcript through a finished compaction: the summary stands in
/// for the first `coveredMessages` transcript messages, and the rest of the
/// transcript is kept verbatim. A compaction that was not finished, or whose
/// summary is empty, is ignored so a failed attempt never loses history.
pub(crate) fn project_history(
    history: &[SessionMessage],
    compaction: Option<&Compaction>,
) -> Vec<SessionMessage> {
    let Some(compaction) = compaction
        .filter(|compaction| compaction.is_finished() && !compaction.summary.trim().is_empty())
    else {
        return history.to_vec();
    };
    let covered = compaction.covered_messages.min(history.len());
    let mut projected = Vec::with_capacity(history.len() - covered + 1);
    projected.push(compaction_summary_message(compaction));
    projected.extend_from_slice(&history[covered..]);
    projected
}

/// The index where the `keep_turns` most recent user-led turns begin, which is
/// exactly how many leading transcript messages a compaction would cover.
/// Keeping zero turns asks for the whole transcript; keeping more turns than
/// exist leaves nothing to summarize (index 0).
pub(crate) fn keep_suffix_start(history: &[SessionMessage], keep_turns: usize) -> usize {
    if keep_turns == 0 {
        return history.len();
    }
    let mut seen = 0;
    for index in (0..history.len()).rev() {
        if history[index].role == "user" {
            seen += 1;
            if seen == keep_turns {
                return index;
            }
        }
    }
    0
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

    #[test]
    fn auto_compaction_flags_are_parsed() {
        for value in ["1", "true", "ON", " yes "] {
            assert!(parse_flag(value).unwrap(), "{value:?} should enable");
        }
        for value in ["", "0", "false", "off", "no", "False"] {
            assert!(!parse_flag(value).unwrap(), "{value:?} should disable");
        }
        for value in ["maybe", "2", "-1"] {
            assert!(parse_flag(value).is_err(), "{value:?} should be rejected");
        }
    }

    fn compaction(status: &str, covered: usize, summary: &str) -> Compaction {
        Compaction {
            id: "c1".into(),
            status: status.into(),
            covered_messages: covered,
            summary: summary.into(),
            error_type: None,
            error: None,
            provider: "stub".into(),
            model: "stub".into(),
            usage: None,
            covered_estimated_tokens: 0,
            summary_estimated_tokens: 0,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            trigger: "manual".into(),
        }
    }

    #[test]
    fn a_finished_compaction_replaces_the_covered_prefix() {
        let history = vec![
            message("user", "one"),
            message("assistant", "answer 1"),
            message("user", "two"),
            message("assistant", "answer 2"),
        ];
        let projected = project_history(&history, Some(&compaction("finished", 2, "summary")));
        assert_eq!(projected.len(), 3);
        assert_eq!(projected[0].role, "user");
        assert_eq!(
            projected[0].content,
            format!("{SUMMARY_OPEN}\nsummary\n{SUMMARY_CLOSE}")
        );
        assert_eq!(projected[1].content, "two");
        assert_eq!(projected[2].content, "answer 2");

        // A coverage past the end is clamped rather than panicking.
        let projected = project_history(&history, Some(&compaction("finished", 99, "summary")));
        assert_eq!(projected.len(), 1);
    }

    #[test]
    fn only_finished_compactions_project() {
        let history = vec![message("user", "one"), message("assistant", "a")];
        for record in [
            compaction("failed", 2, "summary"),
            compaction("cancelled", 2, "summary"),
            compaction("finished", 2, "   "),
        ] {
            assert_eq!(project_history(&history, Some(&record)).len(), 2);
        }
        assert_eq!(project_history(&history, None).len(), 2);
    }

    #[test]
    fn keep_suffix_start_counts_only_user_turns() {
        let history = vec![
            message("user", "one"),
            message("assistant", "a"),
            message("user", "two"),
            message("assistant", "b"),
            message("user", "three"),
            message("assistant", "c"),
        ];
        assert_eq!(keep_suffix_start(&history, 1), 4);
        assert_eq!(keep_suffix_start(&history, 2), 2);
        assert_eq!(keep_suffix_start(&history, 3), 0);
        assert_eq!(keep_suffix_start(&history, 4), 0);
        assert_eq!(keep_suffix_start(&history, 0), 6);
        assert_eq!(keep_suffix_start(&[], 1), 0);
    }
}
