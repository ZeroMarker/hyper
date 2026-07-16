mod ui;

use crate::{AgentMode, latest_display_output, prompt_to_task, run_task};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    execute,
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

pub struct App {
    pub root: PathBuf,
    pub input: String,
    pub mode: AgentMode,
    pub output: Vec<String>,
    pub busy: bool,
    pub quit: bool,
    pub scroll: u16,
    pub follow_tail: bool,
    pub tick: usize,
    pub command_index: usize,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}
enum Message {
    Task(Result<String, String>),
}
impl App {
    fn new(root: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            root,
            input: String::new(),
            mode: AgentMode::Build,
            output: vec!["Hyper\n你好，需要我帮你做什么？".into()],
            busy: false,
            quit: false,
            scroll: 0,
            follow_tail: true,
            tick: 0,
            command_index: 0,
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
            "/new" => self.output.clear(),
            "/runs" => self
                .output
                .push("Hyper\n请使用 `hy ls` 查看运行历史。".into()),
            "/config" => self
                .output
                .push("Hyper\n退出后运行 `hy config` 可重新配置 API Key。".into()),
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
                let root = self.root.clone();
                let mode = self.mode;
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let result = run_task(&prompt_to_task(&value, mode), &root)
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

    pub fn command_suggestions(&self) -> Vec<(&'static str, &'static str)> {
        if !self.input.starts_with('/') || self.input.contains(' ') {
            return Vec::new();
        }
        const COMMANDS: [(&str, &str); 7] = [
            ("/help", "显示帮助"),
            ("/new", "清空当前对话"),
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
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Message::Task(r) => {
                    self.busy = false;
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
pub fn run(root: PathBuf) -> Result<()> {
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = (|| {
        let mut app = App::new(root);
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
            match (k.code, k.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => app.quit = true,
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
        let mut app = App::new(PathBuf::from("."));
        app.input = "/mo".into();
        assert_eq!(app.command_suggestions().len(), 2);
        assert!(app.complete_command());
        assert_eq!(app.input, "/mode plan");
    }
}
