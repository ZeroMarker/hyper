use super::*;

/// Fixed instructions for the summarization call. The summary is guidance the
/// model reads back on later turns, not an instruction the host executes.
const COMPACTION_SYSTEM: &str = "You compress the earlier turns of a coding conversation into a durable summary that a later run can read in place of those turns. Preserve, in this order: the user's current goal; hard constraints and requirements; files created or modified (with paths); decisions already made; and unfinished work with the next step. Keep exact identifiers, paths, commands and error text. Do not invent details. Reply with the summary only, in Markdown.";
const COMPACTION_INSTRUCTION: &str = "Summarize the conversation prefix below so it can stand in for those turns. Reply with the summary only.";

/// Summarize the older turns of a conversation into an auditable record.
///
/// The transcript is never rewritten. A finished record is appended under
/// `sessions/<id>.compactions.jsonl`; the newest finished record then projects
/// later runs as `summary` followed by the untranslated transcript suffix, and
/// the projection is what `model.started` records. A failed or cancelled
/// attempt is also recorded, so the audit is terminal without losing the usable
/// history; the call is made once and never retried here.
pub fn compact_session(
    root: impl AsRef<Path>,
    session_id: &str,
    keep_turns: usize,
    cancellation: CancellationToken,
) -> Result<Compaction> {
    let workspace = Workspace::open(root)?;
    workspace::validate_session_id(session_id)?;
    let history = workspace.session_messages(session_id)?;
    if history.is_empty() {
        bail!("session not found or empty: {session_id}");
    }
    let prior = workspace.active_compaction(session_id)?;
    let prior_covered = prior
        .as_ref()
        .map_or(0, |compaction| compaction.covered_messages)
        .min(history.len());
    let covered = keep_suffix_start(&history, keep_turns);
    if covered <= prior_covered {
        bail!(
            "nothing to compact: {prior_covered} message(s) of session {session_id} are already \
             summarized and keeping {keep_turns} turn(s) would summarize {covered}"
        );
    }
    let mut config = DeepSeekConfig::from_env()?;
    config.cancellation = cancellation;
    let record = run_compaction(
        &workspace,
        session_id,
        &history,
        prior.as_ref(),
        covered,
        "manual",
        &config,
    )?;
    if !record.is_finished() {
        bail!(
            "{}",
            record
                .error
                .clone()
                .unwrap_or_else(|| format!("compaction {}", record.status))
        );
    }
    Ok(record)
}

/// One summarization attempt: append the record and return it. A model failure
/// or cancellation is captured as a terminal record and returned as `Ok`; only
/// a failure to append the record is an `Err`. The transcript slice being
/// replaced is an earlier summary (if any) followed by the newly covered
/// transcript messages, so the newest summary always folds in the older one.
fn run_compaction(
    workspace: &Workspace,
    session_id: &str,
    history: &[SessionMessage],
    prior: Option<&Compaction>,
    covered: usize,
    trigger: &str,
    config: &DeepSeekConfig,
) -> Result<Compaction> {
    let prior_covered = prior
        .map_or(0, |compaction| compaction.covered_messages)
        .min(history.len());
    let mut replaced: Vec<SessionMessage> = Vec::new();
    if let Some(prior) = prior.filter(|prior| !prior.summary.trim().is_empty()) {
        replaced.push(summary_message(&prior.summary, &prior.created_at));
    }
    replaced.extend_from_slice(&history[prior_covered..covered]);
    let covered_estimated_tokens: usize = replaced.iter().map(estimated_tokens).sum();

    let created_at = now();
    let id = workspace::id();
    let failed = |kind: &str, message: String| Compaction {
        id: id.clone(),
        status: if kind == "Cancelled" {
            "cancelled".to_owned()
        } else {
            "failed".to_owned()
        },
        covered_messages: covered,
        summary: String::new(),
        error_type: Some(kind.to_owned()),
        error: Some(message),
        provider: config.provider.clone(),
        model: config.model.clone(),
        usage: None,
        covered_estimated_tokens,
        summary_estimated_tokens: 0,
        created_at: created_at.clone(),
        trigger: trigger.to_owned(),
    };
    let replaced_values: Vec<Value> = replaced
        .iter()
        .map(|message| json!({"role":message.role,"content":message.content}))
        .collect();
    let reply = match chat_messages(
        config,
        &compaction_request(&replaced_values, config.budget.input_tokens()),
        None,
    ) {
        Ok(reply) => reply,
        Err(error) => {
            let kind = if error.is::<Cancelled>() {
                "Cancelled"
            } else {
                "CompactionError"
            };
            let record = failed(kind, format!("{error:#}"));
            workspace.append_compaction(session_id, &record)?;
            return Ok(record);
        }
    };
    let summary = reply.content.trim();
    if summary.is_empty() {
        let record = failed(
            "CompactionError",
            "the model returned an empty summary".to_owned(),
        );
        workspace.append_compaction(session_id, &record)?;
        return Ok(record);
    }
    let record = Compaction {
        id,
        status: "finished".to_owned(),
        covered_messages: covered,
        summary: summary.to_owned(),
        error_type: None,
        error: None,
        provider: config.provider.clone(),
        model: reply.model.clone(),
        usage: reply.usage.map(serde_json::to_value).transpose()?,
        covered_estimated_tokens,
        summary_estimated_tokens: estimated_tokens(&summary_message(summary, &created_at)),
        created_at,
        trigger: trigger.to_owned(),
    };
    workspace.append_compaction(session_id, &record)?;
    Ok(record)
}

