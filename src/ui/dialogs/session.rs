use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{
    ProjectAgentConfigState, RenameFeatureState, RenameSessionState, SessionConfigState,
};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_rename_session_dialog(frame: &mut Frame, state: &RenameSessionState, theme: &Theme) {
    let area = centered_rect(50, 25, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

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
    crate::ui::draw_modal_overlay(frame, area, theme);

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
    let header_lines = vec![
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
    ];
    draw_agent_config_dialog(
        frame,
        " Session Config ",
        header_lines,
        " Agent Type",
        &state.current_agent,
        &state.allowed_agents,
        state.selected_agent,
        theme,
    );
}

pub fn draw_project_agent_config_dialog(
    frame: &mut Frame,
    state: &ProjectAgentConfigState,
    theme: &Theme,
) {
    let header_lines = vec![Line::from(vec![
        Span::styled(" Project: ", Style::default().fg(theme.primary.to_color())),
        Span::styled(
            &state.project_name,
            Style::default().fg(theme.text.to_color()),
        ),
    ])];
    draw_agent_config_dialog(
        frame,
        " Project Config ",
        header_lines,
        " Preferred Agent",
        &state.current_agent,
        &state.allowed_agents,
        state.selected_agent,
        theme,
    );
}

fn draw_agent_config_dialog(
    frame: &mut Frame,
    title: &str,
    header_lines: Vec<Line>,
    section_title: &str,
    current_agent: &crate::project::AgentKind,
    allowed_agents: &[crate::project::AgentKind],
    selected_agent: usize,
    theme: &Theme,
) {
    let area = centered_rect(50, 35, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = header_lines;
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        section_title,
        Style::default()
            .fg(theme.warning.to_color())
            .add_modifier(Modifier::BOLD),
    )));

    for (index, agent) in allowed_agents.iter().enumerate() {
        let marker = if *agent == *current_agent {
            " (current)"
        } else {
            ""
        };
        let style = if index == selected_agent {
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else if *agent == *current_agent {
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
