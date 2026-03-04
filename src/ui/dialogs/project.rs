use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::{CreateProjectState, CreateProjectStep};

use super::super::dashboard::centered_rect;

pub fn draw_create_project_dialog(frame: &mut Frame, state: &CreateProjectState) {
    let area = centered_rect(60, 30, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" New Project ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let name_style = match state.step {
        CreateProjectStep::Name => Style::default().fg(Color::Cyan),
        _ => Style::default().fg(Color::DarkGray),
    };
    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Name: ", name_style),
        Span::styled(&state.name, Style::default().fg(Color::White)),
        cursor_span_project(&state.step, &CreateProjectStep::Name),
    ]));
    frame.render_widget(name_field, chunks[0]);

    let path_style = match state.step {
        CreateProjectStep::Path => Style::default().fg(Color::Cyan),
        _ => Style::default().fg(Color::DarkGray),
    };
    let path_spans = vec![
        Span::styled(" Repo path: ", path_style),
        Span::styled(&state.path, Style::default().fg(Color::White)),
        cursor_span_project(&state.step, &CreateProjectStep::Path),
        Span::styled("  (Ctrl+B browse)", Style::default().fg(Color::DarkGray)),
    ];
    let path_field = Paragraph::new(Line::from(path_spans));
    frame.render_widget(path_field, chunks[1]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" Tab", Style::default().fg(Color::Yellow)),
        Span::raw(" switch field  "),
        Span::styled("Ctrl+B", Style::default().fg(Color::Yellow)),
        Span::raw(" browse  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[3]);
}

fn cursor_span_project<'a>(current: &CreateProjectStep, target: &CreateProjectStep) -> Span<'a> {
    let is_active = matches!(
        (current, target),
        (CreateProjectStep::Name, CreateProjectStep::Name)
            | (CreateProjectStep::Path, CreateProjectStep::Path)
    );
    if is_active {
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan))
    } else {
        Span::raw("")
    }
}

pub fn draw_delete_project_confirm(frame: &mut Frame, name: &str) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" Delete project "),
            Span::styled(
                name,
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " All features will be destroyed.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            " Tmux sessions will be killed and worktrees removed.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw(" Press "),
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::raw(" to confirm, "),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::raw(" or "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
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
            .border_style(Style::default().fg(Color::Red)),
    );

    frame.render_widget(text, area);
}