/// Summarize an over-budget conversation prefix before a run, in place of the
/// silent `bounded_history` drop. Off by default (`HYPER_AUTOCOMPACT=1` enables);
/// a failed attempt keeps the full history, is recorded, and is not retried
/// until the coverage boundary moves. Auto-compaction never fails the run: with
/// no summary the run still proceeds with the bounded history it had.
pub(super) fn auto_compact(
    events: &mut EventWriter<'_>,
    workspace: &Workspace,
    full: &[SessionMessage],
    session_id: &str,
) -> Result<()> {
    if !auto_compaction_enabled()? {
        return Ok(());
    }
    let budget = history_budget()?;
    let projected_tokens: usize = events.history.iter().map(estimated_tokens).sum();
    if projected_tokens <= budget {
        return Ok(());
    }
    let keep_turns = auto_compaction_keep_turns()?;
    let covered = keep_suffix_start(full, keep_turns);
    let prior_covered = events
        .compaction
        .as_ref()
        .map_or(0, |compaction| compaction.covered_messages)
        .min(full.len());
    if covered <= prior_covered {
        // Nothing left to summarize even though the projection is over budget.
        return Ok(());
    }
    // Never repeat the exact same failing attempt on every later run: the
    // history is kept, so an unchanged boundary stays as it is.
    if let Some(last) = workspace.compactions(session_id)?.last()
        && !last.is_finished()
        && last.covered_messages == covered
    {
        return Ok(());
    }
    let mut config = match events.model_config() {
        Ok(config) => config,
        Err(error) => {
            events.write(
                "compaction.failed",
                json!({"trigger":"auto","reason":"provider","message":format!("{error:#}")}),
                None,
                None,
            )?;
            return Ok(());
        }
    };
    config.cancellation = events.cancellation.clone();
    events.write(
        "compaction.started",
        json!({"trigger":"auto","coveredMessages":covered,"keepTurns":keep_turns}),
        None,
        None,
    )?;
    match run_compaction(
        workspace,
        session_id,
        full,
        events.compaction.as_ref(),
        covered,
        "auto",
        &config,
    ) {
        Ok(record) if record.is_finished() => {
            events.write(
                "compaction.finished",
                serde_json::to_value(&record)?,
                None,
                None,
            )?;
            events.history = project_history(full, Some(&record));
            events.compaction = Some(record);
        }
        Ok(record) => {
            events.write(
                "compaction.failed",
                json!({"trigger":"auto","record":record}),
                None,
                None,
            )?;
        }
        Err(error) => {
            events.write(
                "compaction.failed",
                json!({"trigger":"auto","message":format!("{error:#}")}),
                None,
                None,
            )?;
        }
    }
    Ok(())
}

/// The request a compaction sends: the fixed instruction plus the prefix being
/// replaced. No tools are advertised, so the reply is plain text. Tool calls
/// are rendered with their names and arguments, since an assistant turn that
/// calls tools carries no text and the summary must still say what was run.
/// The transcript is cut to half the input budget (JSON escaping inflates it),
/// keeping its beginning, where the goal lives, and its end, where the latest
/// state is, so the summarization request itself fits when the content that
/// triggered compaction did not.
fn compaction_request(replaced: &[Value], input_budget: usize) -> Vec<Value> {
    let mut transcript = String::new();
    for message in replaced {
        transcript.push_str("### ");
        transcript.push_str(message["role"].as_str().unwrap_or("message"));
        if let Some(id) = message["tool_call_id"].as_str() {
            transcript.push_str(" (result of ");
            transcript.push_str(id);
            transcript.push(')');
        }
        transcript.push('\n');
        transcript.push_str(message["content"].as_str().unwrap_or_default());
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            transcript.push_str("\n[tool call ");
            transcript.push_str(call["id"].as_str().unwrap_or_default());
            transcript.push_str(": ");
            transcript.push_str(call["function"]["name"].as_str().unwrap_or_default());
            transcript.push(' ');
            transcript.push_str(call["function"]["arguments"].as_str().unwrap_or_default());
            transcript.push(']');
        }
        transcript.push_str("\n\n");
    }
    let limit = (input_budget / 2).saturating_sub(COMPACTION_SYSTEM.len() + 1024);
    let transcript = clip_middle(&transcript, limit);
    vec![
        json!({"role":"system","content":COMPACTION_SYSTEM}),
        json!({"role":"user","content":format!("{COMPACTION_INSTRUCTION}\n\n{transcript}")}),
    ]
}

