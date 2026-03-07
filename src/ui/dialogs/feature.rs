use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::app::{
    CreateFeatureState, CreateFeatureStep, DeleteStage, DeletingFeatureState, ForkFeatureState,
    ForkFeatureStep,
};
use crate::extension::FeaturePreset;
use crate::project::{AgentKind, VibeMode};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_create_feature_dialog(
    frame: &mut Frame,
    state: &CreateFeatureState,
    presets: &[FeaturePreset],
    allowed_agents: &[AgentKind],
    theme: &Theme,
) {
    match state.step {
        CreateFeatureStep::Source => {
            draw_create_feature_source(frame, state, presets, theme);
        }
        CreateFeatureStep::ExistingWorktree => {
            draw_create_feature_worktree_picker(frame, state, theme);
        }
        CreateFeatureStep::SelectPreset => {
            draw_create_feature_preset_picker(frame, state, presets, theme);
        }
        _ => {
            draw_create_feature_branch_mode(frame, state, allowed_agents, theme);
        }
    }
}

fn draw_create_feature_source(
    frame: &mut Frame,
    state: &CreateFeatureState,
    presets: &[FeaturePreset],
    theme: &Theme,
) {
    let area = centered_rect(60, 30, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" New Feature ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

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
        Style::default().fg(theme.primary.to_color()),
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
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_muted.to_color())
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
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::raw(" select  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[3]);
}

fn draw_create_feature_preset_picker(
    frame: &mut Frame,
    state: &CreateFeatureState,
    presets: &[FeaturePreset],
    theme: &Theme,
) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" Select Preset ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    if presets.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No presets configured.",
            Style::default().fg(theme.text_muted.to_color()),
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
                        .fg(theme.primary.to_color())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text.to_color())
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
                        Style::default().fg(theme.primary.to_color()),
                    ),
                    Span::styled(&preset.name, name_style),
                    Span::styled(detail, Style::default().fg(theme.text_muted.to_color())),
                ]);
                let item = ListItem::new(line);
                if is_selected {
                    item.style(Style::default().bg(theme.effective_selection_bg()))
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
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::raw(" select  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" use preset  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[1]);
}

fn draw_create_feature_worktree_picker(
    frame: &mut Frame,
    state: &CreateFeatureState,
    theme: &Theme,
) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" Select Worktree ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    if state.worktrees.is_empty() {
        let empty_msg = Paragraph::new(Line::from(Span::styled(
            "  No available worktrees",
            Style::default().fg(theme.warning.to_color()),
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
                        Style::default().fg(theme.primary.to_color()),
                    ),
                    Span::styled(
                        branch_label,
                        if is_selected {
                            Style::default()
                                .fg(theme.text.to_color())
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme.text.to_color())
                        },
                    ),
                    Span::styled(
                        format!("  {}", path_str),
                        Style::default().fg(theme.text_muted.to_color()),
                    ),
                ]);

                if is_selected {
                    ListItem::new(line).style(Style::default().bg(theme.effective_selection_bg()))
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
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" back"),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(
                " j/k or \u{2191}/\u{2193}",
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::raw(" navigate  "),
            Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" select  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" back"),
        ]))
    };
    frame.render_widget(hints, chunks[1]);
}

