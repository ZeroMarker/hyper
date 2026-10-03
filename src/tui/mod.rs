mod ui;

use crate::{
    AgentMode, ApprovalGate, ApprovalRequest, CancellationToken, EventSink, ExecutionMode,
    RunOptions, cancellation, deepseek::DEFAULT_MODEL, i18n, latest_display_output, list_runs,
    prompt_to_task, run_task_with_control, workspace,
};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    execute,
};
use ratatui::text::Line;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

pub struct App {
    pub root: PathBuf,
    pub input: String,
    pub mode: AgentMode,
    pub execution_mode: ExecutionMode,
    pub permissions: crate::ToolPermissions,
    pub model: String,
    pub output: Vec<String>,
    pub rendered: Vec<Line<'static>>,
    pub rendered_count: usize,
    pub event_tail: VecDeque<String>,
    pub live_text: String,
    pub busy: bool,
    pub quit: bool,
    pub scroll: u16,
    pub follow_tail: bool,
    pub tick: usize,
    pub command_index: usize,
    pub approvals: VecDeque<ApprovalRequest>,
    /// The conversation this TUI is continuing. `None` until the first message,
    /// which opens one; `/new` drops it so the next message starts fresh.
    pub session: Option<String>,
    /// The id to show in the header, kept after `/new` clears `session` so the
    /// user can still see which conversation they just left.
    pub session_id: Option<String>,
    gate: Option<ApprovalGate>,
    cancellation: Option<CancellationToken>,
    process_cancellation: CancellationToken,
    worker: Option<std::thread::JoinHandle<()>>,
    quit_requested: bool,
    sink: Option<EventSink>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}
enum Message {
    Task(Result<String, String>),
}

