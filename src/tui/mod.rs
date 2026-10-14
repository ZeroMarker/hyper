mod input;
mod overlay;
mod ui;

use overlay::{ADD_PROVIDER, Action, Overlay, Picker, PickerItem, PickerKind, ProviderForm};

use crate::{
    AgentMode, ApprovalGate, ApprovalRequest, CancellationToken, EventSink, ExecutionMode,
    RunOptions, cancellation, deepseek::profiles, i18n, latest_display_output, list_runs,
    prompt_to_task, run_task_with_control, workspace,
};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
};
use ratatui::text::Line;
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

pub struct App {
    pub root: PathBuf,
    pub input: input::Input,
    pub history: input::History,
    pub mode: AgentMode,
    pub execution_mode: ExecutionMode,
    pub permissions: crate::ToolPermissions,
    /// The permissions the TUI started with; `/permissions` applies on top.
    pub launch_permissions: crate::ToolPermissions,
    pub model: String,
    /// The active provider profile, shown next to the model.
    pub profile: Option<String>,
    /// A picker or form drawn over the chat; it takes all keys while open.
    pub overlay: Option<Overlay>,
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
    /// Approvals answered "always" for the rest of this TUI session: a tool
    /// name for write/edit, or `bash:<exact command>`.
    pub session_allows: HashSet<String>,
    /// When an idle Ctrl-C on an empty input asked for a second press to quit.
    pub quit_armed: Option<Instant>,
    /// The latest run started from this TUI, for `/diff` and `/undo`.
    pub last_run: Option<String>,
    /// Token use in this conversation: the last request's input is how full the
    /// context is; input/output are running totals.
    pub usage: crate::RunUsage,
    /// Workspace files for `@` mentions, refreshed when stale.
    files: Option<(Instant, Vec<String>)>,
    pub mention_index: usize,
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
/// What a finished task reports back to the UI thread.
struct TaskDone {
    run_id: String,
    text: String,
    usage: crate::RunUsage,
}

enum Message {
    Task(Result<TaskDone, String>),
    Models(Result<Vec<String>, String>),
    Compacted(Result<crate::Compaction, String>),
}

impl App {
    fn new(root: PathBuf, session: Option<String>, execution_mode: ExecutionMode) -> Self {
        let (tx, rx) = mpsc::channel();
        let model = profiles::current_model();
        // Per workspace, beside its runs and sessions, never in the checkout.
        let history = input::History::load(
            crate::state::directory(&root)
                .ok()
                .map(|dir| dir.join("input_history.jsonl")),
        );
        Self {
            root,
            input: input::Input::default(),
            history,
            mode: AgentMode::Build,
            execution_mode,
            permissions: crate::ToolPermissions::default(),
            launch_permissions: crate::ToolPermissions::default(),
            model,
            profile: profiles::current_profile(),
            overlay: None,
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
            session_allows: HashSet::new(),
            quit_armed: None,
            last_run: None,
            usage: crate::RunUsage::default(),
            files: None,
            mention_index: 0,
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
        let value = self.input.text.trim().to_owned();
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
        self.history.push(&value);
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
                self.usage = crate::RunUsage::default();
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
            "/config" | "/connect" => {
                self.overlay = Some(Overlay::Picker(Picker::new(
                    PickerKind::Preset,
                    i18n::text(" Add provider ", " 添加服务商 "),
                    overlay::presets(),
                )));
            }
            "/profile" => self.open_profile_picker(),
            "/resume" => self.open_session_picker(),
            "/diff" => self.show_last_diff(),
            "/permissions" => self.open_permissions_picker(),
            "/compact" => self.start_compaction(),
            "/undo" => self.confirm_undo(),
            "/model" => self.open_model_picker(),
            _ if value.starts_with("/model ") => {
                let model = value["/model ".len()..].trim();
                match profiles::set_model(model) {
                    Ok(profile) => {
                        self.model = profiles::current_model();
                        let mut message = format!("Hyper\n{} **{profile}** → {}.", i18n::text("Switched model for profile", "已切换 profile 的模型："), model);
                        if std::env::var("DEEPSEEK_MODEL").is_ok_and(|m| !m.trim().is_empty()) {
                            message.push_str(i18n::text("\nNote: `DEEPSEEK_MODEL` is set and overrides the profile model.", "\n注意：已设置 `DEEPSEEK_MODEL`，它会覆盖 profile 的模型。"));
                        }
                        self.output.push(message);
                    }
                    Err(error) => self
                        .output
                        .push(format!("Hyper\n{} {error:#}", i18n::text("Could not switch model:", "无法切换模型："))),
                }
            }
            _ if value.starts_with("/profile ") => {
                let name = value["/profile ".len()..].trim();
                match profiles::use_profile(name) {
                    Ok(()) => {
                        // Each run resolves the provider afresh, so the next
                        // message already goes to the selected profile.
                        self.model = profiles::current_model();
                        let mut message = format!("Hyper\n{} **{name}** ({}).", i18n::text("Switched to profile", "已切换到 profile"), self.model);
                        if std::env::var("DEEPSEEK_MODEL").is_ok_and(|m| !m.trim().is_empty()) {
                            message.push_str(i18n::text("\nNote: `DEEPSEEK_MODEL` is set and overrides the profile model.", "\n注意：已设置 `DEEPSEEK_MODEL`，它会覆盖 profile 的模型。"));
                        }
                        self.output.push(message);
                    }
                    Err(error) => self
                        .output
                        .push(format!("Hyper\n{} {error:#}", i18n::text("Could not switch profile:", "无法切换 profile："))),
                }
            }
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
                        let text = latest_display_output(&root, &summary.run_id)?
                            .unwrap_or_else(|| format!("{} {}.", i18n::text("Task", "任务已"), summary.status));
                        // Usage is a display nicety; a read failure shows none.
                        let usage = crate::run_usage(&root, &summary.run_id).unwrap_or_default();
                        Ok(TaskDone { run_id: summary.run_id, text, usage })
                    })
                    .map_err(|e| e.to_string());
                    let _ = tx.send(Message::Task(result));
                }));
            }
        }
    }

    fn refresh_provider(&mut self) {
        self.model = profiles::current_model();
        self.profile = profiles::current_profile();
    }

    fn open_model_picker(&mut self) {
        self.overlay = Some(Overlay::Picker(Picker::loading(
            PickerKind::Model,
            i18n::text(" Select model ", " 选择模型 "),
        )));
        let tx = self.tx.clone();
        // A network call; kept off the UI thread so the screen stays live.
        std::thread::spawn(move || {
            let result = profiles::list_models(None).map_err(|error| format!("{error:#}"));
            let _ = tx.send(Message::Models(result));
        });
    }

    fn open_profile_picker(&mut self) {
        let mut items = Vec::new();
        let mut error = None;
        match profiles::list() {
            Ok(listing) => {
                let active = listing["activeProfile"].as_str();
                for profile in listing["profiles"].as_array().into_iter().flatten() {
                    let name = profile["name"].as_str().unwrap_or_default();
                    items.push(PickerItem {
                        label: name.into(),
                        detail: format!(
                            "{}  {}",
                            profile["model"].as_str().unwrap_or_default(),
                            profile["baseUrl"].as_str().unwrap_or_default()
                        ),
                        value: name.into(),
                        current: Some(name) == active,
                    });
                }
            }
            Err(e) => error = Some(format!("{e:#}")),
        }
        items.push(PickerItem {
            label: i18n::text("+ Add provider…", "＋ 添加服务商…").into(),
            detail: String::new(),
            value: ADD_PROVIDER.into(),
            current: false,
        });
        let mut picker = Picker::new(
            PickerKind::Profile,
            i18n::text(" Select provider ", " 选择服务商 "),
            items,
        );
        picker.error = error;
        self.overlay = Some(Overlay::Picker(picker));
    }

    fn overlay_key(&mut self, code: KeyCode) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        match overlay.key(code) {
            None => {}
            Some(Action::Close) => self.overlay = None,
            Some(Action::Choose(PickerKind::Model, model)) => {
                self.overlay = None;
                match profiles::set_model(&model) {
                    Ok(profile) => {
                        self.refresh_provider();
                        self.output.push(format!(
                            "Hyper\n{} **{profile}** → {model}",
                            i18n::text("Model switched:", "已切换模型：")
                        ));
                        self.warn_env_model_override();
                    }
                    Err(error) => self.output.push(format!(
                        "Hyper\n{} {error:#}",
                        i18n::text("Could not switch model:", "无法切换模型：")
                    )),
                }
            }
            Some(Action::Choose(PickerKind::Profile, name)) if name == ADD_PROVIDER => {
                self.overlay = Some(Overlay::Picker(Picker::new(
                    PickerKind::Preset,
                    i18n::text(" Add provider ", " 添加服务商 "),
                    overlay::presets(),
                )));
            }
            Some(Action::Choose(PickerKind::Profile, name)) => {
                self.overlay = None;
                match profiles::use_profile(&name) {
                    Ok(()) => {
                        self.refresh_provider();
                        self.output.push(format!(
                            "Hyper\n{} **{name}** ({})",
                            i18n::text("Provider switched:", "已切换服务商："),
                            self.model
                        ));
                        self.warn_env_model_override();
                    }
                    Err(error) => self.output.push(format!(
                        "Hyper\n{} {error:#}",
                        i18n::text("Could not switch provider:", "无法切换服务商：")
                    )),
                }
            }
            Some(Action::Choose(PickerKind::Session, id)) => {
                self.overlay = None;
                self.resume_session(&id);
            }
            Some(Action::Choose(PickerKind::ConfirmUndo, answer)) => {
                self.overlay = None;
                if answer == "yes" {
                    self.undo_last_run();
                }
            }
            Some(Action::Choose(PickerKind::Permissions, choice)) => {
                self.overlay = None;
                let text = match choice.as_str() {
                    "clear" => {
                        self.session_allows.clear();
                        i18n::text(
                            "Cleared this session's \"always allow\" answers.",
                            "已清除本会话的「总是允许」。",
                        )
                        .to_owned()
                    }
                    choice => {
                        let permission = match choice {
                            "allow" => crate::Permission::Allow,
                            "deny" => crate::Permission::Deny,
                            _ => crate::Permission::Ask,
                        };
                        // Rebuilt from the launch settings so a looser choice
                        // can be undone by a stricter one later.
                        let mut permissions = self.launch_permissions.clone();
                        permissions.apply_approval(permission);
                        permissions.source.push_str(" + /permissions");
                        self.permissions = permissions;
                        format!(
                            "{} write {:?} / edit {:?} / bash {:?} {}",
                            i18n::text("Permissions:", "权限："),
                            self.permissions.write,
                            self.permissions.edit,
                            self.permissions.bash,
                            i18n::text("(this session only)", "（仅本次会话）")
                        )
                    }
                };
                self.output.push(format!("Hyper\n{text}"));
            }
            Some(Action::Choose(PickerKind::Preset, preset)) => {
                self.overlay = Some(Overlay::Form(ProviderForm::for_preset(&preset)));
            }
            Some(Action::Submit) => {
                let Some(Overlay::Form(form)) = &mut self.overlay else {
                    return;
                };
                match profiles::add_profile(
                    form.value(0),
                    form.value(1),
                    form.value(2),
                    form.value(3),
                ) {
                    Ok(()) => {
                        let name = form.value(0).to_owned();
                        self.refresh_provider();
                        self.output.push(format!(
                            "Hyper\n{} **{name}**",
                            i18n::text("Provider saved and selected:", "已保存并切换到服务商：")
                        ));
                        // Pick the model from what the new provider offers.
                        self.open_model_picker();
                    }
                    Err(error) => form.error = Some(format!("{error:#}")),
                }
            }
        }
    }

    fn open_session_picker(&mut self) {
        let sessions = workspace::Workspace::open(&self.root).and_then(|ws| ws.list_sessions(50));
        let items: Vec<PickerItem> = match sessions {
            Ok(sessions) => sessions
                .into_iter()
                .map(|session| PickerItem {
                    current: self.session.as_deref() == Some(session.session_id.as_str()),
                    label: session
                        .title
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(60)
                        .collect(),
                    detail: format!(
                        "{} {}  {}",
                        session.messages,
                        i18n::text("messages", "条消息"),
                        session
                            .updated_at
                            .get(..16)
                            .unwrap_or(&session.updated_at)
                            .replace('T', " ")
                    ),
                    value: session.session_id,
                })
                .collect(),
            Err(error) => {
                self.output.push(format!(
                    "Hyper\n{} {error:#}",
                    i18n::text("Could not list conversations:", "无法读取对话列表：")
                ));
                return;
            }
        };
        if items.is_empty() {
            self.output.push(
                i18n::text(
                    "Hyper\nNo earlier conversations in this workspace.",
                    "Hyper\n这个工作区还没有之前的对话。",
                )
                .into(),
            );
            return;
        }
        self.overlay = Some(Overlay::Picker(Picker::new(
            PickerKind::Session,
            i18n::text(" Resume conversation ", " 恢复对话 "),
            items,
        )));
    }

    /// Continue `id`: later messages join it, and its transcript is shown so
    /// the user sees what the model will remember.
    fn resume_session(&mut self, id: &str) {
        let messages =
            match workspace::Workspace::open(&self.root).and_then(|ws| ws.session_messages(id)) {
                Ok(messages) => messages,
                Err(error) => {
                    self.output.push(format!(
                        "Hyper\n{} {error:#}",
                        i18n::text("Could not open conversation:", "无法打开对话：")
                    ));
                    return;
                }
            };
        self.output.clear();
        self.rendered.clear();
        self.rendered_count = 0;
        self.event_tail.clear();
        self.usage = crate::RunUsage::default();
        self.session = Some(id.to_owned());
        self.session_id = Some(id.to_owned());
        for message in messages {
            let role = if message.role == "user" {
                "You"
            } else {
                "Hyper"
            };
            self.output.push(format!("{role}\n{}", message.content));
        }
        self.output.push(format!(
            "Hyper\n{} `{id}`",
            i18n::text("Resumed conversation", "已恢复对话")
        ));
        self.follow_tail = true;
        self.scroll = 0;
    }

    fn open_permissions_picker(&mut self) {
        let current = self.permissions.write;
        let item = |label: &str, detail: &str, value: &str, current: bool| PickerItem {
            label: label.into(),
            detail: detail.into(),
            value: value.into(),
            current,
        };
        let mut items = vec![
            item(
                i18n::text("Ask each time", "每次询问"),
                i18n::text(
                    "write / edit / bash ask first",
                    "write / edit / bash 执行前确认",
                ),
                "ask",
                current == crate::Permission::Ask,
            ),
            item(
                i18n::text("Allow all", "全部允许"),
                i18n::text(
                    "write / edit / bash run without asking",
                    "write / edit / bash 直接执行",
                ),
                "allow",
                current == crate::Permission::Allow,
            ),
            item(
                i18n::text("Read only", "只读"),
                i18n::text(
                    "write / edit / bash are refused",
                    "拒绝 write / edit / bash",
                ),
                "deny",
                current == crate::Permission::Deny,
            ),
        ];
        if !self.session_allows.is_empty() {
            items.push(item(
                &format!(
                    "{} ({})",
                    i18n::text("Forget \"always allow\" answers", "清除「总是允许」"),
                    self.session_allows.len()
                ),
                "",
                "clear",
                false,
            ));
        }
        self.overlay = Some(Overlay::Picker(Picker::new(
            PickerKind::Permissions,
            i18n::text(" Permissions (this session) ", " 权限（仅本次会话） "),
            items,
        )));
    }

    /// Summarize the conversation's older turns with the model, keeping the
    /// latest turn verbatim. Runs off the UI thread and can be cancelled.
    fn start_compaction(&mut self) {
        let Some(session) = self.session.clone() else {
            self.output.push(
                i18n::text(
                    "Hyper\nNo conversation to compact yet.",
                    "Hyper\n还没有可压缩的对话。",
                )
                .into(),
            );
            return;
        };
        self.busy = true;
        let cancellation = self.process_cancellation.child();
        self.cancellation = Some(cancellation.clone());
        let (root, tx) = (self.root.clone(), self.tx.clone());
        self.output.push(
            i18n::text(
                "Hyper\nCompacting older turns…",
                "Hyper\n正在压缩较早的对话…",
            )
            .into(),
        );
        self.worker = Some(std::thread::spawn(move || {
            let result = crate::compact_session(&root, &session, 1, cancellation)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Message::Compacted(result));
        }));
    }

    fn show_last_diff(&mut self) {
        let Some(run) = self.last_run.clone() else {
            self.output.push(
                i18n::text(
                    "Hyper\nNo run in this session yet.",
                    "Hyper\n本次会话还没有运行过任务。",
                )
                .into(),
            );
            return;
        };
        let text = match crate::run_file_diffs(&self.root, &run) {
            Ok(diffs) if diffs.is_empty() => {
                i18n::text("The last run changed no files.", "上次运行没有改动文件。").into()
            }
            Ok(diffs) => diffs
                .into_iter()
                .map(|(path, diff)| format!("**{path}**\n```diff\n{}\n```", diff.trim_end()))
                .collect::<Vec<_>>()
                .join("\n\n"),
            Err(error) => format!(
                "{} {error:#}",
                i18n::text("Could not read the diff:", "无法读取改动：")
            ),
        };
        self.output.push(format!("Hyper\n{text}"));
    }

    /// Undo overwrites files, possibly including edits made since the run, so
    /// it names the files and asks first.
    fn confirm_undo(&mut self) {
        let Some(run) = self.last_run.clone() else {
            self.output.push(
                i18n::text(
                    "Hyper\nNo run in this session to undo.",
                    "Hyper\n本次会话还没有可撤销的运行。",
                )
                .into(),
            );
            return;
        };
        let mut paths =
            match workspace::Workspace::open(&self.root).and_then(|ws| ws.list_checkpoints(&run)) {
                Ok(checkpoints) => checkpoints
                    .into_iter()
                    .map(|checkpoint| checkpoint.target_path)
                    .collect::<Vec<_>>(),
                Err(error) => {
                    self.output.push(format!(
                        "Hyper\n{} {error:#}",
                        i18n::text("Could not read checkpoints:", "无法读取检查点：")
                    ));
                    return;
                }
            };
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            self.output.push(
                i18n::text(
                    "Hyper\nThe last run changed no files.",
                    "Hyper\n上次运行没有改动文件。",
                )
                .into(),
            );
            return;
        }
        let item = |label: String, value: &str| PickerItem {
            label,
            detail: String::new(),
            value: value.into(),
            current: false,
        };
        self.overlay = Some(Overlay::Picker(Picker::new(
            PickerKind::ConfirmUndo,
            i18n::text(" Undo the last run? ", " 撤销上次运行？ "),
            vec![
                item(
                    format!(
                        "{} {}: {}",
                        i18n::text("Revert", "还原"),
                        paths.len(),
                        paths.join(", ")
                    ),
                    "yes",
                ),
                item(i18n::text("Cancel", "取消").into(), "no"),
            ],
        )));
    }

    fn undo_last_run(&mut self) {
        let Some(run) = self.last_run.take() else {
            return;
        };
        self.output.push(match crate::undo_run(&self.root, &run) {
            Ok(paths) => format!(
                "Hyper\n{} {}",
                i18n::text("Reverted:", "已还原："),
                paths.join(", ")
            ),
            Err(error) => {
                self.last_run = Some(run);
                format!(
                    "Hyper\n{} {error:#}",
                    i18n::text("Undo failed:", "撤销失败：")
                )
            }
        });
    }

    fn warn_env_model_override(&mut self) {
        if std::env::var("DEEPSEEK_MODEL").is_ok_and(|m| !m.trim().is_empty()) {
            self.output.push(
                i18n::text(
                    "Hyper\nNote: `DEEPSEEK_MODEL` is set and overrides the profile model.",
                    "Hyper\n注意：已设置 `DEEPSEEK_MODEL`，它会覆盖 profile 的模型。",
                )
                .into(),
            );
        }
    }

    fn approve_first(&mut self) {
        if let Some(request) = self.approvals.pop_front() {
            let _ = request.response.send(true);
        }
    }

    /// Approve the front request and stop asking for the same action again.
    fn approve_always(&mut self) {
        if let Some(request) = self.approvals.pop_front() {
            let key = allow_key(&request);
            let _ = request.response.send(true);
            // Queued requests for the same action no longer need a person.
            self.approvals.retain(|queued| {
                if allow_key(queued) == key {
                    let _ = queued.response.send(true);
                    false
                } else {
                    true
                }
            });
            self.session_allows.insert(key);
        }
    }

    /// Ctrl-C while idle: clear a draft first, and quit only on a second press
    /// within two seconds, so one stray keystroke never ends the session.
    fn interrupt_idle(&mut self) {
        if !self.input.text.is_empty() {
            self.input.clear();
            self.command_index = 0;
            self.quit_armed = None;
        } else if self
            .quit_armed
            .is_some_and(|armed| armed.elapsed() < Duration::from_secs(2))
        {
            self.request_quit();
        } else {
            self.quit_armed = Some(Instant::now());
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
        if !self.input.text.starts_with('/') || self.input.text.contains(char::is_whitespace) {
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
                "/resume",
                i18n::text("Resume an earlier conversation", "恢复之前的对话"),
            ),
            (
                "/diff",
                i18n::text("Show the last run's file changes", "查看上次运行的文件改动"),
            ),
            (
                "/undo",
                i18n::text(
                    "Revert the last run's file changes",
                    "撤销上次运行的文件改动",
                ),
            ),
            (
                "/compact",
                i18n::text(
                    "Summarize older turns to free context",
                    "压缩较早的对话以释放上下文",
                ),
            ),
            (
                "/permissions",
                i18n::text("Change what needs approval", "调整需要确认的操作"),
            ),
            ("/model", i18n::text("Select a model", "选择模型")),
            ("/profile", i18n::text("Select a provider", "选择服务商")),
            (
                "/connect",
                i18n::text(
                    "Add a provider (API key, base URL)",
                    "添加服务商（API Key、Base URL）",
                ),
            ),
            ("/cancel", i18n::text("Cancel current run", "取消当前运行")),
            ("/quit", i18n::text("Exit Hyper", "退出 Hyper")),
        ];
        commands
            .into_iter()
            .filter(|(command, _)| command.starts_with(self.input.text.as_str()))
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

    /// The `@query` being typed: an `@` at the start or after whitespace, with
    /// no whitespace between it and the cursor. Returns its byte offset.
    fn mention(&self) -> Option<(usize, String)> {
        let before = &self.input.text[..self.input.cursor.min(self.input.text.len())];
        let at = before.rfind('@')?;
        let query = &before[at + 1..];
        let starts_word = before[..at].chars().last().is_none_or(char::is_whitespace);
        (starts_word && !query.contains(char::is_whitespace)).then(|| (at, query.to_owned()))
    }

    /// Up to eight workspace files matching the `@` query: file names starting
    /// with it first, then any path containing it, shorter paths first.
    pub fn mention_suggestions(&mut self) -> Vec<String> {
        let Some((_, query)) = self.mention() else {
            return Vec::new();
        };
        if self
            .files
            .as_ref()
            .is_none_or(|(loaded, _)| loaded.elapsed() > Duration::from_secs(10))
        {
            let files = crate::engine::workspace_files(&self.root, false, 20_000);
            self.files = Some((Instant::now(), files));
        }
        let query = query.to_lowercase();
        let files = &self.files.as_ref().expect("loaded above").1;
        let mut matches: Vec<(bool, usize, &String)> = files
            .iter()
            .filter_map(|path| {
                let lower = path.to_lowercase();
                let name = lower.rsplit('/').next().unwrap_or(&lower);
                lower
                    .contains(&query)
                    .then(|| (!name.starts_with(&query), path.len(), path))
            })
            .collect();
        matches.sort();
        matches
            .into_iter()
            .take(8)
            .map(|(_, _, path)| path.clone())
            .collect()
    }

    /// Replace the `@query` with the chosen path.
    fn complete_mention(&mut self) -> bool {
        let suggestions = self.mention_suggestions();
        let (Some((at, _)), Some(path)) = (
            self.mention(),
            suggestions.get(self.mention_index.min(suggestions.len().saturating_sub(1))),
        ) else {
            return false;
        };
        let end = self.input.cursor.min(self.input.text.len());
        let path = path.clone();
        self.input.text.replace_range(at..end, &format!("@{path} "));
        self.input.cursor = at + path.len() + 2;
        self.mention_index = 0;
        true
    }

    fn complete_command(&mut self) -> bool {
        let suggestions = self.command_suggestions();
        let Some((command, _)) =
            suggestions.get(self.command_index.min(suggestions.len().saturating_sub(1)))
        else {
            return false;
        };
        self.input.set(*command);
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
                if self.session_allows.contains(&allow_key(&request)) {
                    self.event_tail.push_back(format!(
                        "{} {}: {}",
                        i18n::text("allowed for this session", "本会话已允许"),
                        request.tool,
                        request.detail
                    ));
                    let _ = request.response.send(true);
                } else {
                    self.approvals.push_back(request);
                }
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
                Message::Models(result) => {
                    // Ignored if the picker was closed before the list arrived.
                    if let Some(Overlay::Picker(picker)) = &mut self.overlay
                        && picker.kind == PickerKind::Model
                        && picker.loading
                    {
                        match result {
                            Ok(models) if models.is_empty() => {
                                picker.loading = false;
                                picker.error = Some(
                                    i18n::text(
                                        "The provider listed no models.",
                                        "服务商没有返回任何模型。",
                                    )
                                    .into(),
                                );
                            }
                            Ok(models) => picker.set_items(
                                models
                                    .into_iter()
                                    .map(|id| PickerItem {
                                        current: id == self.model,
                                        label: id.clone(),
                                        detail: String::new(),
                                        value: id,
                                    })
                                    .collect(),
                            ),
                            Err(error) => {
                                picker.loading = false;
                                picker.error = Some(error);
                            }
                        }
                    }
                }
                Message::Compacted(result) => {
                    self.busy = false;
                    self.cancellation = None;
                    if let Some(worker) = self.worker.take() {
                        let _ = worker.join();
                    }
                    if self.quit_requested {
                        self.quit = true;
                    }
                    self.output.push(match result {
                        Ok(record) if record.status == "finished" => {
                            // The next request starts from the summary.
                            self.usage.last_input = 0;
                            format!(
                                "Hyper\n{} {} {} (~{} → ~{} tokens)",
                                i18n::text("Compacted", "已压缩"),
                                record.covered_messages,
                                i18n::text("messages", "条消息"),
                                record.covered_estimated_tokens,
                                record.summary_estimated_tokens
                            )
                        }
                        Ok(record) => format!(
                            "Hyper\n{} {}: {}",
                            i18n::text("Compaction", "压缩"),
                            record.status,
                            record.error.unwrap_or_default()
                        ),
                        Err(error) => format!(
                            "Hyper\n{} {error}",
                            i18n::text("Could not compact:", "无法压缩：")
                        ),
                    });
                }
                Message::Task(r) => {
                    // A first run creates the workspace storage history waits on.
                    self.history.flush();
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
                    let text = match r {
                        Ok(done) => {
                            self.last_run = Some(done.run_id);
                            if done.usage.input > 0 {
                                self.usage.last_input = done.usage.last_input;
                            }
                            self.usage.input += done.usage.input;
                            self.usage.output += done.usage.output;
                            done.text
                        }
                        Err(e) => format!("{} {e}", i18n::text("Failed:", "执行失败：")),
                    };
                    self.output.push(format!("Hyper\n{text}"));
                }
            }
        }
    }

    /// Keys for the message box and chat while no modal or overlay is open.
    fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        let alt = modifiers.contains(KeyModifiers::ALT);
        let editing = !self.input.text.is_empty();
        if !matches!(code, KeyCode::Up | KeyCode::Down) && !matches!(code, KeyCode::Char(_) if ctrl)
        {
            self.command_index = 0;
        }
        if !self.mention_suggestions().is_empty() {
            let count = self.mention_suggestions().len();
            match code {
                KeyCode::Up => {
                    self.mention_index = self.mention_index.checked_sub(1).unwrap_or(count - 1);
                    return;
                }
                KeyCode::Down => {
                    self.mention_index = (self.mention_index + 1) % count;
                    return;
                }
                KeyCode::Tab | KeyCode::Enter if modifiers.is_empty() => {
                    self.complete_mention();
                    return;
                }
                _ => self.mention_index = 0,
            }
        }
        match code {
            // Esc never quits: it cancels a run or clears the draft.
            KeyCode::Esc if self.busy => self.cancel_active(),
            KeyCode::Esc => self.input.clear(),
            KeyCode::Tab => {
                if !self.complete_command() {
                    self.mode = if self.mode == AgentMode::Build {
                        AgentMode::Plan
                    } else {
                        AgentMode::Build
                    }
                }
            }
            // Shift/Alt-Enter (where the terminal reports them) and Ctrl-J add
            // a line; a trailing `\` works in every terminal.
            KeyCode::Enter if modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                self.input.insert("\n")
            }
            KeyCode::Char('j') if ctrl => self.input.insert("\n"),
            KeyCode::Enter if self.input.ends_with_continuation() => self.input.continue_line(),
            KeyCode::Enter => {
                self.complete_command();
                self.submit();
            } // While recalling history, a recalled `/command` must not hand ↑/↓
            // to the command palette.
            KeyCode::Up if !self.history.browsing() && !self.command_suggestions().is_empty() => {
                self.move_command_up()
            }
            KeyCode::Down if !self.history.browsing() && !self.command_suggestions().is_empty() => {
                self.move_command_down()
            }
            KeyCode::Up => {
                if !self.input.up() {
                    match self.history.previous(&self.input.text) {
                        Some(entry) => {
                            let entry = entry.to_owned();
                            self.input.set(entry);
                        }
                        None => self.scroll_up(1),
                    }
                }
            }
            KeyCode::Down => {
                if !self.input.down() {
                    match self.history.next() {
                        Some(entry) => {
                            let entry = entry.to_owned();
                            self.input.set(entry);
                        }
                        None => self.scroll_down(1),
                    }
                }
            }
            KeyCode::Left if ctrl || alt => self.input.word_left(),
            KeyCode::Right if ctrl || alt => self.input.word_right(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            // With an empty box Home/End jump the chat, as before.
            KeyCode::Home if editing => self.input.home(),
            KeyCode::End if editing => self.input.end(),
            KeyCode::Home => {
                self.follow_tail = false;
                self.scroll = u16::MAX;
            }
            KeyCode::End => {
                self.follow_tail = true;
                self.scroll = 0;
            }
            KeyCode::PageUp => self.scroll_up(8),
            KeyCode::PageDown => self.scroll_down(8),
            KeyCode::Backspace if ctrl || alt => self.input.delete_word(),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Char('a') if ctrl => self.input.home(),
            KeyCode::Char('e') if ctrl => self.input.end(),
            KeyCode::Char('w') if ctrl => self.input.delete_word(),
            KeyCode::Char('u') if ctrl => self.input.kill_to_start(),
            KeyCode::Char('k') if ctrl => self.input.kill_to_end(),
            KeyCode::Char('b') if alt => self.input.word_left(),
            KeyCode::Char('f') if alt => self.input.word_right(),
            // Other control chords are not text.
            KeyCode::Char(_) if ctrl || alt => {}
            KeyCode::Char(c) => self.input.insert(c.encode_utf8(&mut [0; 4])),
            _ => {}
        }
    }

    fn paste(&mut self, text: &str) {
        if self.approvals.is_empty() {
            match &mut self.overlay {
                Some(overlay) => overlay.paste(text),
                None => self.input.insert(text),
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
/// What "always allow" remembers: the exact command for bash, since one
/// approved command says nothing about another; the tool for write/edit.
fn allow_key(request: &ApprovalRequest) -> String {
    if request.tool == "bash" {
        format!("bash:{}", request.detail)
    } else {
        request.tool.clone()
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
    run_with_onboarding(root, session, execution_mode, permissions, false)
}

/// `onboarding` opens the provider setup first, for a first run with no key.
pub fn run_with_onboarding(
    root: PathBuf,
    session: Option<String>,
    execution_mode: ExecutionMode,
    permissions: crate::ToolPermissions,
    onboarding: bool,
) -> Result<()> {
    let mut terminal = ratatui::init();
    // Bracketed paste delivers a paste as one event, so its newlines land in
    // the message instead of each one sending it.
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let result = (|| {
        let mut app = App::new(root, session.clone(), execution_mode);
        app.launch_permissions = permissions.clone();
        app.permissions = permissions;
        if let Some(id) = &session {
            app.resume_session(id);
        }
        if onboarding {
            app.output.push(i18n::text(
                "Hyper\nNo model provider is configured yet. Pick one to start; you can add more later with `/connect`.",
                "Hyper\n还没有配置模型服务商。先选一个开始，之后可用 `/connect` 继续添加。",
            ).into());
            app.overlay = Some(Overlay::Picker(Picker::new(
                PickerKind::Preset,
                i18n::text(" Add provider ", " 添加服务商 "),
                overlay::presets(),
            )));
        }
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
            if let Event::Paste(text) = input_event {
                app.paste(&text);
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
                } else if app.overlay.is_some() {
                    app.overlay = None;
                } else {
                    app.interrupt_idle();
                }
                continue;
            }
            app.quit_armed = None;
            // Esc in the modal denies just that action; Ctrl-C cancels the run.
            // While an approval is pending, all other keys answer the modal.
            if !app.approvals.is_empty() {
                match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.approve_first(),
                    KeyCode::Char('a') | KeyCode::Char('A') => app.approve_always(),
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.deny_first(),
                    _ => {}
                }
                continue;
            }
            if app.overlay.is_some() {
                app.overlay_key(k.code);
                continue;
            }
            app.key(k.code, k.modifiers);
        }
        // Unblock any tool waiting on an approval when we are quitting.
        app.deny_all();
        Ok(())
    })();
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_and_approve(app: &mut App, instruction: &str) {
        app.input.set(instruction);
        app.submit();
        let started = std::time::Instant::now();
        while app.busy {
            app.poll();
            if !app.approvals.is_empty() {
                app.approve_first();
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "run never finished"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn diff_and_undo_act_on_the_last_run_after_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        let mut app = App::new(dir.path().to_owned(), None, ExecutionMode::default());
        app.input.set("/undo");
        app.submit();
        assert!(app.overlay.is_none(), "nothing to undo yet");
        run_and_approve(&mut app, "write:a.txt\nafter\n");
        assert!(app.last_run.is_some());
        app.input.set("/diff");
        app.submit();
        let shown = app.output.last().unwrap();
        assert!(
            shown.contains("-before") && shown.contains("+after"),
            "{shown}"
        );

        app.input.set("/undo");
        app.submit();
        assert!(
            matches!(&app.overlay, Some(Overlay::Picker(p)) if p.kind == PickerKind::ConfirmUndo)
        );
        app.overlay_key(KeyCode::Down);
        app.overlay_key(KeyCode::Enter);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "after",
            "cancel keeps the file"
        );
        app.input.set("/undo");
        app.submit();
        app.overlay_key(KeyCode::Enter);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "before\n"
        );
        assert!(app.last_run.is_none());
    }

    #[test]
    fn resume_picker_loads_an_earlier_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace::Workspace::open(dir.path()).unwrap();
        for (role, content) in [("user", "fix the parser"), ("assistant", "Fixed it.")] {
            workspace
                .append_session_message(
                    "earlier-chat",
                    &crate::SessionMessage {
                        role: role.into(),
                        content: content.into(),
                        timestamp: "2026-10-14T01:00:00Z".into(),
                        run_id: None,
                    },
                )
                .unwrap();
        }
        let mut app = App::new(dir.path().to_owned(), None, ExecutionMode::default());
        app.input.set("/resume");
        app.submit();
        match &app.overlay {
            Some(Overlay::Picker(picker)) => {
                assert_eq!(picker.visible()[0].label, "fix the parser")
            }
            _ => panic!("expected the conversation picker"),
        }
        app.overlay_key(KeyCode::Enter);
        assert_eq!(app.session.as_deref(), Some("earlier-chat"));
        assert!(
            app.output
                .iter()
                .any(|message| message == "You\nfix the parser")
        );
        assert!(
            app.output
                .iter()
                .any(|message| message == "Hyper\nFixed it.")
        );
    }

    #[test]
    fn at_mentions_complete_workspace_paths_without_sending() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        for file in ["src/main.rs", "src/domain.rs", "README.md"] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        let mut app = App::new(dir.path().to_owned(), None, ExecutionMode::default());
        type_text(&mut app, "mail me@main.rs");
        assert!(
            app.mention_suggestions().is_empty(),
            "an email address is not a mention"
        );
        app.input.clear();
        type_text(&mut app, "look at @MAI");
        assert_eq!(
            app.mention_suggestions(),
            ["src/main.rs", "src/domain.rs"],
            "file names starting with it come first"
        );
        app.key(KeyCode::Down, KeyModifiers::NONE);
        app.key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.input.text, "look at @src/domain.rs ");
        assert_eq!(
            app.output.len(),
            1,
            "Enter inserted the path instead of sending"
        );
        assert!(app.mention_suggestions().is_empty());
    }

    #[test]
    fn permissions_picker_loosens_and_tightens_for_the_session() {
        let mut app = App::new(PathBuf::from("."), None, ExecutionMode::default());
        assert_eq!(app.permissions.write, crate::Permission::Ask);
        app.input.set("/permissions");
        app.submit();
        app.overlay_key(KeyCode::Down);
        app.overlay_key(KeyCode::Enter);
        assert_eq!(app.permissions.bash, crate::Permission::Allow);
        app.input.set("/permissions");
        app.submit();
        app.overlay_key(KeyCode::Up);
        app.overlay_key(KeyCode::Enter);
        assert_eq!(
            app.permissions.bash,
            crate::Permission::Ask,
            "a stricter choice undoes a looser one"
        );
        assert!(app.permissions.source.ends_with("+ /permissions"));
    }

    #[test]
    fn cancel_command_unblocks_modal_and_conversation_can_continue() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path().to_owned(), None, ExecutionMode::default());
        app.input.set("write:out.txt\nwrong");
        app.submit();
        let started = std::time::Instant::now();
        while app.approvals.is_empty() {
            app.poll();
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
        app.input.set("/cancel");
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
        app.input.set("write:out.txt\nok");
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
        app.input.set("write:out.txt\nwrong");
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
        app.input.set("write:out.txt\nwrong");
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
        app.input.set("/mo");
        assert_eq!(app.command_suggestions().len(), 3);
        assert!(app.complete_command());
        assert_eq!(app.input.text, "/mode plan");
        app.input.set("/model");
        assert!(app.complete_command());
        assert_eq!(app.input.text, "/model");
        app.input.set("/pro");
        assert!(app.complete_command());
        assert_eq!(app.input.text, "/profile");
    }

    fn request(tool: &str, detail: &str) -> (ApprovalRequest, std::sync::mpsc::Receiver<bool>) {
        let (response, receiver) = mpsc::channel();
        let request = ApprovalRequest {
            id: workspace::id(),
            tool: tool.into(),
            detail: detail.into(),
            preview: None,
            response,
        };
        (request, receiver)
    }

    #[test]
    fn always_allow_answers_matching_requests_for_the_session() {
        let mut app = App::new(PathBuf::from("."), None, ExecutionMode::default());
        let (first, first_rx) = request("bash", "cargo test");
        let (same, same_rx) = request("bash", "cargo test");
        let (other, other_rx) = request("bash", "rm -rf build");
        app.approvals.extend([first, same, other]);
        app.approve_always();
        assert!(first_rx.recv().unwrap());
        assert!(
            same_rx.recv().unwrap(),
            "a queued identical command is approved too"
        );
        assert_eq!(app.approvals.len(), 1, "a different command still asks");
        assert!(other_rx.try_recv().is_err());

        // Later requests for the same command are answered without a modal.
        let gate = ApprovalGate::new();
        app.gate = Some(gate.clone());
        let worker = std::thread::spawn(move || gate.request("bash", "cargo test"));
        while !worker.is_finished() {
            app.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(worker.join().unwrap());
        assert_eq!(app.approvals.len(), 1);
    }

    #[test]
    fn approval_modal_shows_the_whole_command_and_the_diff() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = App::new(PathBuf::from("."), None, ExecutionMode::default());
        let long = format!("cargo test --workspace {}", "--features x ".repeat(12));
        let (bash, _bash_rx) = request("bash", &long);
        app.approvals.push_back(bash);
        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
        let screen = |terminal: &mut Terminal<TestBackend>, app: &mut App| {
            terminal.draw(|frame| ui::draw(frame, app)).unwrap();
            let buffer = terminal.backend().buffer();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let text = screen(&mut terminal, &mut app);
        let joined: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(joined.contains("--features x --features x"), "{text}");
        assert!(
            text.matches("--features").count() >= 12,
            "the command is wrapped, not cut:\n{text}"
        );

        app.approvals.clear();
        let (mut edit, _edit_rx) = request("edit", "demo.txt");
        edit.preview =
            Some("--- demo.txt\n+++ demo.txt\n@@ -1 +1 @@\n-old line\n+new line\n".into());
        app.approvals.push_back(edit);
        let text = screen(&mut terminal, &mut app);
        assert!(
            text.contains("-old line") && text.contains("+new line"),
            "{text}"
        );
        assert!(
            text.contains("a 本会话总是允许此工具") || text.contains("a Always allow this tool"),
            "{text}"
        );
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.key(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    #[test]
    fn message_box_edits_mid_line_and_adds_lines_without_sending() {
        let mut app = App::new(PathBuf::from("."), None, ExecutionMode::default());
        type_text(&mut app, "fix tst");
        app.key(KeyCode::Left, KeyModifiers::NONE);
        app.key(KeyCode::Left, KeyModifiers::NONE);
        type_text(&mut app, "e");
        assert_eq!(app.input.text, "fix test");
        app.key(KeyCode::End, KeyModifiers::NONE);
        app.key(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(
            app.input.text, "fix ",
            "Ctrl-W deletes a word, never types 'w'"
        );
        type_text(&mut app, "it");
        app.key(KeyCode::Enter, KeyModifiers::SHIFT);
        app.key(KeyCode::Char('j'), KeyModifiers::CONTROL);
        type_text(&mut app, "more \\");
        app.key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.input.text, "fix it\n\nmore \n");
        assert!(app.output.len() == 1, "nothing was sent");
        // A multi-line paste stays in the box.
        app.input.clear();
        app.paste("line one\r\nline two");
        assert_eq!(app.input.text, "line one\nline two");
        assert_eq!(app.output.len(), 1);
    }

    #[test]
    fn up_and_down_recall_earlier_messages() {
        let mut app = App::new(PathBuf::from("."), None, ExecutionMode::default());
        app.history = input::History::load(None);
        for command in ["/session", "/help"] {
            type_text(&mut app, command);
            app.key(KeyCode::Enter, KeyModifiers::NONE);
        }
        type_text(&mut app, "draft");
        app.key(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.input.text, "/help");
        app.key(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.input.text, "/session");
        app.key(KeyCode::Down, KeyModifiers::NONE);
        app.key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.input.text, "draft", "the draft comes back");
    }

    #[test]
    fn esc_and_a_single_ctrl_c_never_quit_an_idle_session() {
        let mut app = App::new(PathBuf::from("."), None, ExecutionMode::default());
        app.input.set("draft");
        app.interrupt_idle();
        assert!(
            app.input.text.is_empty() && !app.quit,
            "the first Ctrl-C clears the draft"
        );
        app.interrupt_idle();
        assert!(
            !app.quit && app.quit_armed.is_some(),
            "an empty input arms quitting"
        );
        app.interrupt_idle();
        assert!(app.quit, "a second press within two seconds quits");

        let mut app = App::new(PathBuf::from("."), None, ExecutionMode::default());
        app.quit_armed = Some(Instant::now() - Duration::from_secs(3));
        app.interrupt_idle();
        assert!(!app.quit, "a stale first press does not count");
    }

    #[test]
    fn connect_walks_from_presets_to_the_form_and_esc_closes() {
        let mut app = App::new(PathBuf::from("."), None, ExecutionMode::default());
        app.input.set("/connect");
        app.submit();
        assert!(matches!(&app.overlay, Some(Overlay::Picker(p)) if p.kind == PickerKind::Preset));
        app.overlay_key(KeyCode::Down);
        app.overlay_key(KeyCode::Enter);
        match &app.overlay {
            Some(Overlay::Form(form)) => assert_eq!(form.value(1), "https://opencode.ai/zen/go/v1"),
            _ => panic!("expected the provider form"),
        }
        app.overlay_key(KeyCode::Esc);
        assert!(app.overlay.is_none());
        assert!(
            app.input.text.is_empty(),
            "overlay keys never reach the message box"
        );
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
        app.input.set("/new");
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

        app.input.set("/session");
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
        app.input.set("/session");
        app.submit();
        assert!(
            app.output.last().unwrap().contains("chat-1"),
            "{:?}",
            app.output.last()
        );
    }
}
