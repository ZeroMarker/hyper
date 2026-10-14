use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use super::{
    App,
    overlay::{Overlay, Picker, ProviderForm},
};
use crate::{AgentMode, i18n};

pub fn draw(frame: &mut Frame, app: &mut App) {
    // Only parse markdown for messages added since the previous frame.
    for message in &app.output[app.rendered_count..] {
        let mut lines = message.lines();
        let role = lines.next().unwrap_or("Hyper");
        let role_color = if role == "You" {
            Color::Cyan
        } else {
            Color::Green
        };
        app.rendered.push(Line::styled(
            role.to_owned(),
            Style::default().fg(role_color).add_modifier(Modifier::BOLD),
        ));
        let markdown = lines.collect::<Vec<_>>().join("\n");
        app.rendered.extend(markdown_lines(&markdown));
        app.rendered.push(Line::from(""));
    }
    app.rendered_count = app.output.len();
    let (input_rows, cursor_row, cursor_column) =
        wrap_input(&app.input, frame.area().width.saturating_sub(2).max(1));
    // The box grows with the message, up to eight rows, then scrolls.
    let input_height = input_rows.len().clamp(1, 8) as u16;
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(input_height + 2),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let mode_color = if app.mode == AgentMode::Build {
        Color::Yellow
    } else {
        Color::Cyan
    };
    let header = Line::from(vec![
        Span::styled(" Hyper ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            match &app.profile {
                Some(profile) => format!("{profile} / {}", app.model),
                None => app.model.clone(),
            },
            Style::default().fg(Color::Magenta),
        ),
        Span::raw("  •  "),
        Span::styled(app.mode.as_str(), Style::default().fg(mode_color)),
        Span::styled(
            format!("  •  {}", app.execution_mode.as_str()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!(
                "  •  write {:?} / edit {:?} / bash {:?}",
                app.permissions.write, app.permissions.edit, app.permissions.bash
            ),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            if app.usage.input > 0 {
                format!(
                    "  •  ctx {} · ↑{} ↓{}",
                    tokens(app.usage.last_input),
                    tokens(app.usage.input),
                    tokens(app.usage.output)
                )
            } else {
                String::new()
            },
            Style::default().fg(Color::DarkGray),
        ),
        // Which conversation the next message joins, so a follow-up is never
        // silently answered by a fresh context.
        Span::styled(
            match &app.session {
                Some(id) => format!("  •  {id}"),
                None => "  •  new session".into(),
            },
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::BOTTOM)),
        areas[0],
    );

    let mut chat = app.rendered.clone();
    if app.busy && !app.live_text.is_empty() {
        chat.push(Line::styled(
            "Hyper",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
        chat.extend(app.live_text.lines().map(|line| Line::raw(line.to_owned())));
        chat.push(Line::from(""));
    }
    if !app.event_tail.is_empty() {
        chat.push(Line::styled(
            "Events",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
        chat.extend(
            app.event_tail
                .iter()
                .map(|event| Line::styled(event.clone(), Style::default().fg(Color::DarkGray))),
        );
        chat.push(Line::from(""));
    }
    if app.busy {
        const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let status = if app
            .cancellation
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            format!(
                "{} {}",
                SPINNER[app.tick % SPINNER.len()],
                i18n::text("Cancelling", "正在取消")
            )
        } else if app.approvals.is_empty() {
            format!(
                "{} {}",
                SPINNER[app.tick % SPINNER.len()],
                i18n::text("Thinking", "正在思考")
            )
        } else {
            format!(
                "{} {}",
                SPINNER[app.tick % SPINNER.len()],
                i18n::text("Waiting for approval", "等待确认")
            )
        };
        chat.push(Line::from(vec![
            Span::styled(
                "Hyper ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(status, Style::default().fg(Color::Magenta)),
        ]));
    }
    let visible_height = areas[1].height.saturating_sub(2) as usize;
    let max_scroll = chat.len().saturating_sub(visible_height) as u16;
    let scroll = if app.follow_tail {
        max_scroll
    } else {
        max_scroll.saturating_sub(app.scroll)
    };
    frame.render_widget(
        Paragraph::new(chat)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        areas[1],
    );

    // Keep the cursor's row in view.
    let scroll = cursor_row.saturating_sub(input_height.saturating_sub(1));
    let multiline = input_rows.len() > 1;
    frame.render_widget(
        Paragraph::new(input_rows.into_iter().map(Line::raw).collect::<Vec<_>>())
            .scroll((scroll, 0))
            .block(
                Block::default()
                    .title(if multiline {
                        i18n::text(
                            " Message — Enter send · Shift/Alt-Enter or Ctrl-J newline ",
                            " 消息 — Enter 发送 · Shift/Alt-Enter 或 Ctrl-J 换行 ",
                        )
                    } else {
                        " Message "
                    })
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(mode_color)),
            ),
        areas[2],
    );
    frame.set_cursor_position((
        areas[2].x + 1 + cursor_column,
        areas[2].y + 1 + cursor_row - scroll,
    ));

    render_approval(frame, app, areas[2]);
    render_command_palette(frame, app, areas[2]);
    match &app.overlay {
        Some(Overlay::Picker(picker)) => render_picker(frame, picker),
        Some(Overlay::Form(form)) => render_form(frame, form),
        None => {}
    }

    let hint = match &app.overlay {
        Some(Overlay::Picker(_)) => i18n::text(
            " Type to filter  ↑↓ Select  Enter Choose  Esc Close",
            " 输入筛选  ↑↓ 选择  Enter 确认  Esc 关闭",
        ),
        Some(Overlay::Form(_)) => i18n::text(
            " Tab/↑↓ Field  Enter Next / Save  Esc Cancel",
            " Tab/↑↓ 切换字段  Enter 下一项 / 保存  Esc 取消",
        ),
        None if app.quit_armed.is_some() => {
            i18n::text(" Press Ctrl-C again to exit", " 再按一次 Ctrl-C 退出")
        }
        None => i18n::text(
            " ↑↓ History  PgUp/PgDn Scroll  Tab Mode  Enter Send  Ctrl-J Newline  /model Model  Esc Clear/Cancel  Ctrl-C×2 Exit",
            " ↑↓ 历史  PgUp/PgDn 滚动  Tab 模式  Enter 发送  Ctrl-J 换行  /model 模型  Esc 清空/取消  Ctrl-C×2 退出",
        ),
    };
    frame.render_widget(
        Paragraph::new(hint)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray)),
        areas[3],
    );
}

fn render_approval(frame: &mut Frame, app: &App, input_area: Rect) {
    let Some(request) = app.approvals.front() else {
        return;
    };
    let width = input_area.width.saturating_sub(4).max(1) as usize;
    let mut body: Vec<Line> = Vec::new();
    // The full command or target, wrapped rather than cut: approving what you
    // cannot read is not approval.
    for line in request.detail.lines() {
        body.push(Line::styled(
            line.to_owned(),
            Style::default().fg(Color::White),
        ));
    }
    if let Some(preview) = &request.preview {
        body.push(Line::from(""));
        for line in preview.lines() {
            let color = if line.starts_with("+++") || line.starts_with("---") {
                Color::Gray
            } else if line.starts_with('+') {
                Color::Green
            } else if line.starts_with('-') {
                Color::Red
            } else if line.starts_with("@@") {
                Color::Cyan
            } else {
                Color::DarkGray
            };
            body.push(Line::styled(line.to_owned(), Style::default().fg(color)));
        }
    }
    // Rows each line takes once wrapped, to size the box.
    let rows = |line: &Line| (line.width().max(1)).div_ceil(width);
    let max_body = (frame.area().height * 3 / 5).saturating_sub(6).max(3) as usize;
    let mut used = 0;
    let mut shown = Vec::new();
    for line in &body {
        if used + rows(line) > max_body {
            break;
        }
        used += rows(line);
        shown.push(line.clone());
    }
    let hidden = body.len() - shown.len();
    if hidden > 0 {
        shown.push(Line::styled(
            format!(
                "… {hidden} {}",
                i18n::text("more lines not shown", "行未显示")
            ),
            Style::default().fg(Color::DarkGray),
        ));
        used += 1;
    }
    let always = if request.tool == "bash" {
        i18n::text("a Always allow this command", "a 本会话总是允许此命令")
    } else {
        i18n::text("a Always allow this tool", "a 本会话总是允许此工具")
    };
    let mut lines = vec![
        Line::from(Span::styled(
            format!(" {} {}", i18n::text("Approve:", "需要确认："), request.tool),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    lines.extend(shown);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " {}    {always}    {}",
            i18n::text("y Allow", "y 允许"),
            i18n::text(
                "n / Esc Deny    Ctrl-C Cancel run",
                "n / Esc 拒绝    Ctrl-C 取消运行"
            )
        ),
        Style::default().fg(Color::Cyan),
    )));
    let height = (used as u16 + 6).min(input_area.y.max(7));
    let area = Rect::new(
        input_area.x,
        input_area.y.saturating_sub(height),
        input_area.width,
        height,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(" Approval ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        ),
        area,
    );
}

/// A token count in thousands once it is large: 950, 12.3k, 215k.
fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..100_000 => format!("{:.1}k", count as f64 / 1_000.0),
        _ => format!("{}k", count / 1_000),
    }
}