impl App {
    fn new(root: PathBuf, session: Option<String>, execution_mode: ExecutionMode) -> Self {
        let (tx, rx) = mpsc::channel();
        let model = std::env::var("DEEPSEEK_MODEL")
            .ok()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.into());
        Self {
            root,
            input: String::new(),
            mode: AgentMode::Build,
            execution_mode,
            permissions: crate::ToolPermissions::default(),
            model,
            output: vec![
                i18n::text(
                    "Hyper\nHi, how can I help?",
                    "Hyper\n你好，需要我帮你做什么？",
                )
                .into(),
            ],
            rendered: Vec::new(),
            rendered_count: 0,
            event_tail: VecDeque::new(),
            live_text: String::new(),
            busy: false,
            quit: false,
            scroll: 0,
            follow_tail: true,
            tick: 0,
            command_index: 0,
            approvals: VecDeque::new(),
            session_id: session.clone(),
            session,
            gate: None,
            cancellation: None,
            process_cancellation: CancellationToken::new(),
            worker: None,
            quit_requested: false,
            sink: None,
            tx,
            rx,
        }
    }
    fn submit(&mut self) {
        let value = self.input.trim().to_owned();
        if value == "/cancel" {
            self.input.clear();
            self.cancel_active();
            return;
        }
        if matches!(value.as_str(), "/quit" | "/exit") {
            self.input.clear();
            self.request_quit();
            return;
        }
        if value.is_empty() || self.busy {
            return;
        }
        self.input.clear();
        match value.as_str() {
            "/quit" | "/exit" => self.request_quit(),
            "/new" => {
                // Clearing the transcript without dropping the conversation
                // would leave the next message answering turns the user can no
                // longer see, so `/new` starts a new conversation as well.
                self.output.clear();
                self.rendered.clear();
                self.rendered_count = 0;
                self.event_tail.clear();
                self.live_text.clear();
                if let Some(previous) = self.session.take() {
                    self.output.push(format!(
                        "Hyper\n{} `{previous}`. {} `hyper session {previous}`.",
                        i18n::text("Started a new conversation. Previous:", "已开始新对话。上一段："),
                        i18n::text("View it with", "可使用以下命令查看：")
                    ));
                }
            }
            "/session" => {
                let text = match (&self.session, &self.session_id) {
                    (Some(id), _) => format!(
                        "Hyper\n{} `{id}`. {} `hyper session {id}`; {} `--session {id}`.",
                        i18n::text("Current conversation:", "当前对话："),
                        i18n::text("View with", "查看："),
                        i18n::text("continue with", "继续：")
                    ),
                    (None, Some(previous)) => {
                        format!("Hyper\n{} `{previous}`", i18n::text("No new conversation yet. Previous:", "尚未开始新对话。上一段："))
                    }
                    (None, None) => i18n::text("Hyper\nNo conversation yet. Send a message to start one.", "Hyper\n还没开始对话：发送一条消息即会创建。").into(),
                };
                self.output.push(text);
            }
            "/runs" => match list_runs(&self.root, 8) {
                Ok(runs) if runs.is_empty() => self
                    .output
                    .push(i18n::text("Hyper\nNo runs yet. Run a task to see it here.", "Hyper\n暂无运行记录。运行一个任务后会显示在这里。").into()),
                Ok(runs) => {
                    let lines = runs
                        .iter()
                        .map(|run| {
                            format!(
                                "{}  {}  {}  {}",
                                &run.run_id[..run.run_id.len().min(8)],
                                run.status,
                                run.task_name,
                                run.started_at
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.output.push(format!("Hyper\n{}\n{lines}", i18n::text("Recent runs (`hyper show <run-id>` for details):", "最近运行（`hyper show <run-id>` 查看详情）：")));
                }
                Err(error) => self
                    .output
                    .push(format!("Hyper\n{} {error}", i18n::text("Could not read runs:", "无法读取运行记录："))),
            },
            "/config" => self.output.push(
                i18n::text("Hyper\nExit and run `hyper config` to update the API key, base URL, or model.", "Hyper\n退出后运行 `hyper config` 可重新配置 API Key / base URL / model。").into(),
            ),
            "/help" => self
                .output
                .push(i18n::text("Hyper\nType `/` for commands. Use ↑↓ to select, Tab to complete, and Enter to run.", "Hyper\n输入 `/` 打开命令提示，使用 ↑↓ 选择、Tab 补全、Enter 执行。").into()),
            "/mode plan" => {
                self.mode = AgentMode::Plan;
                self.output.push(i18n::text("Hyper\nSwitched to **plan** mode.", "Hyper\n已切换到 **plan** 模式。").into());
            }
            "/mode build" => {
                self.mode = AgentMode::Build;
                self.output.push(i18n::text("Hyper\nSwitched to **build** mode.", "Hyper\n已切换到 **build** 模式。").into());
            }
            _ if value.starts_with('/') => self.output.push(format!("Hyper\n{} `{value}`. {}", i18n::text("Unknown command:", "未知命令："), i18n::text("Type `/` to see commands.", "输入 `/` 查看命令提示。"))),
            _ => {
                self.busy = true;
                self.follow_tail = true;
                self.scroll = 0;
                self.output.push(format!("You\n{value}"));
                self.approvals.clear();
                self.event_tail.clear();
                self.live_text.clear();
                let gate = ApprovalGate::new();
                self.gate = Some(gate.clone());
                let sink = EventSink::new();
                self.sink = Some(sink.clone());
                let root = self.root.clone();
                let mode = self.mode;
                let execution_mode = self.execution_mode;
                let permissions = self.permissions.clone();
                let tx = self.tx.clone();
                // The first message opens a conversation, and every later one
                // continues it, which is what gives the model the earlier turns
                // as context.
                let session = self.session.get_or_insert_with(workspace::id).clone();
                self.session_id = Some(session.clone());
                let cancellation = self.process_cancellation.child();
                self.cancellation = Some(cancellation.clone());
                self.worker = Some(std::thread::spawn(move || {
                    let result = run_task_with_control(
                        &prompt_to_task(&value, mode), &root, RunOptions {
                            cancellation, execution_mode, session_id: Some(session),
                            gate: Some(gate), sink: Some(sink), permissions,
                        }
                    )
                    .and_then(|summary| {
                        Ok(latest_display_output(&root, &summary.run_id)?
                            .unwrap_or_else(|| format!("{} {}.", i18n::text("Task", "任务已"), summary.status)))
                    })
                    .map_err(|e| e.to_string());
                    let _ = tx.send(Message::Task(result));
                }));
            }
        }
    }

    fn approve_first(&mut self) {
        if let Some(request) = self.approvals.pop_front() {
            let _ = request.response.send(true);
        }
    }

    fn deny_first(&mut self) {
        if let Some(request) = self.approvals.pop_front() {
            let _ = request.response.send(false);
        }
    }

    fn deny_all(&mut self) {
        while let Some(request) = self.approvals.pop_front() {
            let _ = request.response.send(false);
        }
    }

    pub fn command_suggestions(&self) -> Vec<(&'static str, &'static str)> {
        if !self.input.starts_with('/') || self.input.contains(' ') {
            return Vec::new();
        }
        let commands = [
            ("/help", i18n::text("Show help", "显示帮助")),
            (
                "/new",
                i18n::text(
                    "Start a new conversation",
                    "开始新对话（当前对话保留在 sessions/ 中）",
                ),
            ),
            (
                "/session",
                i18n::text("Show conversation ID", "显示当前对话 id"),
            ),
            (
                "/mode plan",
                i18n::text("Switch to read-only plan mode", "切换到只读规划模式"),
            ),
            (
                "/mode build",
                i18n::text("Switch to build mode", "切换到构建模式"),
            ),
            (
                "/runs",
                i18n::text("Show recent runs", "提示如何查看运行历史"),
            ),
            (
                "/config",
                i18n::text("Show API key setup", "提示如何重新配置 API Key"),
            ),
            ("/cancel", i18n::text("Cancel current run", "取消当前运行")),
            ("/quit", i18n::text("Exit Hyper", "退出 Hyper")),
        ];
        commands
            .into_iter()
            .filter(|(command, _)| command.starts_with(&self.input))
            .collect()
    }

    fn move_command_up(&mut self) {
        let count = self.command_suggestions().len();
        if count > 0 {
            self.command_index = self.command_index.checked_sub(1).unwrap_or(count - 1);
        }
    }

    fn move_command_down(&mut self) {
        let count = self.command_suggestions().len();
        if count > 0 {
            self.command_index = (self.command_index + 1) % count;
        }
    }

    fn complete_command(&mut self) -> bool {
        let suggestions = self.command_suggestions();
        let Some((command, _)) =
            suggestions.get(self.command_index.min(suggestions.len().saturating_sub(1)))
        else {
            return false;
        };
        self.input = (*command).into();
        self.command_index = 0;
        true
    }
    fn cancel_active(&mut self) {
        if let Some(token) = &self.cancellation {
            token.cancel();
        }
        self.deny_all();
    }

    fn request_quit(&mut self) {
        self.quit_requested = true;
        self.cancel_active();
        if !self.busy {
            self.quit = true;
        }
    }

    fn poll(&mut self) {
        if self.process_cancellation.is_cancelled() {
            self.request_quit();
        }
        // Surface new approval requests from the running task thread.
        if let Some(gate) = &self.gate {
            for request in gate.drain() {
                self.approvals.push_back(request);
            }
        }
        if let Some(sink) = &self.sink {
            self.live_text.push_str(&sink.take_text());
            const MAX_LIVE_TEXT: usize = 128 * 1024;
            if self.live_text.len() > MAX_LIVE_TEXT {
                let mut drop = self.live_text.len() - MAX_LIVE_TEXT;
                while !self.live_text.is_char_boundary(drop) {
                    drop += 1;
                }
                self.live_text.drain(..drop);
            }
            for line in sink.drain() {
                // Repeated model iterations can arrive faster than a redraw.
                if self.event_tail.back() != Some(&line) {
                    if self.event_tail.len() == 12 {
                        self.event_tail.pop_front();
                    }
                    self.event_tail.push_back(line);
                }
            }
        }
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Message::Task(r) => {
                    self.busy = false;
                    self.cancellation = None;
                    self.gate = None;
                    if let Some(worker) = self.worker.take() {
                        let _ = worker.join();
                    }
                    if self.quit_requested {
                        self.quit = true;
                    }
                    self.approvals.clear();
                    self.sink = None;
                    self.live_text.clear();
                    self.follow_tail = true;
                    self.scroll = 0;
                    self.output.push(format!(
                        "Hyper\n{}",
                        r.unwrap_or_else(|e| format!(
                            "{} {e}",
                            i18n::text("Failed:", "执行失败：")
                        ))
                    ));
                }
            }
        }
    }

    fn scroll_up(&mut self, amount: u16) {
        self.follow_tail = false;
        self.scroll = self.scroll.saturating_add(amount);
    }

    fn scroll_down(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_sub(amount);
        self.follow_tail = self.scroll == 0;
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.cancel_active();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn run(root: PathBuf, session: Option<String>) -> Result<()> {
    run_with_mode(root, session, ExecutionMode::from_env()?)
}

pub fn run_with_mode(
    root: PathBuf,
    session: Option<String>,
    execution_mode: ExecutionMode,
) -> Result<()> {
    run_with_permissions(
        root,
        session,
        execution_mode,
        crate::ToolPermissions::from_env()?,
    )
}

pub fn run_with_permissions(
    root: PathBuf,
    session: Option<String>,
    execution_mode: ExecutionMode,
    permissions: crate::ToolPermissions,
) -> Result<()> {
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = (|| {
        let mut app = App::new(root, session, execution_mode);
        app.permissions = permissions;
        let _signals = cancellation::signals(&app.process_cancellation)?;
        while !app.quit {
            app.tick = app.tick.wrapping_add(1);
            app.poll();
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            let input_event = event::read()?;
            if let Event::Mouse(mouse) = input_event {
                match mouse.kind {
                    MouseEventKind::ScrollUp => app.scroll_up(3),
                    MouseEventKind::ScrollDown => app.scroll_down(3),
                    _ => {}
                }
                continue;
            }
            let Event::Key(k) = input_event else { continue };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            // Ctrl-C cancels even while an approval modal is active.
            if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
                if app.busy {
                    app.cancel_active();
                } else {
                    app.request_quit();
                }
                continue;
            }
            // Esc in the modal denies just that action; Ctrl-C cancels the run.
            // While an approval is pending, all other keys answer the modal.
            if !app.approvals.is_empty() {
                match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.approve_first(),
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.deny_first(),
                    _ => {}
                }
                continue;
            }
            match (k.code, k.modifiers) {
                (KeyCode::Esc, _) => {
                    if app.busy {
                        app.cancel_active();
                    } else {
                        app.request_quit();
                    }
                }
                (KeyCode::Tab, _) => {
                    if !app.complete_command() {
                        app.mode = if app.mode == AgentMode::Build {
                            AgentMode::Plan
                        } else {
                            AgentMode::Build
                        }
                    }
                }
                (KeyCode::Up, _) => {
                    if app.command_suggestions().is_empty() {
                        app.scroll_up(1)
                    } else {
                        app.move_command_up()
                    }
                }
                (KeyCode::Down, _) => {
                    if app.command_suggestions().is_empty() {
                        app.scroll_down(1)
                    } else {
                        app.move_command_down()
                    }
                }
                (KeyCode::PageUp, _) => app.scroll_up(8),
                (KeyCode::PageDown, _) => app.scroll_down(8),
                (KeyCode::Home, _) => {
                    app.follow_tail = false;
                    app.scroll = u16::MAX;
                }
                (KeyCode::End, _) => {
                    app.follow_tail = true;
                    app.scroll = 0;
                }
                (KeyCode::Enter, _) => {
                    app.complete_command();
                    app.submit();
                }
                (KeyCode::Backspace, _) => {
                    app.input.pop();
                    app.command_index = 0;
                }
                (KeyCode::Char(c), _) => {
                    app.input.push(c);
                    app.command_index = 0;
                }
                _ => {}
            }
        }
        // Unblock any tool waiting on an approval when we are quitting.
        app.deny_all();
        Ok(())
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_command_unblocks_modal_and_conversation_can_continue() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path().to_owned(), None, ExecutionMode::default());
        app.input = "write:out.txt\nwrong".into();
        app.submit();
        let started = std::time::Instant::now();
        while app.approvals.is_empty() {
            app.poll();
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
        app.input = "/cancel".into();
        app.submit();
        while app.busy {
            app.poll();
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!app.quit);
        assert!(!dir.path().join("out.txt").exists());
        assert!(app.approvals.is_empty());
        assert!(app.worker.is_none());
        assert!(
            app.output.last().unwrap().contains("cancelled")
                || app.output.last().unwrap().contains("取消")
        );
        let session = app.session.clone().unwrap();
        app.input = "write:out.txt\nok".into();
        app.submit();
        while app.busy {
            app.poll();
            app.approve_first();
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(app.session.as_deref(), Some(session.as_str()));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
            "ok"
        );
        assert_eq!(
            workspace::Workspace::open(dir.path())
                .unwrap()
                .session_messages(&session)
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn quit_waits_for_active_run_to_settle() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path().to_owned(), None, ExecutionMode::default());
        app.input = "write:out.txt\nwrong".into();
        app.submit();
        app.request_quit();
        assert!(!app.quit);
        let started = std::time::Instant::now();
        while !app.quit {
            app.poll();
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!app.busy);
        assert!(app.worker.is_none());
        assert!(!dir.path().join("out.txt").exists());
        assert_eq!(
            workspace::Workspace::open(dir.path())
                .unwrap()
                .list_runs(1)
                .unwrap()[0]
                .status,
            "cancelled"
        );
    }

    #[test]
    fn process_cancellation_stops_worker_without_ui_poll() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path().to_owned(), None, ExecutionMode::default());
        app.input = "write:out.txt\nwrong".into();
        app.submit();
        let started = std::time::Instant::now();
        let mut pending = Vec::new();
        while pending.is_empty() {
            pending.extend(app.gate.as_ref().unwrap().drain());
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
        app.process_cancellation.cancel();
        while !app.worker.as_ref().unwrap().is_finished() {
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!dir.path().join("out.txt").exists());
        assert_eq!(
            workspace::Workspace::open(dir.path())
                .unwrap()
                .list_runs(1)
                .unwrap()[0]
                .status,
            "cancelled"
        );
        app.poll();
        assert!(app.quit);
        assert!(!app.busy);
    }

    #[test]
    fn slash_input_filters_and_completes_commands() {
        let mut app = App::new(PathBuf::from("."), None, ExecutionMode::default());
        app.input = "/mo".into();
        assert_eq!(app.command_suggestions().len(), 2);
        assert!(app.complete_command());
        assert_eq!(app.input, "/mode plan");
    }

    /// `/new` must start a genuinely new conversation. Clearing only the
    /// transcript would leave the next message answering turns the user can no
    /// longer see, which is worse than having no `/new` at all.
    #[test]
    fn new_starts_a_fresh_conversation() {
        let mut app = App::new(
            PathBuf::from("."),
            Some("previous-chat".into()),
            ExecutionMode::default(),
        );
        app.session = Some("previous-chat".into());
        app.output.push("You\nsomething private".into());
        app.input = "/new".into();
        app.submit();

        assert_eq!(
            app.session, None,
            "the next message must open a new session"
        );
        assert!(
            app.output
                .iter()
                .all(|line| !line.contains("something private")),
            "the transcript is cleared: {:?}",
            app.output
        );
        assert!(
            app.output.iter().any(|line| line.contains("previous-chat")
                && line.contains(i18n::text("Started a new conversation", "已开始新对话"))),
            "the user must be told which conversation was left: {:?}",
            app.output
        );

        app.input = "/session".into();
        app.submit();
        let reported = app.output.last().unwrap();
        assert!(
            reported.contains(i18n::text("No new conversation yet", "尚未开始新对话"))
                && reported.contains("previous-chat"),
            "after /new the app reports the conversation it left: {reported:?}"
        );
    }

    /// Continuing a conversation must not create a second one, or the context
    /// would silently split in two.
    #[test]
    fn a_resumed_app_keeps_the_same_session() {
        let mut app = App::new(
            PathBuf::from("."),
            Some("chat-1".into()),
            ExecutionMode::default(),
        );
        assert_eq!(app.session.as_deref(), Some("chat-1"));
        app.input = "/session".into();
        app.submit();
        assert!(
            app.output.last().unwrap().contains("chat-1"),
            "{:?}",
            app.output.last()
        );
    }
}
