use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use crate::app::{CommandPickerState, PendingInput, SessionSwitcherState};
use crate::project::SessionKind;

use super::dashboard::centered_rect;

pub fn draw_notification_picker(frame: &mut Frame, pending: &[PendingInput], selected: usize) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" Input Requests ({}) ", pending.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if pending.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No pending input requests.",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let items: Vec<ListItem> = pending
        .iter()
        .enumerate()
        .map(|(i, input)| {
            let is_selected = i == selected;

            let proj = input.project_name.as_deref().unwrap_or("unknown");
            let feat = input.feature_name.as_deref().unwrap_or("unknown");

            let msg_preview = if input.message.len() > 50 {
                format!("{}...", &input.message[..47])
            } else if input.message.is_empty() {
                input.notification_type.clone()
            } else {
                input.message.clone()
            };

            let line = Line::from(vec![
                Span::styled(
                    format!("  {} ", proj),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("/ {} ", feat), Style::default().fg(Color::White)),
                Span::styled(
                    format!("- {}", msg_preview),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            if is_selected {
                ListItem::new(line).style(Style::default().bg(Color::DarkGray))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, inner);
}

pub fn draw_command_picker(frame: &mut Frame, state: &CommandPickerState) {
    let area = centered_rect(50, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" Custom Commands ({}) ", state.commands.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.commands.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No custom commands found.",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    let mut items: Vec<ListItem> = Vec::new();
    let mut current_source = String::new();

    for (i, cmd) in state.commands.iter().enumerate() {
        if cmd.source != current_source {
            if !current_source.is_empty() {
                items.push(ListItem::new(Line::from("")));
            }
            current_source = cmd.source.clone();
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  {} Commands", cmd.source),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))));
        }

        let is_selected = i == state.selected;
        let line = Line::from(vec![
            Span::styled("    /", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &cmd.name,
                if is_selected {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ),
        ]);

        if is_selected {
            items.push(ListItem::new(line).style(Style::default().bg(Color::DarkGray)));
        } else {
            items.push(ListItem::new(line));
        }
    }

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled("  j/k", Style::default().fg(Color::Yellow)),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::styled(" send  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_session_switcher(frame: &mut Frame, state: &SessionSwitcherState, nerd_font: bool) {
    let area = centered_rect(40, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" {} / {} ", state.project_name, state.feature_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.sessions.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No sessions.",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = i == state.selected;
            let is_current = entry.tmux_window == state.return_window;

            let icon = match entry.kind {
                SessionKind::Claude => Span::styled("  * ", Style::default().fg(Color::Magenta)),
                SessionKind::Opencode => Span::styled("  * ", Style::default().fg(Color::Cyan)),
                SessionKind::Terminal => Span::styled("  > ", Style::default().fg(Color::Green)),
                SessionKind::Nvim => {
                    let icon = if nerd_font { "  \u{E62B} " } else { "  ~ " };
                    Span::styled(icon, Style::default().fg(Color::Cyan))
                }
            };

            let name_style = if is_selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let mut spans = vec![icon, Span::styled(&entry.label, name_style)];

            if is_current {
                spans.push(Span::styled(
                    " (current)",
                    Style::default().fg(Color::DarkGray),
                ));
            }

            let line = Line::from(spans);
            if is_selected {
                ListItem::new(line).style(Style::default().bg(Color::DarkGray))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled("  j/k", Style::default().fg(Color::Yellow)),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
        Span::styled("r", Style::default().fg(Color::Yellow)),
        Span::styled(" rename  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(hints, chunks[1]);
}
