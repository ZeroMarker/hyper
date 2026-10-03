use std::{
    collections::VecDeque,
    io::Write,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};

use crate::model::HarnessEvent;

const MAX_PENDING: usize = 128;
const MAX_PENDING_TEXT: usize = 128 * 1024;

/// A persisted-event destination: bounded TUI updates by default, or a
/// lossless JSONL writer. The audit log remains authoritative in both modes.
#[derive(Clone, Default)]
pub struct EventSink {
    queue: Arc<Mutex<VecDeque<String>>>,
    text: Arc<Mutex<String>>,
    jsonl: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
}

impl EventSink {
    /// CLI pipe writes must not prevent signal cancellation. A writable pipe
    /// still receives terminal events; a stalled/closed pipe cannot hold the
    /// engine hostage after cancellation. The persisted log is authoritative.
    pub(crate) fn cancellable_stdout(cancellation: crate::CancellationToken) -> Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self::jsonl(std::io::BufWriter::new(
                CancellableStdout::new(cancellation)?,
            )))
        }
        #[cfg(not(unix))]
        {
            let _ = cancellation;
            Ok(Self::jsonl(std::io::stdout()))
        }
    }
    pub fn new() -> Self {
        Self::default()
    }

    /// A lossless synchronous event stream. Flush each persisted event so
    /// consumers see progress while the task runs; slow consumers apply
    /// backpressure instead of dropping audit events.
    pub fn jsonl(writer: impl Write + Send + 'static) -> Self {
        Self {
            jsonl: Some(Arc::new(Mutex::new(Box::new(writer)))),
            ..Self::default()
        }
    }

    pub(crate) fn publish(&self, event: &HarnessEvent) -> Result<()> {
        if let Some(writer) = &self.jsonl {
            let mut writer = writer.lock().expect("JSONL output poisoned");
            serde_json::to_writer(&mut **writer, event).context("failed to write JSONL event")?;
            writeln!(writer).context("failed to write JSONL newline")?;
            writer.flush().context("failed to flush JSONL event")?;
        } else {
            self.push(event);
        }
        Ok(())
    }

    pub fn push(&self, event: &HarnessEvent) {
        if event.event_type == "model.delta" {
            if let Some(delta) = event
                .payload
                .get("content")
                .and_then(serde_json::Value::as_str)
            {
                let mut text = self.text.lock().expect("text queue poisoned");
                text.push_str(delta);
                if text.len() > MAX_PENDING_TEXT {
                    let mut drop = text.len() - MAX_PENDING_TEXT;
                    while !text.is_char_boundary(drop) {
                        drop += 1;
                    }
                    text.drain(..drop);
                }
            }
            return;
        }
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

    pub fn take_text(&self) -> String {
        std::mem::take(&mut *self.text.lock().expect("text queue poisoned"))
    }
}

#[cfg(unix)]
struct CancellableStdout {
    fd: std::os::fd::OwnedFd,
    original_flags: i32,
    cancellation: crate::CancellationToken,
}

#[cfg(unix)]
impl CancellableStdout {
    fn new(cancellation: crate::CancellationToken) -> std::io::Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd};
        std::io::stdout().flush()?;
        // dup shares the open-file-description flags with stdout. Restore them
        // before the owned descriptor is closed, including every error path.
        let raw = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
        let original_flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if original_flags < 0
            || unsafe {
                libc::fcntl(
                    fd.as_raw_fd(),
                    libc::F_SETFL,
                    original_flags | libc::O_NONBLOCK,
                )
            } < 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            fd,
            original_flags,
            cancellation,
        })
    }
}

#[cfg(unix)]
impl Write for CancellableStdout {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        use std::os::fd::AsRawFd;
        loop {
            let count =
                unsafe { libc::write(self.fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
            if count >= 0 {
                return Ok(count as usize);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() != std::io::ErrorKind::WouldBlock {
                return Err(error);
            }
            self.cancellation.check().map_err(std::io::Error::other)?;
            let mut poll = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            let result = unsafe { libc::poll(&mut poll, 1, 25) };
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for CancellableStdout {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        let _ = unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_SETFL, self.original_flags) };
    }
}

fn event_line(event: &HarnessEvent) -> String {
    if event.event_type == "model.context_budget" {
        return format!(
            "context {}/{} +{} output{}",
            event.payload["estimatedInputTokens"],
            event.payload["maxInputTokens"],
            event.payload["outputTokens"],
            if event.payload["fits"] == false {
                " exceeded"
            } else {
                ""
            }
        );
    }
    let detail = match event.event_type.as_str() {
        "model.started" => event.payload.get("model"),
        "tool.started" | "tool.finished" | "tool.cancelled" => event.payload.get("tool"),
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
