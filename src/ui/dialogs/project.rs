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

pub fn draw_create_project_dialog(
    frame: &mut Frame,
    state: &CreateProjectState,
    allowed_agents: &[AgentKind],
    message: Option<&str>,
    theme: &Theme,
) {
    let area = crate::ui::dialog_rect(frame.area(), 90, 20);
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
            Constraint::Length(allowed_agents.len() as u16 + 1),
            Constraint::Min(1),
            Constraint::Length(3),
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
    let path_field = Paragraph::new(Line::from(path_spans)).wrap(Wrap { trim: false });
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

    let mut message_area = chunks[3];
    if state.path_is_git == Some(false) {
        // The message (git init result, errors) outranks the warning.
        let message_height = message.map_or(0, |m| {
            Paragraph::new(m.to_string())
                .wrap(Wrap { trim: false })
                .line_count(message_area.width) as u16
        });
        let warning_height = message_area.height.saturating_sub(message_height).min(5);
        let warning = vec![
            Line::from(Span::styled(
                " This folder is not a git repository.",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "   Still works: agent and terminal sessions in the folder.",
                Style::default().fg(theme.text_muted.to_color()),
            )),
            Line::from(Span::styled(
                "   Needs git: worktrees, branches, diffs, GitHub issues and PRs.",
                Style::default().fg(theme.text_muted.to_color()),
            )),
            Line::from(vec![
                Span::styled("   Ctrl+G", Style::default().fg(theme.warning.to_color())),
                Span::raw(" runs git init here, or "),
                Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
                Span::raw(" on the last field to continue without git."),
            ]),
        ];
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(warning_height), Constraint::Min(0)])
            .split(message_area);
        frame.render_widget(Paragraph::new(warning).wrap(Wrap { trim: false }), split[0]);
        message_area = split[1];
    }
    if let Some(message) = message {
        let style = if message.starts_with("Error") || message.starts_with("git init failed") {
            Style::default().fg(theme.danger.to_color())
        } else {
            Style::default().fg(theme.text.to_color())
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(style)
                .wrap(Wrap { trim: false }),
            message_area,
        );
    }

    let enter_label = if matches!(state.step, CreateProjectStep::Agent) {
        " confirm  "
    } else {
        " next  "
    };
    let mut hints = vec![
        Span::styled(
            " Tab/Shift+Tab",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::raw(" fields  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(enter_label),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ];
    match state.step {
        CreateProjectStep::Path => {
            hints.push(Span::styled(
                "  Ctrl+B",
                Style::default().fg(theme.warning.to_color()),
            ));
            hints.push(Span::raw(" browse"));
        }
        CreateProjectStep::Agent => {
            hints.push(Span::styled(
                "  ↑/↓ or j/k",
                Style::default().fg(theme.warning.to_color()),
            ));
            hints.push(Span::raw(" choose harness"));
        }
        CreateProjectStep::Name => {}
    }
    if state.path_is_git == Some(false) {
        hints.push(Span::styled(
            "  Ctrl+G",
            Style::default().fg(theme.warning.to_color()),
        ));
        hints.push(Span::raw(" git init"));
    }
    let hints = Paragraph::new(Line::from(hints)).wrap(Wrap { trim: false });
    frame.render_widget(hints, chunks[4]);
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
    let width = frame.area().width.min(76);
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
    .wrap(Wrap { trim: false });
    let height = text.line_count(width.saturating_sub(2)).saturating_add(2);
    let area = crate::ui::dialog_rect(frame.area(), width, height.min(u16::MAX as usize) as u16);
    crate::ui::draw_modal_overlay(frame, area, theme);
    let text = text.block(
        Block::default()
            .title(" Confirm Delete ")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.danger.to_color())),
    );

    frame.render_widget(text, area);
}
