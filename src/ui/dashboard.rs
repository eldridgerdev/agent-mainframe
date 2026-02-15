use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph,
        Wrap,
    },
    Frame,
};

use std::path::Path;

use crate::app::{
    App, AppMode, BrowsePathState, CommandPickerState,
    CreateFeatureStep, PendingInput, RenameReturnTo,
    RenameSessionState, Selection, SessionSwitcherState,
    VisibleItem,
};
use crate::project::{ProjectStatus, SessionKind, VibeMode};


const RAINBOW_COLORS: &[Color] = &[
    Color::Red,
    Color::Rgb(255, 127, 0), // orange
    Color::Yellow,
    Color::Green,
    Color::Cyan,
    Color::Blue,
    Color::Magenta,
];

fn rainbow_spans(text: &str) -> Vec<Span<'static>> {
    text.chars()
        .enumerate()
        .map(|(i, ch)| {
            let color = RAINBOW_COLORS[i % RAINBOW_COLORS.len()];
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(color)
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

fn shorten_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

/// Lighter gray readable on DarkGray background.
const SELECTED_GRAY: Color = Color::Rgb(140, 140, 140);

pub fn draw(frame: &mut Frame, app: &App) {
    // Viewing mode gets its own full-screen layout
    if let AppMode::Viewing(view) = &app.mode {
        draw_pane_view(
            frame,
            view,
            &app.pane_content,
            app.pane_cursor,
            app.leader_active,
            app.pending_inputs.len(),
        );
        return;
    }

    // Session switcher overlays on top of pane view
    if let AppMode::SessionSwitcher(state) = &app.mode {
        // Draw the pane view underneath (reconstruct a
        // temporary ViewState from switcher state)
        let temp_view = crate::app::ViewState {
            project_name: state.project_name.clone(),
            feature_name: state.feature_name.clone(),
            session: state.tmux_session.clone(),
            window: state.return_window.clone(),
            session_label: state.return_label.clone(),
            vibe_mode: state.vibe_mode.clone(),
        };
        draw_pane_view(
            frame,
            &temp_view,
            &app.pane_content,
            app.pane_cursor,
            false,
            app.pending_inputs.len(),
        );
        draw_session_switcher(
            frame,
            state,
            app.config.nerd_font,
        );
        return;
    }

    // Command picker overlays on top of pane view when
    // opened from view mode
    if let AppMode::CommandPicker(state) = &app.mode
        && state.from_view.is_some()
    {
        let view = state.from_view.as_ref().unwrap();
        draw_pane_view(
            frame,
            view,
            &app.pane_content,
            app.pane_cursor,
            false,
            app.pending_inputs.len(),
        );
        draw_command_picker(frame, state);
        return;
    }

    // Rename dialog from session switcher overlays on pane
    if let AppMode::RenamingSession(state) = &app.mode
        && let RenameReturnTo::SessionSwitcher(ref sw) =
            state.return_to
    {
        let temp_view = crate::app::ViewState {
            project_name: sw.project_name.clone(),
            feature_name: sw.feature_name.clone(),
            session: sw.tmux_session.clone(),
            window: sw.return_window.clone(),
            session_label: sw.return_label.clone(),
            vibe_mode: sw.vibe_mode.clone(),
        };
        draw_pane_view(
            frame,
            &temp_view,
            &app.pane_content,
            app.pane_cursor,
            false,
            app.pending_inputs.len(),
        );
        draw_rename_session_dialog(frame, state);
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

    draw_header(frame, chunks[0], app.pending_inputs.len());
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
        AppMode::DeletingFeature(
            project_name,
            feature_name,
        ) => {
            draw_delete_feature_confirm(
                frame,
                project_name,
                feature_name,
            );
        }
        AppMode::BrowsingPath(state) => {
            draw_browse_path_dialog(frame, state);
        }
        _ => {}
    }

    // Draw rename session dialog overlay
    if let AppMode::RenamingSession(state) = &app.mode {
        draw_rename_session_dialog(frame, state);
    }

    // Draw help overlay
    if matches!(app.mode, AppMode::Help) {
        draw_help(frame);
    }

    // Draw notification picker overlay
    if let AppMode::NotificationPicker(selected) = &app.mode
    {
        draw_notification_picker(
            frame,
            &app.pending_inputs,
            *selected,
        );
    }

    // Draw command picker overlay
    if let AppMode::CommandPicker(state) = &app.mode {
        draw_command_picker(frame, state);
    }
}

fn utilization_color(pct: f64) -> Color {
    if pct >= 80.0 {
        Color::Red
    } else if pct >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn draw_header(
    frame: &mut Frame,
    area: Rect,
    pending_count: usize,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Left side: title + optional notification badge
    let mut title_spans = vec![
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
    ];

    if pending_count > 0 {
        title_spans.push(Span::styled(
            format!(
                "  [{} input request{}]",
                pending_count,
                if pending_count == 1 { "" } else { "s" },
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let title = Paragraph::new(Line::from(title_spans));
    frame.render_widget(title, inner);

    // Right side: help hint
    let help_hint = Line::from(vec![
        Span::styled(
            "?",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            " help ",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    let hint_width: u16 = help_hint
        .spans
        .iter()
        .map(|s| s.content.len() as u16)
        .sum();
    let hint_area = Rect {
        x: inner
            .x
            .saturating_add(inner.width.saturating_sub(hint_width)),
        y: inner.y,
        width: hint_width,
        height: 1,
    };
    let hint = Paragraph::new(help_hint);
    frame.render_widget(hint, hint_area);
}

fn draw_project_list(
    frame: &mut Frame,
    app: &App,
    area: Rect,
) {
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
            let is_selected =
                match (&app.selection, item) {
                    (
                        Selection::Project(a),
                        VisibleItem::Project(b),
                    ) => a == b,
                    (
                        Selection::Feature(a1, a2),
                        VisibleItem::Feature(b1, b2),
                    ) => a1 == b1 && a2 == b2,
                    (
                        Selection::Session(a1, a2, a3),
                        VisibleItem::Session(b1, b2, b3),
                    ) => a1 == b1 && a2 == b2 && a3 == b3,
                    _ => false,
                };

            // Color for secondary/muted text that must
            // remain readable on DarkGray selection bg.
            let muted = if is_selected {
                SELECTED_GRAY
            } else {
                Color::DarkGray
            };

            let line = match item {
                VisibleItem::Project(pi) => {
                    let project =
                        &app.store.projects[*pi];
                    let collapse_icon =
                        if project.collapsed {
                            ">"
                        } else {
                            "v"
                        };

                    let mut spans = vec![
                        Span::styled(
                            format!(
                                " {} ",
                                collapse_icon
                            ),
                            Style::default().fg(muted),
                        ),
                        Span::styled(
                            &project.name,
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(
                                    Modifier::BOLD,
                                ),
                        ),
                        Span::styled(
                            format!(
                                "  {}",
                                shorten_path(&project.repo)
                            ),
                            Style::default().fg(muted),
                        ),
                    ];

                    if project.features.is_empty() {
                        spans.push(Span::styled(
                            "  (press n to add a feature)",
                            Style::default().fg(muted),
                        ));
                    }

                    Line::from(spans)
                }
                VisibleItem::Feature(pi, fi) => {
                    let project =
                        &app.store.projects[*pi];
                    let feature = &project.features[*fi];
                    let is_last_feature =
                        *fi == project.features.len() - 1;

                    let connector = if is_last_feature {
                        "  └─"
                    } else {
                        "  ├─"
                    };

                    let status_dot = match feature.status {
                        ProjectStatus::Active => {
                            Span::styled(
                                " ● ",
                                Style::default()
                                    .fg(Color::Green),
                            )
                        }
                        ProjectStatus::Idle => {
                            Span::styled(
                                " ○ ",
                                Style::default()
                                    .fg(Color::Yellow),
                            )
                        }
                        ProjectStatus::Stopped => {
                            Span::styled(
                                " ■ ",
                                Style::default()
                                    .fg(Color::Red),
                            )
                        }
                    };

                    let collapse_icon =
                        if feature.sessions.is_empty() {
                            " "
                        } else if feature.collapsed {
                            ">"
                        } else {
                            "v"
                        };

                    let name_style = if is_selected {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    let session_count =
                        feature.sessions.len();
                    let badge = if session_count > 0 {
                        format!(" [{}]", session_count)
                    } else {
                        String::new()
                    };

                    let mode_badge_spans: Vec<Span> = match feature.mode {
                        VibeMode::Vibeless => vec![Span::styled(
                            " [vibeless]",
                            Style::default()
                                .fg(Color::Green),
                        )],
                        VibeMode::Vibe => vec![Span::styled(
                            " [vibe]",
                            Style::default()
                                .fg(Color::Yellow),
                        )],
                        VibeMode::SuperVibe => {
                            let mut spans = vec![Span::raw(" [")];
                            spans.extend(rainbow_spans("supervibe"));
                            spans.push(Span::raw("]"));
                            spans
                        }
                    };

                    let mut line_spans = vec![
                        Span::styled(
                            connector,
                            Style::default().fg(muted),
                        ),
                        status_dot,
                        Span::styled(
                            format!("{} ", collapse_icon),
                            Style::default().fg(muted),
                        ),
                        Span::styled(
                            &feature.name,
                            name_style,
                        ),
                    ];
                    line_spans.extend(mode_badge_spans);
                    line_spans.push(Span::styled(
                        badge,
                        Style::default().fg(muted),
                    ));
                    line_spans.push(Span::styled(
                        format!(
                            "  {}",
                            shorten_path(&feature.workdir)
                        ),
                        Style::default().fg(muted),
                    ));
                    Line::from(line_spans)
                }
                VisibleItem::Session(pi, fi, si) => {
                    let project =
                        &app.store.projects[*pi];
                    let feature = &project.features[*fi];
                    let session =
                        &feature.sessions[*si];

                    let is_last_feature =
                        *fi == project.features.len() - 1;
                    let is_last_session =
                        *si == feature.sessions.len() - 1;

                    let vert = if is_last_feature {
                        "  "
                    } else {
                        "  │"
                    };
                    let branch = if is_last_session {
                        "   └─ "
                    } else {
                        "   ├─ "
                    };

                    let kind_icon = match session.kind {
                        SessionKind::Claude => {
                            Span::styled(
                                "* ",
                                Style::default()
                                    .fg(Color::Magenta),
                            )
                        }
                        SessionKind::Terminal => {
                            Span::styled(
                                "> ",
                                Style::default()
                                    .fg(Color::Green),
                            )
                        }
                        SessionKind::Nvim => {
                            let icon =
                                if app.config.nerd_font {
                                    "\u{E62B} "
                                } else {
                                    "~ "
                                };
                            Span::styled(
                                icon,
                                Style::default()
                                    .fg(Color::Cyan),
                            )
                        }
                    };

                    let name_style = if is_selected {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    Line::from(vec![
                        Span::styled(
                            vert,
                            Style::default().fg(muted),
                        ),
                        Span::styled(
                            branch,
                            Style::default().fg(muted),
                        ),
                        kind_icon,
                        Span::styled(
                            &session.label,
                            name_style,
                        ),
                    ])
                }
            };

            if is_selected {
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

fn draw_status_bar(
    frame: &mut Frame,
    app: &App,
    area: Rect,
) {
    let keybinds = match &app.mode {
        AppMode::Normal => {
            let on_session = matches!(
                app.selection,
                Selection::Session(_, _, _)
            );
            let on_feature = matches!(
                app.selection,
                Selection::Feature(_, _)
            );
            if on_session {
                Line::from(vec![
                    Span::styled(
                        " Enter",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" view  "),
                    Span::styled(
                        "r",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" rename  "),
                    Span::styled(
                        "x",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" remove  "),
                    Span::styled(
                        "d",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" delete  "),
                    Span::styled(
                        "s",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" switch  "),
                    Span::styled(
                        "q",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" quit"),
                ])
            } else if on_feature {
                Line::from(vec![
                    Span::styled(
                        " n",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" feature  "),
                    Span::styled(
                        "Enter",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" expand  "),
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
                        "t",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" +term  "),
                    Span::styled(
                        "a",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" +claude  "),
                    Span::styled(
                        "v",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" +nvim  "),
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
                        "R",
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
        AppMode::CreatingProject(_)
        | AppMode::CreatingFeature(_)
        | AppMode::RenamingSession(_)
        | AppMode::BrowsingPath(_) => Line::from(vec![
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
        ]),
        AppMode::DeletingProject(_)
        | AppMode::DeletingFeature(_, _) => {
            Line::from(vec![
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
            ])
        }
        AppMode::Help => Line::from(vec![
            Span::styled(
                "Esc/q/?",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" close help"),
        ]),
        AppMode::CommandPicker(_)
        | AppMode::NotificationPicker(_)
        | AppMode::SessionSwitcher(_) => Line::from(vec![
            Span::styled(
                "j/k",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" navigate  "),
            Span::styled(
                "Enter",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" select  "),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" cancel"),
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
        let color = if msg.starts_with("Error:") {
            Color::Red
        } else {
            Color::Green
        };
        Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(color),
        ))
    } else {
        match &app.selection {
            Selection::Project(pi)
                if *pi < app.store.projects.len() =>
            {
                let project = &app.store.projects[*pi];
                Line::from(vec![
                    Span::styled(
                        format!(" {}", project.name),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "  {}",
                            shorten_path(&project.repo)
                        ),
                        Style::default()
                            .fg(Color::DarkGray),
                    ),
                ])
            }
            Selection::Feature(pi, fi)
                if *pi < app.store.projects.len()
                    && *fi
                        < app.store.projects[*pi]
                            .features
                            .len() =>
            {
                let feature =
                    &app.store.projects[*pi].features[*fi];
                let branch_info =
                    if feature.branch.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", feature.branch)
                    };
                Line::from(vec![
                    Span::styled(
                        format!(" {}", feature.name),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        branch_info,
                        Style::default()
                            .fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!(
                            "  {}",
                            shorten_path(&feature.workdir)
                        ),
                        Style::default()
                            .fg(Color::DarkGray),
                    ),
                ])
            }
            Selection::Session(pi, fi, si)
                if *pi < app.store.projects.len()
                    && *fi
                        < app.store.projects[*pi]
                            .features
                            .len()
                    && *si
                        < app.store.projects[*pi]
                            .features[*fi]
                            .sessions
                            .len() =>
            {
                let feature =
                    &app.store.projects[*pi].features[*fi];
                let session = &feature.sessions[*si];
                let kind_label = match session.kind {
                    SessionKind::Claude => "claude",
                    SessionKind::Terminal => "terminal",
                    SessionKind::Nvim => "nvim",
                };
                Line::from(vec![
                    Span::styled(
                        format!(
                            " {} ({})",
                            session.label, kind_label
                        ),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}", feature.name),
                        Style::default()
                            .fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!(
                            "  {}",
                            shorten_path(&feature.workdir)
                        ),
                        Style::default()
                            .fg(Color::DarkGray),
                    ),
                ])
            }
            _ => {
                let project_count =
                    app.store.projects.len();
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
                        if project_count == 1 {
                            ""
                        } else {
                            "s"
                        },
                        feature_count,
                        if feature_count == 1 {
                            ""
                        } else {
                            "s"
                        },
                    ),
                    Style::default().fg(Color::DarkGray),
                ))
            }
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(
            Style::default().fg(Color::DarkGray),
        );
    let inner = block.inner(area);

    let status =
        Paragraph::new(vec![message_line, keybinds])
            .block(block);
    frame.render_widget(status, area);

    // Usage stats on the right side of the top line
    let usage = app.usage.get_data();
    let mut right_spans: Vec<Span> = Vec::new();

    if let Some(pct5) = usage.five_hour_pct {
        right_spans.push(Span::styled(
            "5h: ",
            Style::default().fg(Color::DarkGray),
        ));
        right_spans.push(Span::styled(
            format!("{:.0}%", pct5),
            Style::default()
                .fg(utilization_color(pct5))
                .add_modifier(Modifier::BOLD),
        ));
        right_spans.push(Span::styled(
            " | ",
            Style::default().fg(Color::DarkGray),
        ));
    }

    if let Some(pct7) = usage.seven_day_pct {
        right_spans.push(Span::styled(
            "7d: ",
            Style::default().fg(Color::DarkGray),
        ));
        right_spans.push(Span::styled(
            format!("{:.0}%", pct7),
            Style::default()
                .fg(utilization_color(pct7))
                .add_modifier(Modifier::BOLD),
        ));
        right_spans.push(Span::styled(
            " | ",
            Style::default().fg(Color::DarkGray),
        ));
    } else if let Some(ref err) = usage.last_error {
        right_spans.push(Span::styled(
            format!("{} ", err),
            Style::default().fg(Color::Red),
        ));
        right_spans.push(Span::styled(
            "| ",
            Style::default().fg(Color::DarkGray),
        ));
    }

    right_spans.push(Span::styled(
        format!("{} msgs today ", usage.today_messages),
        Style::default().fg(Color::DarkGray),
    ));

    let right_width: u16 = right_spans
        .iter()
        .map(|s| s.content.len() as u16)
        .sum();
    let right_area = Rect {
        x: inner
            .x
            .saturating_add(inner.width.saturating_sub(right_width)),
        y: inner.y,
        width: right_width,
        height: 1,
    };
    let right = Paragraph::new(Line::from(right_spans));
    frame.render_widget(right, right_area);
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
            Constraint::Length(1),
        ])
        .split(inner);

    let name_style = match state.step {
        CreateProjectStep::Name => {
            Style::default().fg(Color::Cyan)
        }
        _ => Style::default().fg(Color::DarkGray),
    };
    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Name: ", name_style),
        Span::styled(
            &state.name,
            Style::default().fg(Color::White),
        ),
        cursor_span_project(
            &state.step,
            &CreateProjectStep::Name,
        ),
    ]));
    frame.render_widget(name_field, chunks[0]);

    let path_style = match state.step {
        CreateProjectStep::Path => {
            Style::default().fg(Color::Cyan)
        }
        _ => Style::default().fg(Color::DarkGray),
    };
    let path_spans = vec![
        Span::styled(" Repo path: ", path_style),
        Span::styled(
            &state.path,
            Style::default().fg(Color::White),
        ),
        cursor_span_project(
            &state.step,
            &CreateProjectStep::Path,
        ),
        Span::styled(
            "  (Ctrl+B browse)",
            Style::default().fg(Color::DarkGray),
        ),
    ];
    let path_field = Paragraph::new(Line::from(path_spans));
    frame.render_widget(path_field, chunks[1]);

    // Key hints at bottom of dialog
    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            " Tab",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" switch field  "),
        Span::styled(
            "Ctrl+B",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" browse  "),
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
    ]));
    frame.render_widget(hints, chunks[3]);
}

fn draw_create_feature_dialog(
    frame: &mut Frame,
    state: &crate::app::CreateFeatureState,
) {
    if state.step == CreateFeatureStep::ConfirmSuperVibe {
        draw_confirm_supervibe_dialog(frame);
        return;
    }

    match state.step {
        CreateFeatureStep::Source => {
            draw_create_feature_source(frame, state);
        }
        CreateFeatureStep::ExistingWorktree => {
            draw_create_feature_worktree_picker(
                frame, state,
            );
        }
        _ => {
            draw_create_feature_branch_mode(frame, state);
        }
    }
}

fn draw_create_feature_source(
    frame: &mut Frame,
    state: &crate::app::CreateFeatureState,
) {
    let area = centered_rect(60, 30, frame.area());
    frame.render_widget(Clear, area);

    let title =
        format!(" New Feature ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // label
            Constraint::Length(2), // options
            Constraint::Min(0),   // fill
            Constraint::Length(1), // hints
        ])
        .split(inner);

    let label = Paragraph::new(Line::from(Span::styled(
        " Source:",
        Style::default().fg(Color::Cyan),
    )));
    frame.render_widget(label, chunks[0]);

    let options = ["New branch", "Existing worktree"];
    let mut lines = Vec::new();
    for (i, opt) in options.iter().enumerate() {
        let is_selected = i == state.source_index;
        let marker = if is_selected { ">" } else { " " };
        let style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        lines.push(Line::from(Span::styled(
            format!("   {} {}", marker, opt),
            style,
        )));
    }
    let options_widget = Paragraph::new(lines);
    frame.render_widget(options_widget, chunks[1]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            " j/k",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" select  "),
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
    ]));
    frame.render_widget(hints, chunks[3]);
}

