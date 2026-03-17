use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::app::{
    CreateFeatureState, CreateFeatureStep, DeleteStage, DeletingFeatureState, ForkFeatureState,
    ForkFeatureStep, PromptAnalysis, SteeringPromptState,
};
use crate::editor::{TextEditor, VimMode};
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
        CreateFeatureStep::TaskPrompt => {
            draw_create_feature_prompt_coach(frame, state, theme);
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
                let mode_str = preset.mode.display_name().to_ascii_lowercase();
                let detail = format!(
                    " {} | {}{}",
                    agent_str,
                    mode_str,
                    if preset.review { " | review log" } else { "" }
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
    let area = centered_rect(60, 90, frame.area());
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
            Constraint::Length(3), // worktree
            Constraint::Length(1), // spacer
            Constraint::Length(4), // agent
            Constraint::Length(1), // spacer
            Constraint::Length(5), // mode
            Constraint::Length(1), // spacer
            Constraint::Length(1), // review checkbox
            Constraint::Length(1), // spacer
            Constraint::Length(2), // plan_mode checkbox
            Constraint::Length(1), // spacer
            Constraint::Length(2), // chrome checkbox
            Constraint::Length(1), // spacer
            Constraint::Length(1), // steering coach checkbox
            Constraint::Length(1), // extra space
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
        let name_style = if mode_active && is_selected {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(theme.text.to_color())
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };
        let desc_style = if mode_active && is_selected {
            Style::default().fg(theme.text.to_color())
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };
        mode_lines.push(Line::from(vec![
            Span::styled(
                format!("   {} {:<10}", marker, m.display_name()),
                name_style,
            ),
            Span::styled(m.description(), desc_style),
        ]));
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
            format!("{} Log changes for final code review", review_check),
            review_style,
        ),
    ])];
    let review_widget = Paragraph::new(review_lines);
    frame.render_widget(review_widget, chunks[8]);

    // Plan mode checkbox (chunks[10])
    let plan_active = state.step == CreateFeatureStep::Mode && state.mode_focus == 3;
    let plan_check = if state.plan_mode { "[x]" } else { "[ ]" };
    let plan_style = if plan_active {
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let plan_lines = vec![Line::from(vec![
        Span::styled(
            " Plan: ",
            if plan_active {
                Style::default().fg(theme.primary.to_color())
            } else {
                Style::default().fg(theme.text_muted.to_color())
            },
        ),
        Span::styled(
            format!("{} Collaborative planning mode", plan_check),
            plan_style,
        ),
    ])];
    let plan_widget = Paragraph::new(plan_lines);
    frame.render_widget(plan_widget, chunks[10]);

    // Chrome checkbox (chunks[12])
    let chrome_active = state.step == CreateFeatureStep::Mode
        && state.mode_focus == 4
        && state.agent == AgentKind::Claude;
    let chrome_check = if state.enable_chrome { "[x]" } else { "[ ]" };
    let chrome_style = if chrome_active {
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let chrome_label_style =
        if state.step == CreateFeatureStep::Mode && state.agent == AgentKind::Claude {
            if state.mode_focus == 4 {
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
        frame.render_widget(chrome_widget, chunks[12]);
    }

    let steering_focus = if state.agent == AgentKind::Claude {
        4
    } else {
        3
    };
    let steering_active =
        state.step == CreateFeatureStep::Mode && state.mode_focus == steering_focus;
    let steering_check = if state.steering_enabled { "[x]" } else { "[ ]" };
    let steering_lines = vec![Line::from(vec![
        Span::styled(
            " Steering Coach: ",
            if steering_active {
                Style::default().fg(theme.primary.to_color())
            } else {
                Style::default().fg(theme.text_muted.to_color())
            },
        ),
        Span::styled(
            format!("{} Show prompt guidance before launch", steering_check),
            if steering_active {
                Style::default().fg(theme.text.to_color())
            } else {
                Style::default().fg(theme.text_muted.to_color())
            },
        ),
    ])];
    let steering_widget = Paragraph::new(steering_lines);
    frame.render_widget(steering_widget, chunks[14]);

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
    frame.render_widget(hints, chunks[17]);
}

fn draw_create_feature_prompt_coach(frame: &mut Frame, state: &CreateFeatureState, theme: &Theme) {
    let area = centered_rect(84, 82, frame.area());
    frame.render_widget(Clear, area);

    let score_color = if state.prompt_analysis.score >= 8 {
        theme.success.to_color()
    } else if state.prompt_analysis.score >= 4 {
        theme.warning.to_color()
    } else {
        theme.danger.to_color()
    };

    let title = format!(
        " Steering Coach ({})  {} / {} ",
        state.project_name, state.prompt_analysis.score, state.prompt_analysis.max_score
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(score_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Length(1),
            Constraint::Length(8),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(inner);

    let summary = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" Score: ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled(
                format!(
                    "{} / {}",
                    state.prompt_analysis.score, state.prompt_analysis.max_score
                ),
                Style::default()
                    .fg(score_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            state.prompt_analysis.summary.as_str(),
            Style::default().fg(theme.text.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(summary, chunks[0]);

    let prompt_text = if state.task_prompt.is_empty() {
        vec![Line::from(Span::styled(
            "Describe the task, then add boundaries, validation, and watch-outs.",
            Style::default().fg(theme.text_muted.to_color()),
        ))]
    } else {
        let mut lines = state
            .task_prompt
            .lines()
            .map(|line| Line::from(Span::styled(line, Style::default().fg(theme.text.to_color()))))
            .collect::<Vec<_>>();
        if state.task_prompt.ends_with('\n') || lines.is_empty() {
            lines.push(Line::from(""));
        }
        if let Some(last) = lines.last_mut() {
            last.spans.push(Span::styled(
                "\u{2588}",
                Style::default().fg(theme.primary.to_color()),
            ));
        }
        lines
    };

    let prompt = Paragraph::new(prompt_text)
        .block(
            Block::default()
                .title(" Draft Task Prompt ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.primary.to_color())),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(prompt, chunks[1]);

    let checklist = Paragraph::new(prompt_checklist_lines(&state.prompt_analysis, theme))
        .block(
            Block::default()
                .title(" Constraint Checklist ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.primary.to_color())),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(checklist, chunks[3]);

    let mut tip_lines = Vec::new();
    if state.prompt_analysis.teaching_tips.is_empty() {
        tip_lines.push(Line::from(Span::styled(
            "Your draft already covers the core steering constraints. Launch when the wording is precise enough for this repo.",
            Style::default().fg(theme.text.to_color()),
        )));
    } else {
        for tip in &state.prompt_analysis.teaching_tips {
            tip_lines.push(Line::from(vec![
                Span::styled(" - ", Style::default().fg(theme.warning.to_color())),
                Span::styled(tip, Style::default().fg(theme.text.to_color())),
            ]));
        }
    }
    let tips = Paragraph::new(tip_lines)
        .block(
            Block::default()
                .title(" How To Strengthen It ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.primary.to_color())),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(tips, chunks[5]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled("Type / Paste", Style::default().fg(theme.warning.to_color())),
        Span::raw(" edit  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" newline  "),
        Span::styled("Tab", Style::default().fg(theme.warning.to_color())),
        Span::raw(" launch"),
    ]));
    frame.render_widget(hints, chunks[6]);
}

fn prompt_checklist_lines(analysis: &PromptAnalysis, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for check in &analysis.checks {
        let (marker, color, detail) = if check.present {
            (
                "[x]",
                theme.success.to_color(),
                "covered",
            )
        } else {
            (
                "[ ]",
                theme.warning.to_color(),
                check.constraint.missing_explanation(),
            )
        };

        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", marker), Style::default().fg(color)),
            Span::styled(
                check.constraint.label(),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" - {}", detail),
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]));
    }

    lines
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

pub fn draw_steering_prompt_dialog(
    frame: &mut Frame,
    state: &SteeringPromptState,
    theme: &Theme,
) {
    let area = centered_rect(84, 82, frame.area());
    frame.render_widget(Clear, area);

    let score_color = if state.prompt_analysis.score >= 8 {
        theme.success.to_color()
    } else if state.prompt_analysis.score >= 4 {
        theme.warning.to_color()
    } else {
        theme.danger.to_color()
    };

    let title = format!(
        " Steering Coach ({})  {} / {} ",
        state.view.feature_name, state.prompt_analysis.score, state.prompt_analysis.max_score
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(score_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Length(1),
            Constraint::Length(8),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(inner);

    let summary = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(" Session: ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled(
                state.view.session_label.as_str(),
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            state.prompt_analysis.summary.as_str(),
            Style::default().fg(theme.text.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(summary, chunks[0]);

    let prompt_text = steering_editor_lines(&state.editor, theme);
    let prompt_title = match state.editor.vim_mode() {
        Some(VimMode::Insert) => " Prompt To Inject [Vim Insert] ",
        Some(VimMode::Normal) => " Prompt To Inject [Vim Normal] ",
        None => " Prompt To Inject ",
    };

    let prompt = Paragraph::new(prompt_text)
        .block(
            Block::default()
                .title(prompt_title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.primary.to_color())),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(prompt, chunks[1]);

    let checklist = Paragraph::new(prompt_checklist_lines(&state.prompt_analysis, theme))
        .block(
            Block::default()
                .title(" Constraint Checklist ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.primary.to_color())),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(checklist, chunks[3]);

    let mut tip_lines = Vec::new();
    if state.prompt_analysis.teaching_tips.is_empty() {
        tip_lines.push(Line::from(Span::styled(
            "Your draft covers the main steering constraints. Press Tab to inject it into the running agent session.",
            Style::default().fg(theme.text.to_color()),
        )));
    } else {
        for tip in &state.prompt_analysis.teaching_tips {
            tip_lines.push(Line::from(vec![
                Span::styled(" - ", Style::default().fg(theme.warning.to_color())),
                Span::styled(tip, Style::default().fg(theme.text.to_color())),
            ]));
        }
    }
    let tips = Paragraph::new(tip_lines)
        .block(
            Block::default()
                .title(" How To Strengthen It ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.primary.to_color())),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(tips, chunks[5]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            if matches!(state.editor.vim_mode(), Some(VimMode::Normal)) {
                "i / a / o"
            } else {
                "Type / Paste"
            },
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::raw(if matches!(state.editor.vim_mode(), Some(VimMode::Normal)) {
            " edit  "
        } else {
            " edit  "
        }),
        Span::styled(
            if matches!(state.editor.vim_mode(), Some(VimMode::Normal)) {
                "h/j/k/l"
            } else {
                "Esc"
            },
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::raw(if matches!(state.editor.vim_mode(), Some(VimMode::Normal)) {
            " move  "
        } else {
            " normal  "
        }),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(if matches!(state.editor.vim_mode(), Some(VimMode::Normal)) {
            " ignored  "
        } else {
            " newline  "
        }),
        Span::styled("Ctrl+V", Style::default().fg(theme.warning.to_color())),
        Span::raw(if state.editor.vim_mode().is_some() {
            " vim off  "
        } else {
            " vim on  "
        }),
        Span::styled("Tab", Style::default().fg(theme.warning.to_color())),
        Span::raw(" inject  "),
        Span::styled("Ctrl+Q", Style::default().fg(theme.warning.to_color())),
        Span::raw(" close"),
    ]));
    frame.render_widget(hints, chunks[6]);
}

fn steering_editor_lines(editor: &TextEditor, theme: &Theme) -> Vec<Line<'static>> {
    if editor.text().is_empty() {
        return vec![
            Line::from(Span::styled(
                "\u{2588}",
                Style::default().fg(theme.primary.to_color()),
            )),
            Line::from(Span::styled(
                "Describe the task, then add boundaries, validation, and watch-outs.",
                Style::default().fg(theme.text_muted.to_color()),
            )),
        ];
    }

    let mut lines = editor
        .text()
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let (cursor_row, cursor_col) = editor.cursor_row_col();
    while lines.len() <= cursor_row {
        lines.push(String::new());
    }
    if let Some(line) = lines.get_mut(cursor_row) {
        let insert_at = char_col_to_byte_idx(line, cursor_col);
        line.insert(insert_at, '\u{2588}');
    }

    lines
        .into_iter()
        .map(|line| {
            Line::from(Span::styled(
                line,
                Style::default().fg(theme.text.to_color()),
            ))
        })
        .collect()
}

fn char_col_to_byte_idx(text: &str, char_col: usize) -> usize {
    text.char_indices()
        .nth(char_col)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
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
