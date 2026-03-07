use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::{RenameFeatureState, RenameSessionState, SessionConfigState};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_rename_session_dialog(frame: &mut Frame, state: &RenameSessionState, theme: &Theme) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Rename Session ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Name: ", Style::default().fg(theme.primary.to_color())),
        Span::styled(&state.input, Style::default().fg(theme.text.to_color())),
        Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color())),
    ]));
    frame.render_widget(name_field, chunks[0]);
}

pub fn draw_rename_feature_dialog(frame: &mut Frame, state: &RenameFeatureState, theme: &Theme) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Rename Feature ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Nickname: ", Style::default().fg(theme.primary.to_color())),
        Span::styled(&state.input, Style::default().fg(theme.text.to_color())),
        Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color())),
    ]));
    frame.render_widget(name_field, chunks[0]);
}

pub fn draw_session_config_dialog(frame: &mut Frame, state: &SessionConfigState, theme: &Theme) {
    let area = centered_rect(50, 35, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Session Config ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Project: ", Style::default().fg(theme.primary.to_color())),
            Span::styled(
                &state.project_name,
                Style::default().fg(theme.text.to_color()),
            ),
        ]),
        Line::from(vec![
            Span::styled(" Feature: ", Style::default().fg(theme.primary.to_color())),
            Span::styled(
                &state.feature_name,
                Style::default().fg(theme.text.to_color()),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Agent Type",
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        )),
    ];

    for (index, agent) in state.allowed_agents.iter().enumerate() {
        let marker = if *agent == state.current_agent {
            " (current)"
        } else {
            ""
        };
        let style = if index == state.selected_agent {
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else if *agent == state.current_agent {
            Style::default().fg(theme.primary.to_color())
        } else {
            Style::default().fg(theme.text.to_color())
        };

        lines.push(Line::from(Span::styled(
            format!("  {}{}", agent.display_name(), marker),
            style,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(" apply  ", Style::default().fg(theme.text.to_color())),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text.to_color())),
    ]));

    frame.render_widget(Paragraph::new(lines), inner);
}
