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
    // Viewing mode gets its own full-screen layout
    if let AppMode::Viewing(view) = &app.mode {
        draw_pane_view(frame, view, &app.pane_content);
        return;
    }

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
            " Agent Mainframe ",
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
            Span::raw(" view  "),
            Span::styled("s", Style::default().fg(Color::Yellow)),
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
        AppMode::Viewing(_) => Line::from(vec![
            Span::styled("Ctrl+Q", Style::default().fg(Color::Yellow)),
            Span::raw(" exit view"),
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

fn draw_pane_view(frame: &mut Frame, view: &crate::app::ViewState, pane_content: &str) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(1),   // pane content
        ])
        .split(frame.area());

    // Header bar with project info and escape hint
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", view.project_name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("| {} ", view.window),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "| Ctrl+Q to exit",
            Style::default().fg(Color::Yellow),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(header, chunks[0]);

    // Parse ANSI content through vt100 and render
    let content_area = chunks[1];
    let text = ansi_to_ratatui_text(pane_content, content_area.width, content_area.height);
    let paragraph = Paragraph::new(text);
    frame.render_widget(paragraph, content_area);
}

fn ansi_to_ratatui_text<'a>(raw: &str, cols: u16, rows: u16) -> Vec<Line<'a>> {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    // capture-pane uses \n line endings, but vt100 treats LF as cursor-down
    // only (no carriage return). Convert to \r\n so the parser resets to
    // column 0 on each new line.
    let normalized = raw.replace('\n', "\r\n");
    parser.process(normalized.as_bytes());
    let screen = parser.screen();

    let mut lines = Vec::with_capacity(rows as usize);

    for row in 0..rows {
        let mut spans: Vec<Span<'a>> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        for col in 0..cols {
            let cell = screen.cell(row, col);
            let cell = match cell {
                Some(c) => c,
                None => continue,
            };

            let style = vt100_cell_to_style(&cell);

            if style != current_style && !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            current_style = style;
            current_text.push_str(&cell.contents());
        }

        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }

        lines.push(Line::from(spans));
    }

    lines
}

fn vt100_cell_to_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    style = style.fg(vt100_color_to_ratatui(cell.fgcolor()));
    style = style.bg(vt100_color_to_ratatui(cell.bgcolor()));

    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
