use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, AppMode, CreateStep};
use crate::project::ProjectStatus;

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Min(5),    // main content
            Constraint::Length(3), // status bar
        ])
        .split(frame.area());

    draw_header(frame, chunks[0]);
    draw_project_list(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);

    // Draw create dialog overlay if in create mode
    if let AppMode::Creating(state) = &app.mode {
        draw_create_dialog(frame, state);
    }

    // Draw delete confirmation if in delete mode
    if let AppMode::Deleting(name) = &app.mode {
        draw_delete_confirm(frame, name);
    }
}

fn draw_header(frame: &mut Frame, area: Rect) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            " Claude Super Vibeless ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "| Multi-Project Agent Manager",
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(header, area);
}

fn draw_project_list(frame: &mut Frame, app: &App, area: Rect) {
    let projects = app.store.list();

    if projects.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled(
                "No projects yet. Press ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to create one.",
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(
            Block::default()
                .title(" Projects ")
                .borders(Borders::ALL),
        );
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = projects
        .iter()
        .enumerate()
        .map(|(i, project)| {
            let status_indicator = match project.status {
                ProjectStatus::Active => Span::styled(
                    " ● ",
                    Style::default().fg(Color::Green),
                ),
                ProjectStatus::Idle => Span::styled(
                    " ○ ",
                    Style::default().fg(Color::Yellow),
                ),
                ProjectStatus::Stopped => Span::styled(
                    " ■ ",
                    Style::default().fg(Color::Red),
                ),
            };

            let name = Span::styled(
                &project.name,
                if i == app.selected {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            );

            let branch_text = project
                .branch
                .as_deref()
                .unwrap_or("(no branch)");
            let branch = Span::styled(
                format!("  {}", branch_text),
                Style::default().fg(Color::DarkGray),
            );

            let path = Span::styled(
                format!("  {}", project.workdir.display()),
                Style::default().fg(Color::DarkGray),
            );

            let line = Line::from(vec![status_indicator, name, branch, path]);

            if i == app.selected {
                ListItem::new(line).style(
                    Style::default().bg(Color::DarkGray),
                )
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Projects ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::White)),
    );

    frame.render_widget(list, area);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let keybinds = match &app.mode {
        AppMode::Normal => Line::from(vec![
            Span::styled(" n", Style::default().fg(Color::Yellow)),
            Span::raw(" new  "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" switch  "),
            Span::styled("t", Style::default().fg(Color::Yellow)),
            Span::raw(" terminal  "),
            Span::styled("x", Style::default().fg(Color::Yellow)),
            Span::raw(" stop  "),
            Span::styled("d", Style::default().fg(Color::Yellow)),
            Span::raw(" delete  "),
            Span::styled("r", Style::default().fg(Color::Yellow)),
            Span::raw(" refresh  "),
            Span::styled("q", Style::default().fg(Color::Yellow)),
            Span::raw(" quit"),
        ]),
        AppMode::Creating(_) => Line::from(vec![
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" confirm  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ]),
        AppMode::Deleting(_) => Line::from(vec![
            Span::styled("y", Style::default().fg(Color::Yellow)),
            Span::raw(" confirm  "),
            Span::styled("n/Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ]),
    };

    let message_line = if let Some(msg) = &app.message {
        Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Green),
        ))
    } else {
        let count = app.project_count();
        Line::from(Span::styled(
            format!(" {} project{}", count, if count == 1 { "" } else { "s" }),
            Style::default().fg(Color::DarkGray),
        ))
    };

    let status = Paragraph::new(vec![message_line, keybinds]).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    frame.render_widget(status, area);
}

fn draw_create_dialog(frame: &mut Frame, state: &crate::app::CreateState) {
    let area = centered_rect(60, 40, frame.area());
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
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(inner);

    let name_style = match state.step {
        CreateStep::Name => Style::default().fg(Color::Cyan),
        _ => Style::default().fg(Color::DarkGray),
    };
    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Name: ", name_style),
        Span::styled(
            &state.name,
            Style::default().fg(Color::White),
        ),
        cursor_span(&state.step, &CreateStep::Name),
    ]));
    frame.render_widget(name_field, chunks[0]);

    let path_style = match state.step {
        CreateStep::Path => Style::default().fg(Color::Cyan),
        _ => Style::default().fg(Color::DarkGray),
    };
    let path_field = Paragraph::new(Line::from(vec![
        Span::styled(" Repo path: ", path_style),
        Span::styled(
            &state.path,
            Style::default().fg(Color::White),
        ),
        cursor_span(&state.step, &CreateStep::Path),
    ]));
    frame.render_widget(path_field, chunks[1]);

    let branch_style = match state.step {
        CreateStep::Branch => Style::default().fg(Color::Cyan),
        _ => Style::default().fg(Color::DarkGray),
    };
    let branch_field = Paragraph::new(Line::from(vec![
        Span::styled(" Branch (optional): ", branch_style),
        Span::styled(
            &state.branch,
            Style::default().fg(Color::White),
        ),
        cursor_span(&state.step, &CreateStep::Branch),
    ]));
    frame.render_widget(branch_field, chunks[2]);
}

fn draw_delete_confirm(frame: &mut Frame, name: &str) {
    let area = centered_rect(50, 20, frame.area());
    frame.render_widget(Clear, area);

    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" Delete project "),
            Span::styled(
                name,
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " This will kill the tmux session and remove the worktree.",
            Style::default().fg(Color::DarkGray),
        )),
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

fn cursor_span<'a>(current: &CreateStep, target: &CreateStep) -> Span<'a> {
    let is_active = matches!(
        (current, target),
        (CreateStep::Name, CreateStep::Name)
            | (CreateStep::Path, CreateStep::Path)
            | (CreateStep::Branch, CreateStep::Branch)
    );
    if is_active {
        Span::styled("█", Style::default().fg(Color::Cyan))
    } else {
        Span::raw("")
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