/// Split the message into display rows of at most `width` columns, wrapping at
/// any character so the cursor maps to an exact cell. Returns the rows and the
/// cursor's row and column.
fn wrap_input(input: &super::input::Input, width: u16) -> (Vec<String>, u16, u16) {
    use unicode_width::UnicodeWidthChar;
    let width = width as usize;
    let cursor = input.cursor.min(input.text.len());
    let mut rows = vec![String::new()];
    let mut column = 0usize;
    let mut position = (0, 0);
    for (index, ch) in input.text.char_indices() {
        if index == cursor {
            position = (rows.len() - 1, column);
        }
        if ch == '\n' {
            rows.push(String::new());
            column = 0;
            continue;
        }
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if column + ch_width > width {
            rows.push(String::new());
            column = 0;
        }
        rows.last_mut().expect("rows start non-empty").push(ch);
        column += ch_width;
    }
    if cursor >= input.text.len() {
        // A cursor at the end of a full row starts the next one.
        if column >= width {
            rows.push(String::new());
            column = 0;
        }
        position = (rows.len() - 1, column);
    }
    (rows, position.0 as u16, position.1 as u16)
}

/// A box centered in the frame, `percent` of its width and up to `height` rows.
fn centered(frame: &Frame, percent: u16, height: u16) -> Rect {
    let area = frame.area();
    let width = (area.width * percent / 100).max(40).min(area.width);
    let height = height.min(area.height.saturating_sub(2)).max(5);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn render_picker(frame: &mut Frame, picker: &Picker) {
    let visible = picker.visible();
    let rows = if picker.loading || picker.error.is_some() {
        1
    } else {
        visible.len().max(1)
    };
    let area = centered(frame, 60, rows as u16 + 4);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(picker.title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [filter_area, list_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .areas(inner);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" › ", Style::default().fg(Color::Cyan)),
            Span::raw(picker.filter.as_str()),
        ]))
        .block(Block::default().borders(Borders::BOTTOM)),
        filter_area,
    );
    frame.set_cursor_position((
        filter_area.x + 3 + UnicodeWidthStr::width(picker.filter.as_str()) as u16,
        filter_area.y,
    ));
    if picker.loading {
        frame.render_widget(
            Paragraph::new(i18n::text(" Loading…", " 正在加载…"))
                .style(Style::default().fg(Color::DarkGray)),
            list_area,
        );
        return;
    }
    if let Some(error) = &picker.error
        && visible.is_empty()
    {
        frame.render_widget(
            Paragraph::new(format!(" {error}"))
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(Color::Red)),
            list_area,
        );
        return;
    }
    if visible.is_empty() {
        frame.render_widget(
            Paragraph::new(i18n::text(" No matches", " 没有匹配项"))
                .style(Style::default().fg(Color::DarkGray)),
            list_area,
        );
        return;
    }
    let items = visible.iter().map(|item| {
        let mut spans = vec![Span::styled(
            format!(" {} {}", if item.current { "●" } else { " " }, item.label),
            Style::default().fg(if item.current {
                Color::Green
            } else {
                Color::White
            }),
        )];
        if !item.detail.is_empty() {
            spans.push(Span::styled(
                format!("  {}", item.detail),
                Style::default().fg(Color::DarkGray),
            ));
        }
        ListItem::new(Line::from(spans))
    });
    let mut state =
        ListState::default().with_selected(Some(picker.selected.min(visible.len() - 1)));
    frame.render_stateful_widget(
        List::new(items).highlight_symbol("›").highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        list_area,
        &mut state,
    );
}

