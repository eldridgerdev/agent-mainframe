use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, AppMode, Selection, VisibleItem};
use crate::project::ProjectStatus;

pub fn draw(frame: &mut Frame, app: &App) {
    // Viewing mode gets its own full-screen layout
    if let AppMode::Viewing(view) = &app.mode {
        draw_pane_view(
            frame,
            view,
            &app.pane_content,
            app.leader_active,
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(5),   // main content
            Constraint::Length(3), // status bar
        ])
        .split(frame.area());

    draw_header(frame, chunks[0]);
    draw_project_list(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);

    // Draw dialog overlays
    match &app.mode {
        AppMode::CreatingProject(state) => {
            draw_create_project_dialog(frame, state);
        }
        AppMode::CreatingFeature(state) => {
            draw_create_feature_dialog(frame, state);
        }
        AppMode::DeletingProject(name) => {
            draw_delete_project_confirm(frame, name);
        }
        AppMode::DeletingFeature(project_name, feature_name) => {
            draw_delete_feature_confirm(
                frame,
                project_name,
                feature_name,
            );
        }
        _ => {}
    }

    // Draw help overlay
    if matches!(app.mode, AppMode::Help) {
        draw_help(frame);
    }
}

fn draw_header(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Left side: title
    let title = Paragraph::new(Line::from(vec![
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
    ]));
    frame.render_widget(title, inner);

    // Right side: help hint
    let help_hint = Line::from(vec![
        Span::styled("h", Style::default().fg(Color::Yellow)),
        Span::styled(" help ", Style::default().fg(Color::DarkGray)),
    ]);
    let hint_width: u16 = help_hint.spans.iter()
        .map(|s| s.content.len() as u16)
        .sum();
    let hint_area = Rect {
        x: inner.x + inner.width.saturating_sub(hint_width),
        y: inner.y,
        width: hint_width,
        height: 1,
    };
    let hint = Paragraph::new(help_hint);
    frame.render_widget(hint, hint_area);
}