fn draw_create_feature_branch_mode(
    frame: &mut Frame,
    state: &CreateFeatureState,
    allowed_agents: &[AgentKind],
    theme: &Theme,
) {
    let area = centered_rect(60, 70, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" New Feature ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

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
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let cursor = if branch_active {
        Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color()))
    } else {
        Span::raw("")
    };

    let branch_field = Paragraph::new(Line::from(vec![
        Span::styled(" Branch: ", branch_label_style),
        Span::styled(&state.branch, Style::default().fg(theme.text.to_color())),
        cursor,
    ]));
    frame.render_widget(branch_field, chunks[0]);

    let wt_active = state.step == CreateFeatureStep::Worktree;
    let wt_label_style = if wt_active {
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };

    let yes_marker = if state.use_worktree { ">" } else { " " };
    let no_marker = if !state.use_worktree { ">" } else { " " };

    let yes_style = if wt_active && state.use_worktree {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else if state.use_worktree {
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let no_style = if wt_active && !state.use_worktree {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else if !state.use_worktree {
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
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
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };

    let mut agent_lines = vec![Line::from(Span::styled(" Agent:", agent_label_style))];

    for (i, agent) in allowed_agents.iter().enumerate() {
        let is_selected = i == state.agent_index;
        let marker = if is_selected { ">" } else { " " };
        let style = if agent_active && is_selected {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(theme.text.to_color())
        } else {
            Style::default().fg(theme.text_muted.to_color())
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
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };

    let mut mode_lines = vec![Line::from(Span::styled(" Mode:", mode_label_style))];

    for (i, m) in VibeMode::ALL.iter().enumerate() {
        let is_selected = i == state.mode_index;
        let marker = if is_selected { ">" } else { " " };
        let style = if mode_active && is_selected {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(theme.text.to_color())
        } else {
            Style::default().fg(theme.text_muted.to_color())
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
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let review_lines = vec![Line::from(vec![
        Span::styled(
            " Review: ",
            if review_active {
                Style::default().fg(theme.primary.to_color())
            } else {
                Style::default().fg(theme.text_muted.to_color())
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
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let chrome_label_style =
        if state.step == CreateFeatureStep::Mode && state.agent == AgentKind::Claude {
            if state.mode_focus == 3 {
                Style::default().fg(theme.primary.to_color())
            } else {
                Style::default().fg(theme.text_muted.to_color())
            }
        } else {
            Style::default().fg(theme.text_muted.to_color())
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
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let notes_lines = vec![Line::from(vec![
        Span::styled(
            " Memo: ",
            if notes_active {
                Style::default().fg(theme.primary.to_color())
            } else {
                Style::default().fg(theme.text_muted.to_color())
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
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::raw(" select  "),
            Span::styled("h/l", Style::default().fg(theme.warning.to_color())),
            Span::raw(" prev/next field  "),
            Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" confirm"),
        ]))
    } else if state.step == CreateFeatureStep::Worktree {
        Paragraph::new(Line::from(vec![
            Span::styled(
                " j/k or \u{2191}/\u{2193}",
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::raw(" toggle  "),
            Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" next  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" back"),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" next  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" cancel"),
        ]))
    };
    frame.render_widget(hints, chunks[15]);
}

pub fn draw_confirm_supervibe_dialog(frame: &mut Frame, theme: &Theme) {
    let area = centered_rect(60, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" SuperVibe Mode ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.danger.to_color()));

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
        Style::default()
            .fg(theme.danger.to_color())
            .add_modifier(Modifier::BOLD),
    )]));
    frame.render_widget(warning, chunks[0]);

    let desc = Paragraph::new(vec![
        Line::from(Span::styled(
            " SuperVibe skips ALL permission checks.",
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(Span::styled(
            " Claude will be able to execute any tool",
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(Span::styled(
            " without asking for confirmation, including",
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(Span::styled(
            " running arbitrary shell commands.",
            Style::default().fg(theme.text.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(desc, chunks[2]);

    let prompt = Paragraph::new(Line::from(vec![
        Span::styled(" Continue? ", Style::default().fg(theme.warning.to_color())),
        Span::styled("(y/n)", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(prompt, chunks[4]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" y", Style::default().fg(theme.warning.to_color())),
        Span::raw(" confirm  "),
        Span::styled("n/Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[5]);
}

pub fn draw_delete_feature_confirm(
    frame: &mut Frame,
    project_name: &str,
    feature_name: &str,
    theme: &Theme,
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
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" from "),
            Span::styled(
                project_name,
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " This will kill the tmux session and remove the worktree.",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw(" Press "),
            Span::styled(
                "y",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to confirm, "),
            Span::styled(
                "n",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" or "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(theme.warning.to_color())
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
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.danger.to_color())),
    );

    frame.render_widget(text, area);
}

pub fn draw_deleting_feature_dialog(
    frame: &mut Frame,
    state: &DeletingFeatureState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(50, 30, frame.area());
    frame.render_widget(Clear, area);

    let is_running = state.child.is_some();
    let border_color = if is_running {
        theme.warning.to_color()
    } else if state.error.is_some() {
        theme.danger.to_color()
    } else {
        theme.success.to_color()
    };

    let block = Block::default()
        .title(" Deleting Feature ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
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
                    .fg(theme.warning.to_color())
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
                Style::default().fg(theme.warning.to_color()),
            ),
        ])
    } else if let Some(ref err) = state.error {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✗ ", Style::default().fg(theme.danger.to_color())),
            Span::styled(err, Style::default().fg(theme.danger.to_color())),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✓ ", Style::default().fg(theme.success.to_color())),
            Span::styled(
                "Feature deleted successfully",
                Style::default().fg(theme.success.to_color()),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(status_text), chunks[0]);

    let feature_line = Paragraph::new(Line::from(vec![
        Span::styled(
            " Feature: ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            &state.feature_name,
            Style::default()
                .fg(theme.danger.to_color())
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(feature_line, chunks[1]);

    let project_line = Paragraph::new(Line::from(vec![
        Span::styled(
            " Project: ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            &state.project_name,
            Style::default().fg(theme.primary.to_color()),
        ),
    ]));
    frame.render_widget(project_line, chunks[2]);

    let hints = if is_running {
        Paragraph::new(Line::from(vec![
            Span::styled(" h", Style::default().fg(theme.warning.to_color())),
            Span::styled(" hide  ", Style::default().fg(theme.text_muted.to_color())),
        ]))
    } else if state.error.is_some() {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" acknowledge  "),
        ]))
    } else {
        Paragraph::new(Line::from(Span::styled(
            " Press any key to continue...",
            Style::default().fg(theme.text_muted.to_color()),
        )))
    };
    frame.render_widget(hints, chunks[3]);
}

pub fn draw_fork_feature_dialog(
    frame: &mut Frame,
    state: &ForkFeatureState,
    allowed_agents: &[AgentKind],
    theme: &Theme,
) {
    let area = centered_rect(60, 40, frame.area());
    frame.render_widget(Clear, area);

    let source_name = &state.source_branch;
    let title = format!(" Fork Feature: {} ", source_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // branch input
            Constraint::Length(1), // spacer
            Constraint::Length(4), // agent picker
            Constraint::Length(1), // spacer
            Constraint::Length(1), // context checkbox
            Constraint::Min(0),
            Constraint::Length(1), // hints
        ])
        .split(inner);

    // Branch name input
    let branch_active = state.step == ForkFeatureStep::Branch;
    let branch_label_style = if branch_active {
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let cursor = if branch_active {
        Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color()))
    } else {
        Span::raw("")
    };

    let branch_field = Paragraph::new(Line::from(vec![
        Span::styled(" Branch: ", branch_label_style),
        Span::styled(
            &state.new_branch,
            Style::default().fg(theme.text.to_color()),
        ),
        cursor,
    ]));
    frame.render_widget(branch_field, chunks[0]);

    // Agent picker
    let agent_active = state.step == ForkFeatureStep::Agent;
    let agent_label_style = if agent_active {
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };

    let mut agent_lines = vec![Line::from(Span::styled(" Agent:", agent_label_style))];

    for (i, agent) in allowed_agents.iter().enumerate() {
        let is_selected = i == state.agent_index;
        let marker = if is_selected { ">" } else { " " };
        let style = if agent_active && is_selected {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(theme.text.to_color())
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };
        agent_lines.push(Line::from(Span::styled(
            format!("   {} {}", marker, agent.display_name()),
            style,
        )));
    }

    let agent_widget = Paragraph::new(agent_lines);
    frame.render_widget(agent_widget, chunks[2]);

    // Context checkbox
    let context_active = state.step == ForkFeatureStep::Agent;
    let ctx_check = if state.include_context { "[x]" } else { "[ ]" };
    let ctx_style = if context_active {
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let ctx_label_style = if context_active {
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let ctx_line = Paragraph::new(Line::from(vec![
        Span::styled(" Context: ", ctx_label_style),
        Span::styled(
            format!("{} Include session transcript", ctx_check),
            ctx_style,
        ),
    ]));
    frame.render_widget(ctx_line, chunks[4]);

    // Hints
    let hints = if branch_active {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" next  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" cancel"),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(
                " j/k or \u{2191}/\u{2193}",
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::raw(" select  "),
            Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" confirm  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" back  "),
            Span::styled("Tab", Style::default().fg(theme.warning.to_color())),
            Span::raw(" toggle context"),
        ]))
    };
    frame.render_widget(hints, chunks[6]);
}
