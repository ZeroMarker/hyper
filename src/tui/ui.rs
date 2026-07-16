use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use super::App;
use crate::AgentMode;

pub fn draw(frame: &mut Frame, app: &App) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(3),
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
        Span::styled("DeepSeek V4 Flash", Style::default().fg(Color::Magenta)),
        Span::raw("  •  "),
        Span::styled(app.mode.as_str(), Style::default().fg(mode_color)),
    ]);
    frame.render_widget(
        Paragraph::new(header)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::BOTTOM)),
        areas[0],
    );

    let mut chat = Vec::new();
    for message in &app.output {
        let mut lines = message.lines();
        let role = lines.next().unwrap_or("Hyper");
        let role_color = if role == "You" {
            Color::Cyan
        } else {
            Color::Green
        };
        chat.push(Line::styled(
            role,
            Style::default().fg(role_color).add_modifier(Modifier::BOLD),
        ));
        let markdown = lines.collect::<Vec<_>>().join("\n");
        chat.extend(markdown_lines(&markdown));
        chat.push(Line::from(""));
    }
    if app.busy {
        const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        chat.push(Line::from(vec![
            Span::styled(
                "Hyper ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} 正在思考", SPINNER[app.tick % SPINNER.len()]),
                Style::default().fg(Color::Magenta),
            ),
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

    let input_width = UnicodeWidthStr::width(app.input.as_str()) as u16;
    let visible_width = areas[2].width.saturating_sub(2).max(1);
    let horizontal_scroll = input_width.saturating_sub(visible_width.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(app.input.as_str())
            .scroll((0, horizontal_scroll))
            .block(
                Block::default()
                    .title(" Message ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(mode_color)),
            ),
        areas[2],
    );
    frame.set_cursor_position((
        areas[2].x + 1 + input_width.saturating_sub(horizontal_scroll),
        areas[2].y + 1,
    ));

    render_command_palette(frame, app, areas[2]);

    frame.render_widget(
        Paragraph::new(" ↑↓/PgUp/PgDn 滚动  End 最新  Tab 模式  Enter 发送  Esc 退出")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray)),
        areas[3],
    );
}

fn render_command_palette(frame: &mut Frame, app: &App, input_area: Rect) {
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
