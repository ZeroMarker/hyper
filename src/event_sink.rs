use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use crate::model::HarnessEvent;

const MAX_PENDING: usize = 128;

/// A bounded handoff from the task worker to the TUI. The audit log remains
/// authoritative; the UI can safely skip old updates if it falls behind.
#[derive(Clone, Default)]
pub struct EventSink {
    queue: Arc<Mutex<VecDeque<String>>>,
}

impl EventSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, event: &HarnessEvent) {
        let mut queue = self.queue.lock().expect("event queue poisoned");
        let line = event_line(event);
        if queue.back() == Some(&line) {
            return;
        }
        if queue.len() == MAX_PENDING {
            queue.pop_front();
        }
        queue.push_back(line);
    }

    pub fn drain(&self) -> Vec<String> {
        self.queue
            .lock()
            .expect("event queue poisoned")
            .drain(..)
            .collect()
    }
}

fn event_line(event: &HarnessEvent) -> String {
    let detail = match event.event_type.as_str() {
        "model.started" => event.payload.get("model"),
        "tool.started" | "tool.finished" => event.payload.get("tool"),
        "step.failed" | "run.failed" => event.payload.get("message"),
        _ => None,
    }
    .and_then(serde_json::Value::as_str)
    .unwrap_or("");
    let step = event.step_id.as_deref().unwrap_or("");
    format!("{} {} {}", event.event_type, step, detail)
        .trim()
        .chars()
        .take(120)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queue_is_bounded_and_does_not_copy_large_payloads() {
        let sink = EventSink::new();
        for index in 0..MAX_PENDING + 10 {
            sink.push(&HarnessEvent {
                event_id: index.to_string(),
                run_id: "run".into(),
                task_id: "task".into(),
                event_type: "tool.finished".into(),
                timestamp: "now".into(),
                step_id: Some(index.to_string()),
                step_index: Some(index),
                payload: json!({"tool":"read","content":"private output"}),
            });
        }
        let lines = sink.drain();
        assert_eq!(lines.len(), MAX_PENDING);
        assert!(lines[0].contains("10"));
        assert!(lines.iter().all(|line| !line.contains("private output")));
        assert!(sink.drain().is_empty());

        let event = HarnessEvent {
            event_id: "repeat".into(),
            run_id: "run".into(),
            task_id: "task".into(),
            event_type: "model.iteration".into(),
            timestamp: "now".into(),
            step_id: None,
            step_index: None,
            payload: json!({}),
        };
        sink.push(&event);
        sink.push(&event);
        assert_eq!(sink.drain().len(), 1);
    }
}