fn render_form(frame: &mut Frame, form: &ProviderForm) {
    let area = centered(frame, 60, form.fields.len() as u16 + 6);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(i18n::text(" Add provider ", " 添加服务商 "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    const LABEL: usize = 10;
    let mut lines = Vec::new();
    let mut cursor = None;
    for (index, field) in form.fields.iter().enumerate() {
        let focused = index == form.focus;
        // The key is never drawn; only its length is visible while typing.
        let shown = if field.secret {
            "•".repeat(field.value.chars().count())
        } else {
            field.value.clone()
        };
        let label = format!(
            " {}{:<width$}",
            if focused { "›" } else { " " },
            field.label,
            width = LABEL
        );
        if focused {
            cursor = Some((
                inner.x
                    + UnicodeWidthStr::width(label.as_str()) as u16
                    + 1
                    + UnicodeWidthStr::width(shown.as_str()) as u16,
                inner.y + index as u16,
            ));
        }
        lines.push(Line::from(vec![
            Span::styled(
                label,
                Style::default().fg(if focused { Color::Cyan } else { Color::Gray }),
            ),
            Span::raw(" "),
            Span::styled(shown, Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        i18n::text(
            " The key is stored in your owner-only Hyper config file.",
            " Key 保存在仅你可读的 Hyper 配置文件中。",
        ),
        Style::default().fg(Color::DarkGray),
    ));
    if let Some(error) = &form.error {
        lines.push(Line::styled(
            format!(" {error}"),
            Style::default().fg(Color::Red),
        ));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    if let Some(position) = cursor {
        frame.set_cursor_position(position);
    }
}

fn render_command_palette(frame: &mut Frame, app: &App, input_area: Rect) {
    if !app.approvals.is_empty() {
        return;
    }
    let suggestions = app.command_suggestions();
    if suggestions.is_empty() {
        return;
    }
    let height = (suggestions.len() as u16 + 2).min(9);
    let area = Rect::new(
        input_area.x,
        input_area.y.saturating_sub(height),
        input_area.width,
        height,
    );
    let items = suggestions.iter().map(|(command, description)| {
        ListItem::new(Line::from(vec![
            Span::styled(format!(" {command:<16}"), Style::default().fg(Color::Cyan)),
            Span::styled(*description, Style::default().fg(Color::Gray)),
        ]))
    });
    let mut state = ListState::default().with_selected(Some(
        app.command_index.min(suggestions.len().saturating_sub(1)),
    ));
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("›")
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .block(
                Block::default()
                    .title(" Commands ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
        area,
        &mut state,
    );
}

fn markdown_lines(markdown: &str) -> Vec<Line<'static>> {
    tui_markdown::from_str(markdown)
        .lines
        .into_iter()
        .map(|line| {
            let style = line.style;
            let spans = line
                .spans
                .into_iter()
                .map(|span| Span::styled(span.content.into_owned(), span.style))
                .collect::<Vec<_>>();
            Line::from(spans).style(style)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::input::Input;

    #[test]
    fn input_wraps_by_cell_and_places_the_cursor() {
        let mut input = Input::default();
        input.set("abcdef\n深圳x");
        let (rows, row, column) = wrap_input(&input, 4);
        assert_eq!(rows, ["abcd", "ef", "深圳", "x"]);
        assert_eq!((row, column), (3, 1));
        input.cursor = 7; // just after the newline
        let (_, row, column) = wrap_input(&input, 4);
        assert_eq!((row, column), (2, 0));
        input.set("abcd");
        assert_eq!(
            wrap_input(&input, 4),
            (vec!["abcd".into(), String::new()], 1, 0)
        );
    }
}
