mod ui;

use crate::{
    AgentMode, ApprovalGate, ApprovalRequest, deepseek::DEFAULT_MODEL, latest_display_output,
    list_runs, prompt_to_task, run_task_in_session_with_approval, workspace,
};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    execute,
};
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
    pub model: String,
    pub output: Vec<String>,
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
    tx: Sender<Message>,
    rx: Receiver<Message>,
}
enum Message {
    Task(Result<String, String>),
}
impl App {
    fn new(root: PathBuf, session: Option<String>) -> Self {
        let (tx, rx) = mpsc::channel();
        let model = std::env::var("DEEPSEEK_MODEL")
            .ok()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.into());
        Self {
            root,
            input: String::new(),
            mode: AgentMode::Build,
            model,
            output: vec!["Hyper\n你好，需要我帮你做什么？".into()],
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
            tx,
            rx,
        }
    }
    fn submit(&mut self) {
        let value = self.input.trim().to_owned();
        if value.is_empty() || self.busy {
            return;
        }
        self.input.clear();
        match value.as_str() {
            "/quit" | "/exit" => self.quit = true,
            "/new" => {
                // Clearing the transcript without dropping the conversation
                // would leave the next message answering turns the user can no
                // longer see, so `/new` starts a new conversation as well.
                self.output.clear();
                if let Some(previous) = self.session.take() {
                    self.output.push(format!(
                        "Hyper\n已开始新对话（上一段：`{previous}`，用 `hyper session {previous}` 查看）。"
                    ));
                }
            }
            "/session" => {
                let text = match (&self.session, &self.session_id) {
                    (Some(id), _) => format!(
                        "Hyper\n当前对话：`{id}`（`hyper session {id}` 查看，`--session {id}` 继续）"
                    ),
                    (None, Some(previous)) => {
                        format!("Hyper\n当前尚未开始新对话。上一段：`{previous}`")
                    }
                    (None, None) => "Hyper\n还没开始对话：发送一条消息即会创建。".into(),
                };
                self.output.push(text);
            }
            "/runs" => match list_runs(&self.root, 8) {
                Ok(runs) if runs.is_empty() => self
                    .output
                    .push("Hyper\n暂无运行记录。运行一个任务后会显示在这里。".into()),
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
                    self.output.push(format!(
                        "Hyper\n最近运行（`hyper show <run-id>` 查看详情）：\n{lines}"
                    ));
                }
                Err(error) => self
                    .output
                    .push(format!("Hyper\n无法读取运行记录：{error}")),
            },
            "/config" => self.output.push(
                "Hyper\n退出后运行 `hyper config` 可重新配置 API Key / base URL / model。".into(),
            ),
            "/help" => self
                .output
                .push("Hyper\n输入 `/` 打开命令提示，使用 ↑↓ 选择、Tab 补全、Enter 执行。".into()),
            "/mode plan" => {
                self.mode = AgentMode::Plan;
                self.output.push("Hyper\n已切换到 **plan** 模式。".into());
            }
            "/mode build" => {
                self.mode = AgentMode::Build;
                self.output.push("Hyper\n已切换到 **build** 模式。".into());
            }
            _ if value.starts_with('/') => self.output.push(format!(
                "Hyper\n未知命令：`{value}`。输入 `/` 查看命令提示。"
            )),
            _ => {
                self.busy = true;
                self.follow_tail = true;
                self.scroll = 0;
                self.output.push(format!("You\n{value}"));
                self.approvals.clear();
                let gate = ApprovalGate::new();
                self.gate = Some(gate.clone());
                let root = self.root.clone();
                let mode = self.mode;
                let tx = self.tx.clone();
                // The first message opens a conversation, and every later one
                // continues it, which is what gives the model the earlier turns
                // as context.
                let session = self.session.get_or_insert_with(workspace::id).clone();
                self.session_id = Some(session.clone());
                std::thread::spawn(move || {
                    let result = run_task_in_session_with_approval(
                        &prompt_to_task(&value, mode),
                        &root,
                        &session,
                        gate,
                    )
                    .and_then(|summary| {
                        Ok(latest_display_output(&root, &summary.run_id)?
                            .unwrap_or_else(|| format!("任务已{}。", summary.status)))
                    })
                    .map_err(|e| e.to_string());
                    let _ = tx.send(Message::Task(result));
                });
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
        const COMMANDS: [(&str, &str); 8] = [
            ("/help", "显示帮助"),
            ("/new", "开始新对话（当前对话保留在 sessions/ 中）"),
            ("/session", "显示当前对话 id"),
            ("/mode plan", "切换到只读规划模式"),
            ("/mode build", "切换到构建模式"),
            ("/runs", "提示如何查看运行历史"),
            ("/config", "提示如何重新配置 API Key"),
            ("/quit", "退出 Hyper"),
        ];
        COMMANDS
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
    fn poll(&mut self) {
        // Surface new approval requests from the running task thread.
        if let Some(gate) = &self.gate {
            for request in gate.drain() {
                self.approvals.push_back(request);
            }
        }
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Message::Task(r) => {
                    self.busy = false;
                    self.approvals.clear();
                    self.follow_tail = true;
                    self.scroll = 0;
                    self.output.push(format!(
                        "Hyper\n{}",
                        r.unwrap_or_else(|e| format!("执行失败：{e}"))
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
pub fn run(root: PathBuf, session: Option<String>) -> Result<()> {
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = (|| {
        let mut app = App::new(root, session);
        while !app.quit {
            app.tick = app.tick.wrapping_add(1);
            app.poll();
            terminal.draw(|f| ui::draw(f, &app))?;
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
            // While an approval is pending, all keys answer the modal.
            if !app.approvals.is_empty() {
                match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.approve_first(),
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.deny_first(),
                    _ => {}
                }
                continue;
            }
            match (k.code, k.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    app.deny_all();
                    app.quit = true;
                }
                (KeyCode::Esc, _) => app.quit = true,
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
    fn slash_input_filters_and_completes_commands() {
        let mut app = App::new(PathBuf::from("."), None);
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
        let mut app = App::new(PathBuf::from("."), Some("previous-chat".into()));
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
            app.output
                .iter()
                .any(|line| line.contains("previous-chat") && line.contains("已开始新对话")),
            "the user must be told which conversation was left: {:?}",
            app.output
        );

        app.input = "/session".into();
        app.submit();
        let reported = app.output.last().unwrap();
        assert!(
            reported.contains("尚未开始新对话") && reported.contains("previous-chat"),
            "after /new the app reports the conversation it left: {reported:?}"
        );
    }

    /// Continuing a conversation must not create a second one, or the context
    /// would silently split in two.
    #[test]
    fn a_resumed_app_keeps_the_same_session() {
        let mut app = App::new(PathBuf::from("."), Some("chat-1".into()));
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