fn draw_project_list(frame: &mut Frame, app: &App, area: Rect) {
    if app.store.projects.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled(
                "No projects yet. Press ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "N",
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

    let visible = app.visible_items();

    let items: Vec<ListItem> = visible
        .iter()
        .map(|item| {
            let is_selected = match (&app.selection, item) {
                (
                    Selection::Project(a),
                    VisibleItem::Project(b),
                ) => a == b,
                (
                    Selection::Feature(a1, a2),
                    VisibleItem::Feature(b1, b2),
                ) => a1 == b1 && a2 == b2,
                _ => false,
            };

            let line = match item {
                VisibleItem::Project(pi) => {
                    let project = &app.store.projects[*pi];
                    let collapse_icon = if project.collapsed {
                        ">"
                    } else {
                        "v"
                    };

                    let mut spans = vec![
                        Span::styled(
                            format!(" {} ", collapse_icon),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            &project.name,
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(
                                "  {}",
                                project.repo.display()
                            ),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ];

                    if project.features.is_empty() {
                        spans.push(Span::styled(
                            "  (press n to add a feature)",
                            Style::default().fg(Color::DarkGray),
                        ));
                    }

                    Line::from(spans)
                }
                VisibleItem::Feature(pi, fi) => {
                    let feature =
                        &app.store.projects[*pi].features[*fi];

                    let status_dot = match feature.status {
                        ProjectStatus::Active => Span::styled(
                            "   ● ",
                            Style::default().fg(Color::Green),
                        ),
                        ProjectStatus::Idle => Span::styled(
                            "   ○ ",
                            Style::default().fg(Color::Yellow),
                        ),
                        ProjectStatus::Stopped => Span::styled(
                            "   ■ ",
                            Style::default().fg(Color::Red),
                        ),
                    };

                    let name_style = if is_selected {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    Line::from(vec![
                        status_dot,
                        Span::styled(&feature.name, name_style),
                        Span::styled(
                            format!(
                                "  {}",
                                feature.workdir.display()
                            ),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ])
                }
            };

            if is_selected {
                ListItem::new(line)
                    .style(Style::default().bg(Color::DarkGray))
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
        AppMode::Normal => {
            let on_feature =
                matches!(app.selection, Selection::Feature(_, _));
            if on_feature {
                Line::from(vec![
                    Span::styled(
                        " n",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" feature  "),
                    Span::styled(
                        "N",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" project  "),
                    Span::styled(
                        "Enter",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" view  "),
                    Span::styled(
                        "c",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" start  "),
                    Span::styled(
                        "x",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" stop  "),
                    Span::styled(
                        "s",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" switch  "),
                    Span::styled(
                        "d",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" delete  "),
                    Span::styled(
                        "q",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" quit"),
                ])
            } else {
                Line::from(vec![
                    Span::styled(
                        " n",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" feature  "),
                    Span::styled(
                        "N",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" project  "),
                    Span::styled(
                        "Enter",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" expand  "),
                    Span::styled(
                        "d",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" delete  "),
                    Span::styled(
                        "r",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" refresh  "),
                    Span::styled(
                        "q",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" quit"),
                ])
            }
        }
        AppMode::CreatingProject(_) | AppMode::CreatingFeature(_) => {
            Line::from(vec![
                Span::styled(
                    "Enter",
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" confirm  "),
                Span::styled(
                    "Esc",
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" cancel"),
            ])
        }
        AppMode::DeletingProject(_)
        | AppMode::DeletingFeature(_, _) => Line::from(vec![
            Span::styled(
                "y",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" confirm  "),
            Span::styled(
                "n/Esc",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" cancel"),
        ]),
        AppMode::Help => Line::from(vec![
            Span::styled("Esc/q/h", Style::default().fg(Color::Yellow)),
            Span::raw(" close help"),
        ]),
        AppMode::Viewing(_) => Line::from(vec![
            Span::styled(
                "Ctrl+Space",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" commands  "),
            Span::styled(
                "Ctrl+Q",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" exit view"),
        ]),
    };

    let message_line = if let Some(msg) = &app.message {
        Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Green),
        ))
    } else {
        let project_count = app.store.projects.len();
        let feature_count: usize = app
            .store
            .projects
            .iter()
            .map(|p| p.features.len())
            .sum();
        Line::from(Span::styled(
            format!(
                " {} project{}, {} feature{}",
                project_count,
                if project_count == 1 { "" } else { "s" },
                feature_count,
                if feature_count == 1 { "" } else { "s" },
            ),
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

fn draw_create_project_dialog(
    frame: &mut Frame,
    state: &crate::app::CreateProjectState,
) {
    use crate::app::CreateProjectStep;

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
        ])
        .split(inner);

    let name_style = match state.step {
        CreateProjectStep::Name => Style::default().fg(Color::Cyan),
        _ => Style::default().fg(Color::DarkGray),
    };
    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Name: ", name_style),
        Span::styled(
            &state.name,
            Style::default().fg(Color::White),
        ),
        cursor_span_project(&state.step, &CreateProjectStep::Name),
    ]));
    frame.render_widget(name_field, chunks[0]);

    let path_style = match state.step {
        CreateProjectStep::Path => Style::default().fg(Color::Cyan),
        _ => Style::default().fg(Color::DarkGray),
    };
    let path_field = Paragraph::new(Line::from(vec![
        Span::styled(" Repo path: ", path_style),
        Span::styled(
            &state.path,
            Style::default().fg(Color::White),
        ),
        cursor_span_project(&state.step, &CreateProjectStep::Path),
    ]));
    frame.render_widget(path_field, chunks[1]);
}

fn draw_create_feature_dialog(
    frame: &mut Frame,
    state: &crate::app::CreateFeatureState,
) {
    let area = centered_rect(60, 25, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" New Feature ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(inner);

    let branch_field = Paragraph::new(Line::from(vec![
        Span::styled(
            " Branch: ",
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            &state.branch,
            Style::default().fg(Color::White),
        ),
        Span::styled("█", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(branch_field, chunks[0]);
}

fn draw_delete_project_confirm(frame: &mut Frame, name: &str) {
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
                    .add_modifier(Modifier::BOLD),
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
            Span::styled("y", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to confirm, "),
            Span::styled("n", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" or "),
            Span::styled("Esc", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
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

fn draw_delete_feature_confirm(
    frame: &mut Frame,
    project_name: &str,
    feature_name: &str,
) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" Delete feature "),
            Span::styled(
                feature_name,
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" from "),
            Span::styled(
                project_name,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " This will kill the tmux session and remove the worktree.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw(" Press "),
            Span::styled("y", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to confirm, "),
            Span::styled("n", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" or "),
            Span::styled("Esc", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
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

fn draw_help(frame: &mut Frame) {
    let area = centered_rect(50, 60, frame.area());
    frame.render_widget(Clear, area);

    let keybinds: Vec<(&str, &str)> = vec![
        ("j/k / ↑/↓", "Navigate projects"),
        ("Enter", "View project (embedded tmux)"),
        ("s", "Switch to project (tmux attach)"),
        ("t", "Open terminal window"),
        ("n", "Create new project"),
        ("d", "Delete project"),
        ("x", "Stop project session"),
        ("r", "Refresh statuses"),
        ("h", "Toggle this help"),
        ("q / Esc", "Quit"),
    ];

    let mut lines: Vec<Line> = vec![Line::from("")];
    for (key, desc) in &keybinds {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>12}", key),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(*desc, Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  While viewing (embedded tmux):",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "Ctrl+Q"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Exit view", Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "Ctrl+Space"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Leader key (then: q t s n p r x h)",
            Style::default().fg(Color::White),
        ),
    ]));

    let help = Paragraph::new(lines).block(
        Block::default()
            .title(" Keybindings ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(help, area);
}

fn cursor_span_project<'a>(
    current: &crate::app::CreateProjectStep,
    target: &crate::app::CreateProjectStep,
) -> Span<'a> {
    use crate::app::CreateProjectStep;
    let is_active = matches!(
        (current, target),
        (CreateProjectStep::Name, CreateProjectStep::Name)
            | (CreateProjectStep::Path, CreateProjectStep::Path)
    );
    if is_active {
        Span::styled("█", Style::default().fg(Color::Cyan))
    } else {
        Span::raw("")
    }
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    area: Rect,
) -> Rect {
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

fn draw_pane_view(
    frame: &mut Frame,
    view: &crate::app::ViewState,
    pane_content: &str,
    leader_active: bool,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(1),   // pane content
        ])
        .split(frame.area());

    // Header bar with project/feature info and escape hint
    let mut header_spans = vec![
        Span::styled(
            format!(" {} ", view.project_name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("/ {} ", view.feature_name),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("| {} ", view.window),
            Style::default().fg(Color::DarkGray),
        ),
    ];

    if leader_active {
        header_spans.push(Span::styled(
            "| LEADER ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            " q:exit t:terminal n/p:cycle s:attach x:stop h:help",
            Style::default().fg(Color::Yellow),
        ));
    } else {
        header_spans.push(Span::styled(
            "| ",
            Style::default().fg(Color::DarkGray),
        ));
        header_spans.push(Span::styled(
            "Ctrl+Space",
            Style::default().fg(Color::Yellow),
        ));
        header_spans.push(Span::styled(
            " command palette",
            Style::default().fg(Color::DarkGray),
        ));
    }

    let border_color = if leader_active {
        Color::Yellow
    } else {
        Color::Cyan
    };

    let header = Paragraph::new(Line::from(header_spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)),
    );
    frame.render_widget(header, chunks[0]);

    // Parse ANSI content through vt100 and render
    let content_area = chunks[1];
    let text = ansi_to_ratatui_text(
        pane_content,
        content_area.width,
        content_area.height,
    );
    let paragraph = Paragraph::new(text);
    frame.render_widget(paragraph, content_area);
}

fn ansi_to_ratatui_text<'a>(
    raw: &str,
    cols: u16,
    rows: u16,
) -> Vec<Line<'a>> {
    let mut parser = vt100::Parser::new(rows, cols, 0);
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
