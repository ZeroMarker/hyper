use std::{
    fmt,
    future::Future,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};

pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One run's cooperative cancellation source. Clones share state; a new run
/// gets a fresh token so cancelling a conversation turn does not poison resume.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<CancellationState>);

#[derive(Debug, Default)]
struct CancellationState {
    cancelled: AtomicBool,
    parent: Option<Arc<CancellationState>>,
}

#[derive(Debug)]
pub struct Cancelled;
impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("run cancelled by user")
    }
}
impl std::error::Error for Cancelled {}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        let mut state = Some(self.0.as_ref());
        while let Some(current) = state {
            if current.cancelled.load(Ordering::SeqCst) {
                return true;
            }
            state = current.parent.as_deref();
        }
        false
    }
    // Local TUI cancellation affects only a turn; an external process signal
    // cancels its child directly even when the UI cannot poll or redraw.
    pub(crate) fn child(&self) -> Self {
        Self(Arc::new(CancellationState {
            parent: Some(self.0.clone()),
            ..CancellationState::default()
        }))
    }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Cancelled.into())
        } else {
            Ok(())
        }
    }

    pub(crate) fn wait(&self, duration: Duration) -> Result<()> {
        let started = Instant::now();
        loop {
            self.check()?;
            let remaining = duration.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Ok(());
            }
            std::thread::sleep(remaining.min(POLL_INTERVAL));
        }
    }

    /// Only transport futures run on this runtime. Parsers, event callbacks and
    /// tools remain on the calling engine thread. Dropping the losing request
    /// or body-read future stops that operation without a detached agent worker.
    pub(crate) fn io<T, F: Future<Output = reqwest::Result<T>>>(
        &self,
        future: impl FnOnce() -> F,
    ) -> Result<T> {
        static RUNTIME: OnceLock<std::io::Result<tokio::runtime::Runtime>> = OnceLock::new();
        let runtime = RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
        });
        let runtime = runtime
            .as_ref()
            .map_err(|error| anyhow!("could not start model transport: {error}"))?;
        self.check()?;
        runtime.block_on(async {
            tokio::select! {
                biased;
                _ = async {
                    while !self.is_cancelled() { tokio::time::sleep(POLL_INTERVAL).await; }
                } => Err(Cancelled.into()),
                result = future() => { self.check()?; Ok(result?) }
            }
        })
    }
}

static ACTIVE: Mutex<Option<CancellationToken>> = Mutex::new(None);

pub(crate) struct SignalGuard(CancellationToken);
impl Drop for SignalGuard {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().expect("signal cancellation poisoned");
        if active
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(&token.0, &self.0.0))
        {
            *active = None;
        }
    }
}

/// Used by CLI/TUI frontends, never implicitly installed by the library API.
pub(crate) fn signals(token: &CancellationToken) -> Result<SignalGuard> {
    static INSTALLED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    INSTALLED
        .get_or_init(|| {
            ctrlc::set_handler(|| {
                if let Some(token) = ACTIVE
                    .lock()
                    .expect("signal cancellation poisoned")
                    .as_ref()
                {
                    token.cancel();
                }
            })
            .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| anyhow!("could not install cancellation signal handler: {error}"))?;
    *ACTIVE.lock().expect("signal cancellation poisoned") = Some(token.clone());
    Ok(SignalGuard(token.clone()))
}
