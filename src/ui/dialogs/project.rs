use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::{CreateProjectState, CreateProjectStep};
use crate::project::AgentKind;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_create_project_dialog(
    frame: &mut Frame,
    state: &CreateProjectState,
    allowed_agents: &[AgentKind],
    theme: &Theme,
) {
    let area = centered_rect(60, 40, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" New Project ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(6),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let name_style = match state.step {
        CreateProjectStep::Name => Style::default().fg(theme.primary.to_color()),
        _ => Style::default().fg(theme.text_muted.to_color()),
    };
    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Name: ", name_style),
        Span::styled(&state.name, Style::default().fg(theme.text.to_color())),
        cursor_span_project(&state.step, &CreateProjectStep::Name, theme),
    ]));
    frame.render_widget(name_field, chunks[0]);

    let path_style = match state.step {
        CreateProjectStep::Path => Style::default().fg(theme.primary.to_color()),
        _ => Style::default().fg(theme.text_muted.to_color()),
    };
    let path_spans = vec![
        Span::styled(" Repo path: ", path_style),
        Span::styled(&state.path, Style::default().fg(theme.text.to_color())),
        cursor_span_project(&state.step, &CreateProjectStep::Path, theme),
        Span::styled(
            "  (Ctrl+B browse)",
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ];
    let path_field = Paragraph::new(Line::from(path_spans));
    frame.render_widget(path_field, chunks[1]);

    let agent_active = matches!(state.step, CreateProjectStep::Agent);
    let mut agent_lines = vec![Line::from(Span::styled(
        " Preferred harness:",
        if agent_active {
            Style::default().fg(theme.primary.to_color())
        } else {
            Style::default().fg(theme.text_muted.to_color())
        },
    ))];
    for (index, agent) in allowed_agents.iter().enumerate() {
        let is_selected = index == state.agent_index;
        let marker = if is_selected { ">" } else { " " };
        let style = if agent_active && is_selected {
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };
        agent_lines.push(Line::from(Span::styled(
            format!("   {} {}", marker, agent.display_name()),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(agent_lines), chunks[2]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" Tab", Style::default().fg(theme.warning.to_color())),
        Span::raw(" switch field  "),
        Span::styled("Ctrl+B", Style::default().fg(theme.warning.to_color())),
        Span::raw(" browse  "),
        Span::styled("j/k", Style::default().fg(theme.warning.to_color())),
        Span::raw(" choose agent  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[3]);
}

fn cursor_span_project<'a>(
    current: &CreateProjectStep,
    target: &CreateProjectStep,
    theme: &Theme,
) -> Span<'a> {
    let is_active = matches!(
        (current, target),
        (CreateProjectStep::Name, CreateProjectStep::Name)
            | (CreateProjectStep::Path, CreateProjectStep::Path)
    );
    if is_active {
        Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color()))
    } else {
        Span::raw("")
    }
}

pub fn draw_delete_project_confirm(frame: &mut Frame, name: &str, theme: &Theme) {
    let area = centered_rect(50, 25, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" Delete project "),
            Span::styled(
                name,
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " All features will be destroyed.",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            " Tmux sessions will be killed and worktrees removed.",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw(" Press "),
            Span::styled(
                "y",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::raw(" to confirm, "),
            Span::styled(
                "n",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::raw(" or "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::raw(" to cancel"),
        ]),
    ])
    .wrap(Wrap { trim: false })
    .block(
        Block::default()
            .title(" Confirm Delete ")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.danger.to_color())),
    );

    frame.render_widget(text, area);
}