/// Keep the first quarter and the last three quarters of `limit` bytes of
/// `text`, with an explicit marker for the omitted middle, on char boundaries.
fn clip_middle(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let head = limit / 4;
    let tail = limit - head;
    let mut head_end = head.min(text.len());
    while !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len().saturating_sub(tail);
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}\n[... {} bytes omitted to fit the summarization budget ...]\n{}",
        &text[..head_end],
        tail_start.saturating_sub(head_end),
        &text[tail_start..]
    )
}

/// The start offsets of the loop's turns in `messages`: an assistant turn and
/// the tool results that answer it form one group, as does a standalone user
/// message (verification feedback). Compaction always cuts at a group boundary,
/// so a tool call is never separated from its result.
fn loop_group_boundaries(messages: &[Value]) -> Vec<usize> {
    let mut bounds = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        bounds.push(index);
        let has_calls = messages[index]["role"] == "assistant"
            && messages[index]["tool_calls"]
                .as_array()
                .is_some_and(|calls| !calls.is_empty());
        if has_calls {
            index += 1;
            while index < messages.len() && messages[index]["role"] == "tool" {
                index += 1;
            }
        } else {
            index += 1;
        }
    }
    bounds
}

/// Summarize the older turns of an in-flight agent loop in place, so a request
/// that would exceed the input budget still fits. Only messages after the fixed
/// prefix (system, session history, and the current user input) are touched;
/// the cut is always at a turn boundary, so tool call/result pairs stay whole.
/// Returns whether anything was compacted; a failed or empty summary leaves the
/// messages untouched and is recorded as `model.loop_summary_failed`, so the
/// normal budget check still reports the real error.
#[allow(clippy::too_many_arguments)]
pub(super) fn compact_loop(
    events: &EventWriter<'_>,
    config: &DeepSeekConfig,
    messages: &mut Vec<Value>,
    input_index: usize,
    keep_groups: usize,
    turn: usize,
    step: &StepSpec,
    index: usize,
) -> Result<bool> {
    let start = input_index + 1;
    if start >= messages.len() {
        return Ok(false);
    }
    let bounds = loop_group_boundaries(&messages[start..]);
    let keep_from = if keep_groups == 0 {
        messages.len() - start
    } else {
        if bounds.len() <= keep_groups {
            return Ok(false);
        }
        bounds[bounds.len() - keep_groups]
    };
    if keep_from == 0 {
        return Ok(false);
    }
    let replaced = messages[start..start + keep_from].to_vec();
    let kept = messages[start + keep_from..].to_vec();
    let reply = match chat_messages(
        config,
        &compaction_request(&replaced, config.budget.input_tokens()),
        None,
    ) {
        Ok(reply) => reply,
        Err(error) => {
            events.write(
                "model.loop_summary_failed",
                json!({"turn":turn,"message":format!("{error:#}")}),
                Some(&step.id),
                Some(index),
            )?;
            return Ok(false);
        }
    };
    let summary = reply.content.trim();
    if summary.is_empty() {
        events.write(
            "model.loop_summary_failed",
            json!({"turn":turn,"message":"the model returned an empty summary"}),
            Some(&step.id),
            Some(index),
        )?;
        return Ok(false);
    }
    messages.truncate(start);
    messages.push(json!({"role":"user","content":loop_summary_content(summary)}));
    messages.extend(kept);
    events.write(
        "model.loop_summary",
        json!({
            "turn":turn,
            // How many messages a replay removes at the same boundary before
            // inserting the summary back: the compaction must be reproducible.
            "removedMessages":keep_from,
            "summary":summary,
            "model":reply.model,
            "usage":serde_json::to_value(&reply.usage)?,
        }),
        Some(&step.id),
        Some(index),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_request_names_tool_calls_and_fits_the_budget() {
        let call = json!({"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"bash","arguments":"{\"cmd\":\"cargo test\"}"}}]});
        let tool = json!({"role":"tool","tool_call_id":"c1","content":"ok"});
        let request = compaction_request(&[call, tool], 100_000);
        let text = request[1]["content"].as_str().unwrap();
        assert!(
            text.contains("bash") && text.contains("cargo test"),
            "{text}"
        );
        assert!(text.contains("result of c1"));

        let big = json!({"role":"user","content":"é".repeat(50_000)});
        let request = compaction_request(&[big], 20_000);
        let text = request[1]["content"].as_str().unwrap();
        assert!(text.len() < 10_000, "{}", text.len());
        assert!(text.contains("bytes omitted"));
    }

    #[test]
    fn loop_groups_keep_tool_calls_with_their_results() {
        let call = json!({"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"bash","arguments":"{}"}}]});
        let tool = json!({"role":"tool","tool_call_id":"c1","content":"out"});
        let user = json!({"role":"user","content":"feedback"});
        let text = json!({"role":"assistant","content":"hi"});
        assert_eq!(
            loop_group_boundaries(&[
                call.clone(),
                tool.clone(),
                call.clone(),
                tool.clone(),
                user.clone()
            ]),
            vec![0, 2, 4]
        );
        // An assistant message without calls and a user message are each their
        // own group, so a cut between them is always safe.
        assert_eq!(loop_group_boundaries(&[text, user]), vec![0, 1]);
        assert!(loop_group_boundaries(&[]).is_empty());
    }
}
