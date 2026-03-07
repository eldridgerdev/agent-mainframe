use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{
    BrowsePathState, ChangeReasonState, CreateFeatureState, CreateFeatureStep, CreateProjectState,
    CreateProjectStep, DeleteStage, DeletingFeatureState, HookPromptState, RenameSessionState,
    RunningHookState, SearchState,
};
use crate::extension::FeaturePreset;
use crate::project::{AgentKind, VibeMode};

use super::dashboard::centered_rect;

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

pub fn draw_create_feature_dialog(
    frame: &mut Frame,
    state: &CreateFeatureState,
    presets: &[FeaturePreset],
) {
    match state.step {
        CreateFeatureStep::Source => {
            draw_create_feature_source(frame, state, presets);
        }
        CreateFeatureStep::ExistingWorktree => {
            draw_create_feature_worktree_picker(frame, state);
        }
        CreateFeatureStep::SelectPreset => {
            draw_create_feature_preset_picker(frame, state, presets);
        }
        _ => {
            draw_create_feature_branch_mode(frame, state);
        }
    }
}

fn draw_create_feature_source(
    frame: &mut Frame,
    state: &CreateFeatureState,
    presets: &[FeaturePreset],
) {
    let area = centered_rect(60, 30, frame.area());
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
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let label = Paragraph::new(Line::from(Span::styled(
        " Source:",
        Style::default().fg(Color::Cyan),
    )));
    frame.render_widget(label, chunks[0]);

    let mut options: Vec<&str> = vec!["New branch", "Existing worktree"];
    if !presets.is_empty() {
        options.push("Use preset");
    }
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
            " j/k or \u{2191}/\u{2193}",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" select  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[3]);
}

fn draw_create_feature_preset_picker(
    frame: &mut Frame,
    state: &CreateFeatureState,
    presets: &[FeaturePreset],
) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" Select Preset ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    if presets.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No presets configured.",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(empty, chunks[0]);
    } else {
        let items: Vec<ListItem> = presets
            .iter()
            .enumerate()
            .map(|(i, preset)| {
                let is_selected = i == state.preset_index;
                let name_style = if is_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let agent_str = preset.agent.display_name();
                let mode_str = match &preset.mode {
                    crate::project::VibeMode::Vibeless => "vibeless",
                    crate::project::VibeMode::Vibe => "vibe",
                    crate::project::VibeMode::SuperVibe => "supervibe",
                    crate::project::VibeMode::Review => "review",
                };
                let detail = format!(
                    " {} | {}{}",
                    agent_str,
                    mode_str,
                    if preset.review { " | review" } else { "" }
                );
                let line = Line::from(vec![
                    Span::styled(
                        if is_selected { "  > " } else { "    " },
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(&preset.name, name_style),
                    Span::styled(detail, Style::default().fg(Color::DarkGray)),
                ]);
                let item = ListItem::new(line);
                if is_selected {
                    item.style(Style::default().bg(Color::DarkGray))
                } else {
                    item
                }
            })
            .collect();
        let list = List::new(items);
        frame.render_widget(list, chunks[0]);
    }

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            " j/k or \u{2191}/\u{2193}",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" select  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" use preset  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[1]);
}

fn draw_create_feature_worktree_picker(frame: &mut Frame, state: &CreateFeatureState) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" Select Worktree ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    if state.worktrees.is_empty() {
        let empty_msg = Paragraph::new(Line::from(Span::styled(
            "  No available worktrees",
            Style::default().fg(Color::Yellow),
        )));
        frame.render_widget(empty_msg, chunks[0]);
    } else {
        let items: Vec<ListItem> = state
            .worktrees
            .iter()
            .enumerate()
            .map(|(i, wt)| {
                let is_selected = i == state.worktree_index;
                let branch_label = wt.branch.as_deref().unwrap_or("(detached)");
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
                    ListItem::new(line).style(Style::default().bg(Color::DarkGray))
                } else {
                    ListItem::new(line)
                }
            })
            .collect();

        let list = List::new(items);
        frame.render_widget(list, chunks[0]);
    }

    let hints = if state.worktrees.is_empty() {
        Paragraph::new(Line::from(vec![
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" back"),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(
                " j/k or \u{2191}/\u{2193}",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" navigate  "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" select  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" back"),
        ]))
    };
    frame.render_widget(hints, chunks[1]);
}

