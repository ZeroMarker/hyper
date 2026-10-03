use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc::Sender},
    time::Duration,
};

use crate::workspace;
use crate::{CancellationToken, cancellation::POLL_INTERVAL};

/// How long the engine waits for the UI to answer an approval request before
/// treating it as denied. Acts as a backstop for a vanished TUI.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(600);

/// A pending user-approval question. The engine blocks on the response channel
/// until the UI sends `true` (allow) or `false` (deny).
pub struct ApprovalRequest {
    pub id: String,
    pub tool: String,
    pub detail: String,
    pub response: Sender<bool>,
}

/// Shared gate between the engine (running a task in a worker thread) and the
/// TUI main loop. The engine pushes requests through `request`, the UI drains
/// them with `drain` and answers via the request's response channel.
#[derive(Clone, Default)]
pub struct ApprovalGate {
    queue: Arc<Mutex<VecDeque<ApprovalRequest>>>,
}

impl ApprovalGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Block until the UI approves or denies the given action. Without a UI
    /// (timeout) the request is denied.
    pub fn request(&self, tool: &str, detail: &str) -> bool {
        self.request_cancellable(tool, detail, &CancellationToken::new())
            .unwrap_or(false)
    }

    pub(crate) fn request_cancellable(
        &self,
        tool: &str,
        detail: &str,
        cancellation: &CancellationToken,
    ) -> anyhow::Result<bool> {
        cancellation.check()?;
        let (response, receiver) = std::sync::mpsc::channel();
        let id = workspace::id();
        {
            let mut queue = self.queue.lock().expect("approval queue poisoned");
            queue.push_back(ApprovalRequest {
                id: id.clone(),
                tool: tool.into(),
                detail: detail.into(),
                response,
            });
        }
        let started = std::time::Instant::now();
        let result = loop {
            if let Err(error) = cancellation.check() {
                break Err(error);
            }
            if started.elapsed() >= APPROVAL_TIMEOUT {
                break Ok(false);
            }
            match receiver.recv_timeout(POLL_INTERVAL) {
                Ok(answer) => break cancellation.check().map(|()| answer),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Ok(false),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        };
        self.queue
            .lock()
            .expect("approval queue poisoned")
            .retain(|request| request.id != id);
        result
    }

    /// Take every pending request out of the queue (non-blocking).
    pub fn drain(&self) -> Vec<ApprovalRequest> {
        let mut queue = self.queue.lock().expect("approval queue poisoned");
        queue.drain(..).collect()
    }

    /// Deny everything still pending (e.g. on TUI quit).
    pub fn deny_all(&self) {
        for request in self.drain() {
            let _ = request.response.send(false);
        }
    }
}