fn draw_create_feature_worktree_picker(
    frame: &mut Frame,
    state: &crate::app::CreateFeatureState,
) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let title =
        format!(" Select Worktree ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),   // worktree list
            Constraint::Length(1), // hints
        ])
        .split(inner);

    let items: Vec<ListItem> = state
        .worktrees
        .iter()
        .enumerate()
        .map(|(i, wt)| {
            let is_selected = i == state.worktree_index;
            let branch_label = wt
                .branch
                .as_deref()
                .unwrap_or("(detached)");
            let path_str = wt.path.display().to_string();

            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "  > " } else { "    " },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    branch_label,
                    if is_selected {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
                Span::styled(
                    format!("  {}", path_str),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            if is_selected {
                ListItem::new(line).style(
                    Style::default().bg(Color::DarkGray),
                )
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            " j/k",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" navigate  "),
        Span::styled(
            "Enter",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" select  "),
        Span::styled(
            "Esc",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[1]);
}

fn draw_create_feature_branch_mode(
    frame: &mut Frame,
    state: &crate::app::CreateFeatureState,
) {
    let area = centered_rect(60, 45, frame.area());
    frame.render_widget(Clear, area);

    let title =
        format!(" New Feature ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // branch
            Constraint::Length(1), // spacer
            Constraint::Length(4), // worktree toggle
            Constraint::Length(1), // spacer
            Constraint::Length(5), // mode selection
            Constraint::Length(1), // spacer
            Constraint::Length(2), // notes toggle
            Constraint::Min(0),   // fill
            Constraint::Length(1), // hints
        ])
        .split(inner);

    let branch_active =
        state.step == CreateFeatureStep::Branch;
    let branch_label_style = if branch_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let cursor = if branch_active {
        Span::styled(
            "\u{2588}",
            Style::default().fg(Color::Cyan),
        )
    } else {
        Span::raw("")
    };

    let branch_field = Paragraph::new(Line::from(vec![
        Span::styled(" Branch: ", branch_label_style),
        Span::styled(
            &state.branch,
            Style::default().fg(Color::White),
        ),
        cursor,
    ]));
    frame.render_widget(branch_field, chunks[0]);

    // Worktree toggle
    let wt_active =
        state.step == CreateFeatureStep::Worktree;
    let wt_label_style = if wt_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let yes_marker =
        if state.use_worktree { ">" } else { " " };
    let no_marker =
        if !state.use_worktree { ">" } else { " " };

    let yes_style = if wt_active && state.use_worktree {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if state.use_worktree {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let no_style = if wt_active && !state.use_worktree {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if !state.use_worktree {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let wt_lines = vec![
        Line::from(Span::styled(
            " Worktree:",
            wt_label_style,
        )),
        Line::from(Span::styled(
            format!("   {} Yes", yes_marker),
            yes_style,
        )),
        Line::from(Span::styled(
            format!("   {} No (use repo dir)", no_marker),
            no_style,
        )),
    ];
    let wt_widget = Paragraph::new(wt_lines);
    frame.render_widget(wt_widget, chunks[2]);

    // Mode selection
    let mode_active =
        state.step == CreateFeatureStep::Mode;
    let mode_label_style = if mode_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let mut mode_lines =
        vec![Line::from(Span::styled(
            " Mode:",
            mode_label_style,
        ))];

    for (i, m) in VibeMode::ALL.iter().enumerate() {
        let is_selected = i == state.mode_index;
        let marker = if is_selected { ">" } else { " " };
        let style = if mode_active && is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        mode_lines.push(Line::from(Span::styled(
            format!("   {} {}", marker, m.display_name()),
            style,
        )));
    }

    let mode_widget = Paragraph::new(mode_lines);
    frame.render_widget(mode_widget, chunks[4]);

    // Notes toggle
    let notes_check = if state.enable_notes {
        "[x]"
    } else {
        "[ ]"
    };
    let notes_style = if mode_active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let notes_lines = vec![Line::from(vec![
        Span::styled(
            " Memo: ",
            if mode_active {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::styled(
            format!("{} Create memo", notes_check),
            notes_style,
        ),
    ])];
    let notes_widget = Paragraph::new(notes_lines);
    frame.render_widget(notes_widget, chunks[6]);

    // Hints at bottom
    let hints = if mode_active || wt_active {
        Paragraph::new(Line::from(vec![
            Span::styled(
                " j/k",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" select  "),
            Span::styled(
                "Tab",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" memo  "),
            Span::styled(
                "Enter",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" confirm  "),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" back"),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Enter",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" next  "),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" cancel"),
        ]))
    };
    frame.render_widget(hints, chunks[8]);
}

fn draw_confirm_supervibe_dialog(frame: &mut Frame) {
    let area = centered_rect(60, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" SuperVibe Mode ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // warning header
            Constraint::Length(1), // spacer
            Constraint::Min(4),   // description
            Constraint::Length(1), // spacer
            Constraint::Length(1), // prompt
            Constraint::Length(1), // hints
        ])
        .split(inner);

    let warning = Paragraph::new(Line::from(vec![
        Span::styled(
            " WARNING",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(warning, chunks[0]);

    let desc = Paragraph::new(vec![
        Line::from(Span::styled(
            " SuperVibe skips ALL permission checks.",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            " Claude will be able to execute any tool",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            " without asking for confirmation, including",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            " running arbitrary shell commands.",
            Style::default().fg(Color::White),
        )),
    ])
    .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(desc, chunks[2]);

    let prompt = Paragraph::new(Line::from(vec![
        Span::styled(
            " Continue? ",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            "(y/n)",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(prompt, chunks[4]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            " y",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" confirm  "),
        Span::styled(
            "n/Esc",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[5]);
}

fn draw_rename_session_dialog(
    frame: &mut Frame,
    state: &RenameSessionState,
) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Rename Session ")
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

    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(
            " Name: ",
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            &state.input,
            Style::default().fg(Color::White),
        ),
        Span::styled(
            "\u{2588}",
            Style::default().fg(Color::Cyan),
        ),
    ]));
    frame.render_widget(name_field, chunks[0]);
}

fn draw_delete_project_confirm(
    frame: &mut Frame,
    name: &str,
) {
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
    let area = centered_rect(55, 70, frame.area());
    frame.render_widget(Clear, area);

    let keybinds: Vec<(&str, &str)> = vec![
        ("j/k / \u{2191}/\u{2193}", "Navigate up/down"),
        ("h", "Collapse project / go to parent"),
        ("l", "Expand project / view feature"),
        ("Enter", "Toggle expand / view session"),
        ("s", "Switch to tmux session"),
        ("N", "Create new project"),
        ("n", "Create new feature"),
        ("d", "Delete project/feature/session"),
        ("c", "Start feature (create tmux)"),
        ("x", "Stop feature / remove session"),
        ("r", "Rename session"),
        ("t", "Add terminal session"),
        ("a", "Add Claude session"),
        ("v", "Add nvim session"),
        ("m", "Create memo (.claude/notes.md)"),
        ("i", "Input requests picker"),
        ("R", "Refresh statuses"),
        ("?", "Toggle this help"),
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
            Span::styled(
                *desc,
                Style::default().fg(Color::White),
            ),
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
        Span::styled(
            "Exit view",
            Style::default().fg(Color::White),
        ),
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
            "Leader key (then: q t T w / s n p i r x ?)",
            Style::default().fg(Color::White),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "t / T"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Cycle next/prev session",
            Style::default().fg(Color::White),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "w"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Session switcher",
            Style::default().fg(Color::White),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "/"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Custom commands picker",
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
            | (
                CreateProjectStep::Path,
                CreateProjectStep::Path
            )
    );
    if is_active {
        Span::styled(
            "\u{2588}",
            Style::default().fg(Color::Cyan),
        )
    } else {
        Span::raw("")
    }
}

fn draw_notification_picker(
    frame: &mut Frame,
    pending: &[PendingInput],
    selected: usize,
) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let title =
        format!(" Input Requests ({}) ", pending.len());
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

            let proj = input
                .project_name
                .as_deref()
                .unwrap_or("unknown");
            let feat = input
                .feature_name
                .as_deref()
                .unwrap_or("unknown");

            // Truncate message for preview
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
                Span::styled(
                    format!("/ {} ", feat),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("- {}", msg_preview),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            if is_selected {
                ListItem::new(line).style(
                    Style::default().bg(Color::DarkGray),
                )
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, inner);
}

fn draw_command_picker(
    frame: &mut Frame,
    state: &CommandPickerState,
) {
    let area = centered_rect(50, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(
        " Custom Commands ({}) ",
        state.commands.len()
    );
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
        .constraints([
            Constraint::Min(1),    // command list
            Constraint::Length(2), // footer hints
        ])
        .split(inner);

    let mut items: Vec<ListItem> = Vec::new();
    let mut current_source = String::new();

    for (i, cmd) in state.commands.iter().enumerate() {
        // Add section header when source changes
        if cmd.source != current_source {
            if !current_source.is_empty() {
                items.push(ListItem::new(Line::from("")));
            }
            current_source = cmd.source.clone();
            items.push(ListItem::new(Line::from(
                Span::styled(
                    format!("  {} Commands", cmd.source),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            )));
        }

        let is_selected = i == state.selected;
        let line = Line::from(vec![
            Span::styled(
                "    /",
                Style::default().fg(Color::DarkGray),
            ),
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
            items.push(ListItem::new(line).style(
                Style::default().bg(Color::DarkGray),
            ));
        } else {
            items.push(ListItem::new(line));
        }
    }

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "Enter",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            " send  ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "Esc",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            " cancel",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(hints, chunks[1]);
}

fn draw_session_switcher(
    frame: &mut Frame,
    state: &SessionSwitcherState,
    nerd_font: bool,
) {
    let area = centered_rect(40, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(
        " {} / {} ",
        state.project_name, state.feature_name
    );
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

    // Session list area + footer hint
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // session list
            Constraint::Length(2), // footer hints
        ])
        .split(inner);

    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = i == state.selected;
            let is_current =
                entry.tmux_window == state.return_window;

            let icon = match entry.kind {
                SessionKind::Claude => Span::styled(
                    "  * ",
                    Style::default().fg(Color::Magenta),
                ),
                SessionKind::Terminal => Span::styled(
                    "  > ",
                    Style::default().fg(Color::Green),
                ),
                SessionKind::Nvim => {
                    let icon = if nerd_font {
                        "  \u{E62B} "
                    } else {
                        "  ~ "
                    };
                    Span::styled(
                        icon,
                        Style::default().fg(Color::Cyan),
                    )
                }
            };

            let name_style = if is_selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let mut spans = vec![
                icon,
                Span::styled(&entry.label, name_style),
            ];

            if is_current {
                spans.push(Span::styled(
                    " (current)",
                    Style::default().fg(Color::DarkGray),
                ));
            }

            let line = Line::from(spans);
            if is_selected {
                ListItem::new(line).style(
                    Style::default().bg(Color::DarkGray),
                )
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    // Footer hints
    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "Enter",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            " select  ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "r",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            " rename  ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "Esc",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            " cancel",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(hints, chunks[1]);
}

fn draw_browse_path_dialog(
    frame: &mut Frame,
    state: &BrowsePathState,
) {
    let area = centered_rect(80, 70, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Browse for Directory ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // cwd display
            Constraint::Min(3),   // explorer widget
            Constraint::Length(2), // key hints
        ])
        .split(inner);

    // Current directory path
    let cwd_line = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            state
                .explorer
                .cwd()
                .to_string_lossy()
                .to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(cwd_line, chunks[0]);

    // File explorer widget
    frame.render_widget(&state.explorer.widget(), chunks[1]);

    // Key hints with separator
    let hints = Paragraph::new(vec![
        Line::from(Span::styled(
            "\u{2500}".repeat(inner.width as usize),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(vec![
            Span::styled(
                " Space",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" select  "),
            Span::styled(
                "Enter/l",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" open  "),
            Span::styled(
                "h/BS",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" parent  "),
            Span::styled(
                "Tab",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" name  "),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" cancel"),
        ]),
    ]);
    frame.render_widget(hints, chunks[2]);
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
    pane_cursor: Option<(u16, u16)>,
    leader_active: bool,
    pending_count: usize,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(1),   // pane content
        ])
        .split(frame.area());

    // Header bar with project/feature/session info
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
            format!("/ {} ", view.session_label),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    match view.vibe_mode {
        VibeMode::Vibeless => header_spans.push(Span::styled(
            "[vibeless] ",
            Style::default().fg(Color::Green),
        )),
        VibeMode::Vibe => header_spans.push(Span::styled(
            "[vibe] ",
            Style::default().fg(Color::Yellow),
        )),
        VibeMode::SuperVibe => {
            header_spans.push(Span::raw("["));
            header_spans.extend(rainbow_spans("supervibe"));
            header_spans.push(Span::raw("] "));
        }
    };

    if leader_active {
        header_spans.push(Span::styled(
            "| LEADER ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            " q:exit t/T:cycle w:switcher n/p:feature /:commands i:inputs s:attach x:stop ?:help",
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

    if pending_count > 0 {
        header_spans.push(Span::styled(
            format!(
                " | {} input{}",
                pending_count,
                if pending_count == 1 { "" } else { "s" },
            ),
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let border_color = if leader_active {
        Color::Yellow
    } else {
        Color::Cyan
    };

    let header =
        Paragraph::new(Line::from(header_spans)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(
                    Style::default().fg(border_color),
                ),
        );
    frame.render_widget(header, chunks[0]);

    // Parse ANSI content through vt100 and render
    let content_area = chunks[1];
    let text = ansi_to_ratatui_text(
        pane_content,
        content_area.width,
        content_area.height,
        pane_cursor,
    );
    let paragraph = Paragraph::new(text);
    frame.render_widget(paragraph, content_area);
}

fn ansi_to_ratatui_text<'a>(
    raw: &str,
    cols: u16,
    rows: u16,
    cursor: Option<(u16, u16)>,
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

            let is_cursor = cursor
                == Some((col, row));
            let style = if is_cursor {
                vt100_cell_to_style(cell)
                    .add_modifier(Modifier::REVERSED)
            } else {
                vt100_cell_to_style(cell)
            };

            if style != current_style
                && !current_text.is_empty()
            {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            current_style = style;
            current_text.push_str(&cell.contents());
        }

        if !current_text.is_empty() {
            spans.push(Span::styled(
                current_text,
                current_style,
            ));
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