fn draw_create_feature_branch_mode(frame: &mut Frame, state: &CreateFeatureState) {
    let area = centered_rect(60, 70, frame.area());
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
            Constraint::Length(2), // branch
            Constraint::Length(1), // spacer
            Constraint::Length(4), // worktree
            Constraint::Length(1), // spacer
            Constraint::Length(4), // agent
            Constraint::Length(1), // spacer
            Constraint::Length(4), // mode (3 variants)
            Constraint::Length(1), // spacer
            Constraint::Length(2), // review checkbox
            Constraint::Length(1), // spacer
            Constraint::Length(2), // chrome checkbox
            Constraint::Length(1), // spacer
            Constraint::Length(2), // notes checkbox
            Constraint::Length(2), // extra space
            Constraint::Min(0),
            Constraint::Length(1), // hints
        ])
        .split(inner);

    let branch_active = state.step == CreateFeatureStep::Branch;
    let branch_label_style = if branch_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let cursor = if branch_active {
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan))
    } else {
        Span::raw("")
    };

    let branch_field = Paragraph::new(Line::from(vec![
        Span::styled(" Branch: ", branch_label_style),
        Span::styled(&state.branch, Style::default().fg(Color::White)),
        cursor,
    ]));
    frame.render_widget(branch_field, chunks[0]);

    let wt_active = state.step == CreateFeatureStep::Worktree;
    let wt_label_style = if wt_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let yes_marker = if state.use_worktree { ">" } else { " " };
    let no_marker = if !state.use_worktree { ">" } else { " " };

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
        Line::from(Span::styled(" Worktree:", wt_label_style)),
        Line::from(Span::styled(format!("   {} Yes", yes_marker), yes_style)),
        Line::from(Span::styled(
            format!("   {} No (use repo dir)", no_marker),
            no_style,
        )),
    ];
    let wt_widget = Paragraph::new(wt_lines);
    frame.render_widget(wt_widget, chunks[2]);

    let agent_active = state.step == CreateFeatureStep::Mode && state.mode_focus == 0;
    let agent_label_style = if agent_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let mut agent_lines = vec![Line::from(Span::styled(" Agent:", agent_label_style))];

    for (i, agent) in AgentKind::ALL.iter().enumerate() {
        let is_selected = i == state.agent_index;
        let marker = if is_selected { ">" } else { " " };
        let style = if agent_active && is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        agent_lines.push(Line::from(Span::styled(
            format!("   {} {}", marker, agent.display_name()),
            style,
        )));
    }

    let agent_widget = Paragraph::new(agent_lines);
    frame.render_widget(agent_widget, chunks[4]);

    let mode_active = state.step == CreateFeatureStep::Mode && state.mode_focus == 1;
    let mode_label_style = if mode_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let mut mode_lines = vec![Line::from(Span::styled(" Mode:", mode_label_style))];

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
    frame.render_widget(mode_widget, chunks[6]);

    // Review checkbox (chunks[8])
    let review_active = state.step == CreateFeatureStep::Mode && state.mode_focus == 2;
    let review_check = if state.review { "[x]" } else { "[ ]" };
    let review_style = if review_active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let review_lines = vec![Line::from(vec![
        Span::styled(
            " Review: ",
            if review_active {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::styled(
            format!("{} Approve each edit before apply", review_check),
            review_style,
        ),
    ])];
    let review_widget = Paragraph::new(review_lines);
    frame.render_widget(review_widget, chunks[8]);

    // Chrome checkbox (chunks[10])
    let chrome_active = state.step == CreateFeatureStep::Mode
        && state.mode_focus == 3
        && state.agent == AgentKind::Claude;
    let chrome_check = if state.enable_chrome { "[x]" } else { "[ ]" };
    let chrome_style = if chrome_active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let chrome_label_style =
        if state.step == CreateFeatureStep::Mode && state.agent == AgentKind::Claude {
            if state.mode_focus == 3 {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            }
        } else {
            Style::default().fg(Color::DarkGray)
        };

    if state.agent == AgentKind::Claude {
        let chrome_lines = vec![Line::from(vec![
            Span::styled(" Chrome: ", chrome_label_style),
            Span::styled(
                format!("{} Enable browser automation", chrome_check),
                chrome_style,
            ),
        ])];
        let chrome_widget = Paragraph::new(chrome_lines);
        frame.render_widget(chrome_widget, chunks[10]);
    }

    // Notes checkbox (chunks[12])
    let memo_focus = if state.agent == AgentKind::Claude {
        4
    } else {
        3
    };
    let notes_active = state.step == CreateFeatureStep::Mode && state.mode_focus == memo_focus;
    let notes_check = if state.enable_notes { "[x]" } else { "[ ]" };
    let notes_style = if notes_active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let notes_lines = vec![Line::from(vec![
        Span::styled(
            " Memo: ",
            if notes_active {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::styled(format!("{} Create memo", notes_check), notes_style),
    ])];
    let notes_widget = Paragraph::new(notes_lines);
    frame.render_widget(notes_widget, chunks[12]);

    let hints = if state.step == CreateFeatureStep::Mode {
        Paragraph::new(Line::from(vec![
            Span::styled(
                " j/k or \u{2191}/\u{2193}",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" select  "),
            Span::styled("h/l", Style::default().fg(Color::Yellow)),
            Span::raw(" prev/next field  "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" confirm"),
        ]))
    } else if state.step == CreateFeatureStep::Worktree {
        Paragraph::new(Line::from(vec![
            Span::styled(
                " j/k or \u{2191}/\u{2193}",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" toggle  "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" next  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" back"),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" next  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ]))
    };
    frame.render_widget(hints, chunks[15]);
}

pub fn draw_confirm_supervibe_dialog(frame: &mut Frame) {
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
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let warning = Paragraph::new(Line::from(vec![Span::styled(
        " WARNING",
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )]));
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
    .wrap(Wrap { trim: false });
    frame.render_widget(desc, chunks[2]);

    let prompt = Paragraph::new(Line::from(vec![
        Span::styled(" Continue? ", Style::default().fg(Color::Yellow)),
        Span::styled("(y/n)", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(prompt, chunks[4]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" y", Style::default().fg(Color::Yellow)),
        Span::raw(" confirm  "),
        Span::styled("n/Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[5]);
}

pub fn draw_rename_session_dialog(frame: &mut Frame, state: &RenameSessionState) {
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
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Name: ", Style::default().fg(Color::Cyan)),
        Span::styled(&state.input, Style::default().fg(Color::White)),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(name_field, chunks[0]);
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
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
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
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to confirm, "),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" or "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
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

pub fn draw_delete_feature_confirm(frame: &mut Frame, project_name: &str, feature_name: &str) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" Delete feature "),
            Span::styled(
                feature_name,
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
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
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to confirm, "),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" or "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
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

pub fn draw_help(frame: &mut Frame) {
    let area = centered_rect(55, 70, frame.area());
    draw_help_at(frame, area);
}

pub fn draw_help_bottom_right(frame: &mut Frame) {
    let viewport = frame.area();
    let width = (viewport.width.saturating_mul(55) / 100).max(40);
    let height = (viewport.height.saturating_mul(70) / 100).max(12);
    let area = Rect::new(
        viewport.x + viewport.width.saturating_sub(width + 1),
        viewport.y + viewport.height.saturating_sub(height + 1),
        width,
        height,
    );
    draw_help_at(frame, area);
}

fn draw_help_at(frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);

    let keybinds: Vec<(&str, &str)> = vec![
        ("j/k / \u{2191}/\u{2193}", "Navigate up/down"),
        ("h", "Collapse project / go to parent"),
        ("l", "Expand project / view feature"),
        ("Enter", "Toggle expand / view session"),
        ("s", "Switch to tmux session"),
        ("S", "Pick opencode session to resume"),
        ("N", "Create new project"),
        ("n", "Create new feature"),
        ("O", "Open AMF settings project"),
        ("d", "Delete project/feature/session"),
        ("c", "Start feature (create tmux)"),
        ("x", "Stop feature / remove session"),
        ("r", "Rename session"),
        ("t", "Add terminal session"),
        ("a", "Add Claude session"),
        ("v", "Add nvim session"),
        ("m", "Create memo (.claude/notes.md)"),
        ("i", "Input requests picker"),
        ("/", "Search and jump to item"),
        ("R", "Refresh statuses"),
        ("?", "Toggle this help"),
        ("q / Esc", "Quit"),
    ];

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                "  ESC",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " closes this menu",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];
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
            "Leader key (then: q t T w / s n p i r x f ?)",
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
        Span::styled("Cycle next/prev session", Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "w"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Session switcher", Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "/"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Custom commands picker", Style::default().fg(Color::White)),
    ]));

    let help = Paragraph::new(lines).block(
        Block::default()
            .title(" Keybindings ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(help, area);
}

pub fn draw_browse_path_dialog(frame: &mut Frame, state: &BrowsePathState) {
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
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    let cwd_line = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            state.explorer.cwd().to_string_lossy().to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(cwd_line, chunks[0]);

    if state.creating_folder {
        let input_area = chunks[1];
        let input_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(input_area);

        let prompt = Paragraph::new(Line::from(vec![
            Span::styled(" Create folder: ", Style::default().fg(Color::Yellow)),
            Span::styled(&state.new_folder_name, Style::default().fg(Color::White)),
            Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
        ]));
        frame.render_widget(prompt, input_chunks[0]);
    } else {
        frame.render_widget(&state.explorer.widget(), chunks[1]);
    }

    let hints = if state.creating_folder {
        Paragraph::new(vec![
            Line::from(Span::styled(
                "\u{2500}".repeat(inner.width as usize),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(vec![
                Span::styled(" Enter", Style::default().fg(Color::Yellow)),
                Span::raw(" create  "),
                Span::styled("Esc", Style::default().fg(Color::Yellow)),
                Span::raw(" cancel"),
            ]),
        ])
    } else {
        Paragraph::new(vec![
            Line::from(Span::styled(
                "\u{2500}".repeat(inner.width as usize),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(vec![
                Span::styled(" Space", Style::default().fg(Color::Yellow)),
                Span::raw(" select  "),
                Span::styled("Enter/l", Style::default().fg(Color::Yellow)),
                Span::raw(" open  "),
                Span::styled("c", Style::default().fg(Color::Yellow)),
                Span::raw(" create folder  "),
                Span::styled("h/BS", Style::default().fg(Color::Yellow)),
                Span::raw(" parent  "),
                Span::styled("Esc", Style::default().fg(Color::Yellow)),
                Span::raw(" cancel"),
            ]),
        ])
    };
    frame.render_widget(hints, chunks[2]);
}

pub fn draw_search_dialog(frame: &mut Frame, state: &SearchState) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    let query_line = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(&state.query, Style::default().fg(Color::White)),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(query_line, chunks[0]);

    if state.matches.is_empty() {
        if !state.query.is_empty() {
            let no_results = Paragraph::new(Line::from(Span::styled(
                " No matches found",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(no_results, chunks[1]);
        } else {
            let placeholder = Paragraph::new(Line::from(Span::styled(
                " Type to search projects, features, and sessions...",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(placeholder, chunks[1]);
        }
    } else {
        let visible_matches: Vec<ListItem> = state
            .matches
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let is_selected = i == state.selected_match;
                let marker = if is_selected { ">" } else { " " };
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let line = Line::from(vec![
                    Span::styled(format!(" {} ", marker), Style::default().fg(Color::Cyan)),
                    Span::styled(&m.label, style),
                    Span::styled(
                        format!("  {}", m.context),
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

        let list = List::new(visible_matches);
        frame.render_widget(list, chunks[1]);
    }

    let count_text = if state.matches.is_empty() {
        String::new()
    } else {
        format!(" {} / {}", state.selected_match + 1, state.matches.len())
    };
    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            " j/k or \u{2191}/\u{2193}",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" jump  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" cancel"),
        Span::raw("  "),
        Span::styled(count_text, Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(hints, chunks[2]);
}

pub fn draw_change_reason_dialog(frame: &mut Frame, state: &ChangeReasonState) {
    let area = centered_rect(80, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Review change ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // file
            Constraint::Length(1), // tool
            Constraint::Length(1), // separator
            Constraint::Length(6), // diff
            Constraint::Length(1), // separator
            Constraint::Length(2), // reason
            Constraint::Length(1), // hints
        ])
        .split(inner);

    let file_line = Paragraph::new(Line::from(vec![
        Span::styled(" File: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            &state.relative_path,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(file_line, chunks[0]);

    let tool_label = if state.tool == "edit" {
        "EDIT"
    } else {
        "WRITE"
    };
    let tool_line = Paragraph::new(Line::from(vec![
        Span::styled(" Tool: ", Style::default().fg(Color::DarkGray)),
        Span::styled(tool_label, Style::default().fg(Color::Yellow)),
    ]));
    frame.render_widget(tool_line, chunks[1]);

    let mut diff_lines = vec![];

    // Show old content (removed)
    if !state.old_snippet.is_empty() {
        diff_lines.push(Line::from(Span::styled(
            " Removed:",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        for line in state.old_snippet.lines().take(3) {
            let truncated = if line.len() > 70 { &line[..70] } else { line };
            diff_lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(truncated, Style::default().fg(Color::Red)),
            ]));
        }
        if state.old_snippet.lines().count() > 3 {
            diff_lines.push(Line::from(Span::styled(
                "  ...",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    // Show new content (added)
    diff_lines.push(Line::from(""));
    diff_lines.push(Line::from(Span::styled(
        " Added:",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )));

    let new_preview: String = state
        .new_snippet
        .lines()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let truncated = if new_preview.len() > 60 {
        format!("{}...", &new_preview[..57])
    } else {
        new_preview
    };
    diff_lines.push(Line::from(vec![
        Span::styled(" + ", Style::default().fg(Color::Green)),
        Span::styled(truncated, Style::default().fg(Color::Green)),
    ]));

    let diff_widget = Paragraph::new(diff_lines);
    frame.render_widget(diff_widget, chunks[2]);

    let reason_line = Paragraph::new(Line::from(vec![
        Span::styled(" Reason: ", Style::default().fg(Color::Green)),
        Span::styled(&state.reason, Style::default().fg(Color::White)),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(reason_line, chunks[3]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" accept  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" skip  "),
        Span::styled("r", Style::default().fg(Color::Red)),
        Span::raw(" reject"),
    ]));
    frame.render_widget(hints, chunks[5]);
}

pub fn draw_running_hook_dialog(
    frame: &mut Frame,
    state: &RunningHookState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
) {
    let area = centered_rect(90, 70, frame.area());
    frame.render_widget(Clear, area);

    let is_running = state.child.is_some();
    let border_color = if is_running {
        Color::Cyan
    } else if state.success.unwrap_or(false) {
        Color::Green
    } else {
        Color::Red
    };

    let block = Block::default()
        .title(" Running Hook ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    let status_text = if is_running {
        let throbber = throbber_widgets_tui::Throbber::default()
            .throbber_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
            .use_type(throbber_widgets_tui::WhichUse::Spin);
        let span = throbber.to_symbol_span(throbber_state);
        Line::from(vec![
            Span::styled(" ", Style::default()),
            span,
            Span::styled(" Running hook...", Style::default().fg(Color::Cyan)),
        ])
    } else if state.success.unwrap_or(false) {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✓ ", Style::default().fg(Color::Green)),
            Span::styled(
                "Hook completed successfully",
                Style::default().fg(Color::Green),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✗ ", Style::default().fg(Color::Red)),
            Span::styled("Hook failed", Style::default().fg(Color::Red)),
        ])
    };
    frame.render_widget(Paragraph::new(status_text), chunks[0]);

    let script_line = Paragraph::new(Line::from(vec![
        Span::styled(" Script: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.script.clone(),
            Style::default().fg(Color::White),
        ),
    ]))
    .wrap(Wrap { trim: false });
    frame.render_widget(script_line, chunks[1]);

    // Show the last N lines of captured stdout/stderr output.
    // Subtract 1 for the Borders::TOP header row.
    let output_height =
        chunks[2].height.saturating_sub(1) as usize;
    let all_lines: Vec<&str> = state.output.lines().collect();
    let start =
        all_lines.len().saturating_sub(output_height);
    let output_lines: Vec<Line> = if all_lines.is_empty() {
        vec![Line::from(Span::styled(
            " (no output yet)",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        all_lines[start..]
            .iter()
            .map(|l| {
                Line::from(Span::styled(
                    format!(" {}", l),
                    Style::default().fg(Color::White),
                ))
            })
            .collect()
    };
    let output_para = Paragraph::new(output_lines)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(
                    Style::default().fg(Color::DarkGray),
                )
                .title(Span::styled(
                    " output ",
                    Style::default().fg(Color::DarkGray),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(output_para, chunks[2]);

    let hints = if is_running {
        Paragraph::new(Line::from(Span::styled(
            " Please wait...",
            Style::default().fg(Color::DarkGray),
        )))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" continue  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" skip"),
        ]))
    };
    frame.render_widget(hints, chunks[3]);
}

pub fn draw_deleting_feature_dialog(
    frame: &mut Frame,
    state: &DeletingFeatureState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
) {
    let area = centered_rect(50, 30, frame.area());
    frame.render_widget(Clear, area);

    let is_running = state.child.is_some();
    let border_color = if is_running {
        Color::Yellow
    } else if state.error.is_some() {
        Color::Red
    } else {
        Color::Green
    };

    let block = Block::default()
        .title(" Deleting Feature ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);

    let stage_text = match state.stage {
        DeleteStage::KillingTmux => "Stopping tmux session...",
        DeleteStage::RemovingWorktree => "Removing worktree...",
        DeleteStage::Completed => "Done",
    };

    let status_text = if is_running {
        let throbber = throbber_widgets_tui::Throbber::default()
            .throbber_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
            .use_type(throbber_widgets_tui::WhichUse::Spin);
        let span = throbber.to_symbol_span(throbber_state);
        Line::from(vec![
            Span::styled(" ", Style::default()),
            span,
            Span::styled(
                format!(" {}", stage_text),
                Style::default().fg(Color::Yellow),
            ),
        ])
    } else if let Some(ref err) = state.error {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✗ ", Style::default().fg(Color::Red)),
            Span::styled(err, Style::default().fg(Color::Red)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✓ ", Style::default().fg(Color::Green)),
            Span::styled(
                "Feature deleted successfully",
                Style::default().fg(Color::Green),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(status_text), chunks[0]);

    let feature_line = Paragraph::new(Line::from(vec![
        Span::styled(" Feature: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            &state.feature_name,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(feature_line, chunks[1]);

    let project_line = Paragraph::new(Line::from(vec![
        Span::styled(" Project: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&state.project_name, Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(project_line, chunks[2]);

    let hints = if is_running {
        Paragraph::new(Line::from(Span::styled(
            " Please wait...",
            Style::default().fg(Color::DarkGray),
        )))
    } else if state.error.is_some() {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" acknowledge  "),
        ]))
    } else {
        Paragraph::new(Line::from(Span::styled(
            " Press any key to continue...",
            Style::default().fg(Color::DarkGray),
        )))
    };
    frame.render_widget(hints, chunks[3]);
}

pub fn draw_hook_prompt_dialog(frame: &mut Frame, state: &HookPromptState) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", state.title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let items: Vec<ListItem> = state
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            if i == state.selected {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        " > ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        opt.as_str(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(opt.as_str(), Style::default().fg(Color::White)),
                ]))
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" j/k", Style::default().fg(Color::Yellow)),
        Span::raw(" move  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_latest_prompt_dialog(frame: &mut Frame, prompt: &str) {
    let area = centered_rect(80, 70, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Latest Prompt ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let paragraph = Paragraph::new(prompt)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(Color::White));
    frame.render_widget(paragraph, chunks[0]);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::styled("/q", Style::default().fg(Color::Yellow)),
        Span::styled(" close", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(hint, chunks[1]);
}
