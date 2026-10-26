use crate::app::{
    CreateFeatureState, CreateFeatureStep, DeleteStage, DeletingFeatureState, ForkFeatureState,
    ForkFeatureStep, PromptAnalysis, SteeringPromptState,
};
use crate::editor::VimMode;
use crate::extension::FeaturePreset;
use crate::project::{AgentKind, VibeMode};
use crate::theme::Theme;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};

use super::super::dashboard::centered_rect;

pub fn draw_create_feature_dialog(
    frame: &mut Frame,
    state: &CreateFeatureState,
    presets: &[FeaturePreset],
    allowed_agents: &[AgentKind],
    usage: &crate::usage::UsageData,
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
            draw_create_feature_branch_mode(frame, state, allowed_agents, usage, theme);
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
    crate::ui::draw_modal_overlay(frame, area, theme);

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
    crate::ui::draw_modal_overlay(frame, area, theme);

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
    crate::ui::draw_modal_overlay(frame, area, theme);

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
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let query_line = if state.worktree_search_active {
        let query_text = if state.worktree_query.is_empty() {
            "Type to filter worktrees"
        } else {
            &state.worktree_query
        };
        let query_style = if state.worktree_query.is_empty() {
            Style::default().fg(theme.text_muted.to_color())
        } else {
            Style::default().fg(theme.text.to_color())
        };
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  / ",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(query_text, query_style),
        ]))
    } else {
        let text = if state.worktree_query.is_empty() {
            "  Press / to search".to_string()
        } else {
            format!("  Search: {}", state.worktree_query)
        };
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(theme.text_muted.to_color()),
        )))
    };
    frame.render_widget(query_line, chunks[0]);

    if state.worktrees.is_empty() {
        let empty_msg = Paragraph::new(Line::from(Span::styled(
            "  No available worktrees",
            Style::default().fg(theme.warning.to_color()),
        )));
        frame.render_widget(empty_msg, chunks[1]);
    } else {
        let visible_indices = state.visible_worktree_indices();
        if visible_indices.is_empty() {
            let empty_msg = Paragraph::new(Line::from(Span::styled(
                "  No worktrees match the current filter.",
                Style::default().fg(theme.text_muted.to_color()),
            )));
            frame.render_widget(empty_msg, chunks[1]);
        } else {
            let items: Vec<ListItem> = visible_indices
                .iter()
                .filter_map(|&i| {
                    let wt = state.worktrees.get(i)?;
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

                    Some(if is_selected {
                        ListItem::new(line)
                            .style(Style::default().bg(theme.effective_selection_bg()))
                    } else {
                        ListItem::new(line)
                    })
                })
                .collect();

            let selected_visible = visible_indices
                .iter()
                .position(|&i| i == state.worktree_index);
            let list = List::new(items);
            let mut list_state = ListState::default();
            list_state.select(selected_visible);
            frame.render_stateful_widget(list, chunks[1], &mut list_state);
        }
    }

    let hints = if state.worktrees.is_empty() {
        Paragraph::new(Line::from(vec![
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" back"),
        ]))
    } else if state.worktree_search_active {
        Paragraph::new(Line::from(vec![
            Span::styled(
                " j/k or \u{2191}/\u{2193}",
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::raw(" navigate  "),
            Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" select  "),
            Span::styled("Backspace", Style::default().fg(theme.warning.to_color())),
            Span::raw(" clear filter  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" back to selection mode"),
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
            Span::styled("/", Style::default().fg(theme.warning.to_color())),
            Span::raw(" search  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" back"),
        ]))
    };
    frame.render_widget(hints, chunks[2]);
}

/// A form row's label. The focused row gets a `▸` marker and a highlighted
/// background, so focus is visible without reading the footer; other rows
/// keep a space in the marker column so nothing shifts.
fn form_label(label: &str, active: bool, theme: &Theme) -> Span<'static> {
    if active {
        Span::styled(
            format!("\u{25b8}{label}"),
            Style::default()
                .fg(theme.primary.to_color())
                .bg(theme.effective_selection_bg())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!(" {label}"),
            Style::default().fg(theme.text_muted.to_color()),
        )
    }
}

fn draw_create_feature_branch_mode(
    frame: &mut Frame,
    state: &CreateFeatureState,
    allowed_agents: &[AgentKind],
    usage: &crate::usage::UsageData,
    theme: &Theme,
) {
    let area = centered_rect(60, 90, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);
    let has_chrome_row = state.agent == AgentKind::Claude;
    let has_rc_row = state.agent == AgentKind::Claude;

    let title = format!(" New Feature ({}) ", state.project_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    let buf = frame.buffer_mut();

    // 1 name line per harness, plus 1 more when a usage summary is known
    // for it (omitted entirely when unknown, per the picker's "fail
    // silently" rule) — the block above sizes itself to this each frame.
    let agent_summaries: Vec<Option<String>> = allowed_agents
        .iter()
        .map(|agent| {
            crate::usage::format_usage_summary(&crate::usage::usage_windows_for(agent, usage))
        })
        .collect();
    let wanted_agent_block_height = 1 + agent_summaries
        .iter()
        .map(|summary| if summary.is_some() { 2 } else { 1 })
        .sum::<u16>();
    // Every other Length section below plus the two single-line spacers
    // around the Min(0) help area — kept in sync with the constraints list
    // just below so the agent block never grows into space they need.
    let other_sections_height = 3 + 1 + 3 + 1 // [0..3] branch, spacer, worktree, spacer
        + 1 + 5 + 1 // [5..7] spacer, mode, spacer
        + 1 + 1 // [8][10] review, plan checkboxes
        + if has_chrome_row { 1 } else { 0 } // [12] chrome checkbox
        + if has_rc_row { 1 } else { 0 } // [14] remote_control checkbox
        + 1 // [16] steering coach checkbox
        + 1 // [17] session name
        + 1 // [18] spacer before focused help
        + 2; // [20] hints
    let min_agent_block_height = 1 + allowed_agents.len() as u16;
    // Not even the squeezed form plus its hints fits (80x24 with Claude): lay
    // it out at full height off-screen and show a window that follows focus,
    // rather than clipping the Mode list.
    let compact = inner.height < other_sections_height + min_agent_block_height;
    let agent_block_height = if compact {
        wanted_agent_block_height
    } else {
        wanted_agent_block_height.min(
            inner
                .height
                .saturating_sub(other_sections_height)
                .max(min_agent_block_height),
        )
    };
    let form_height = other_sections_height - 2 + agent_block_height;
    let mut scratch = ratatui::buffer::Buffer::empty(Rect::new(0, 0, inner.width, form_height));
    let layout_area = if compact {
        // The two trailing rows are the hints chunk; they are drawn for real.
        Rect::new(0, 0, inner.width, form_height + 2)
    } else {
        inner
    };
    let frame_buf = buf;
    let buf: &mut ratatui::buffer::Buffer = if compact { &mut scratch } else { frame_buf };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),                                  // [0] branch
            Constraint::Length(1),                                  // [1] spacer
            Constraint::Length(3),                                  // [2] worktree
            Constraint::Length(1),                                  // [3] spacer
            Constraint::Length(agent_block_height),                 // [4] agent
            Constraint::Length(1),                                  // [5] spacer
            Constraint::Length(5),                                  // [6] mode
            Constraint::Length(1),                                  // [7] spacer
            Constraint::Length(1),                                  // [8] review checkbox
            Constraint::Length(0),                                  // [9] spacer
            Constraint::Length(1),                                  // [10] plan_mode checkbox
            Constraint::Length(0),                                  // [11] spacer
            Constraint::Length(if has_chrome_row { 1 } else { 0 }), // [12] chrome checkbox
            Constraint::Length(0),                                  // [13] spacer
            Constraint::Length(if has_rc_row { 1 } else { 0 }),     // [14] remote_control
            Constraint::Length(0),                                  // [15] spacer
            Constraint::Length(1),                                  // [16] steering coach checkbox
            Constraint::Length(1),                                  // [17] session name
            Constraint::Length(1), // [18] spacer before focused help
            Constraint::Min(0),    // [19] focused option help
            Constraint::Length(2), // [20] hints
        ])
        .split(layout_area);

    let branch_active = state.step == CreateFeatureStep::Branch;
    let branch_has_error = state.branch_error.is_some();
    let branch_label_style = if branch_has_error {
        Style::default().fg(theme.danger.to_color())
    } else if branch_active {
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let cursor = if branch_active {
        let color = if branch_has_error {
            theme.danger.to_color()
        } else {
            theme.primary.to_color()
        };
        Span::styled("\u{2588}", Style::default().fg(color))
    } else {
        Span::raw("")
    };
    let branch_value_style = if branch_has_error {
        Style::default().fg(theme.danger.to_color())
    } else {
        Style::default().fg(theme.text.to_color())
    };

    let branch_label = if branch_active {
        form_label("Name: ", true, theme)
    } else {
        Span::styled(" Name: ", branch_label_style)
    };
    let mut branch_lines = vec![Line::from(vec![
        branch_label,
        Span::styled(&state.branch, branch_value_style),
        cursor,
    ])];
    if let Some(error) = &state.branch_error {
        branch_lines.push(Line::from(Span::styled(
            format!("   {error}"),
            Style::default().fg(theme.danger.to_color()),
        )));
    }
    let branch_field = Paragraph::new(branch_lines);
    put(buf, branch_field, chunks[0]);

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

    let yes_text = if state.git_available {
        format!("   {} Yes", yes_marker)
    } else {
        "     Yes (unavailable: this folder is not a git repository; run git init)".to_string()
    };
    let yes_style = if state.git_available {
        yes_style
    } else {
        Style::default().fg(theme.warning.to_color())
    };
    let wt_lines = vec![
        Line::from(if wt_active {
            form_label("Worktree:", true, theme)
        } else {
            Span::styled(" Worktree:", wt_label_style)
        }),
        Line::from(Span::styled(yes_text, yes_style)),
        Line::from(Span::styled(
            format!("   {} No (use repo dir)", no_marker),
            no_style,
        )),
    ];
    let wt_widget = Paragraph::new(wt_lines);
    put(buf, wt_widget, chunks[2]);

    let agent_active = state.step == CreateFeatureStep::Mode && state.mode_focus == 0;
    let agent_label_style = if agent_active {
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };

    let mut agent_lines = vec![Line::from(if agent_active {
        form_label("Harness:", true, theme)
    } else {
        Span::styled(" Harness:", agent_label_style)
    })];

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
        if let Some(summary) = &agent_summaries[i] {
            agent_lines.push(Line::from(Span::styled(
                format!("      {summary}"),
                Style::default().fg(theme.text_muted.to_color()),
            )));
        }
    }

    let agent_widget = Paragraph::new(agent_lines);
    put(buf, agent_widget, chunks[4]);

    let mode_active = state.step == CreateFeatureStep::Mode && state.mode_focus == 1;
    let mode_label_style = if mode_active {
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };

    let mut mode_lines = vec![Line::from(if mode_active {
        form_label("Mode:", true, theme)
    } else {
        Span::styled(" Mode:", mode_label_style)
    })];

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
    put(buf, mode_widget, chunks[6]);

    // Review checkbox (chunks[8])
    let review_active = state.step == CreateFeatureStep::Mode && state.mode_focus == 2;
    let review_check = if state.review { "[x]" } else { "[ ]" };
    let review_style = if review_active {
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let review_lines = vec![Line::from(vec![
        form_label("Review: ", review_active, theme),
        Span::styled(format!("{} Final review log", review_check), review_style),
        Span::styled(
            " HIGH TOKEN USAGE",
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    let review_widget = Paragraph::new(review_lines);
    put(buf, review_widget, chunks[8]);

    // Plan checkbox (chunks[10]): a 3-state cycle (None / Quick Plan / Full
    // Plan) on one field, mirroring how the mode picker above lists its
    // options rather than adding a second checkbox.
    let plan_active = state.step == CreateFeatureStep::Mode && state.mode_focus == 3;
    let (plan_check, plan_label) = match (state.plan_mode, state.quick_plan) {
        (true, true) => ("[Q]", "Quick Plan"),
        (true, false) => ("[x]", "Full Plan (collaborative planning)"),
        (false, _) => ("[ ]", "None"),
    };
    let plan_style = if plan_active {
        Style::default().fg(theme.text.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let plan_lines = vec![Line::from(vec![
        form_label("Plan: ", plan_active, theme),
        Span::styled(format!("{plan_check} {plan_label}"), plan_style),
    ])];
    let plan_widget = Paragraph::new(plan_lines);
    put(buf, plan_widget, chunks[10]);

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
            if chrome_active {
                form_label("Chrome: ", true, theme)
            } else {
                Span::styled(" Chrome: ", chrome_label_style)
            },
            Span::styled(
                format!("{} Enable browser automation", chrome_check),
                chrome_style,
            ),
        ])];
        let chrome_widget = Paragraph::new(chrome_lines);
        put(buf, chrome_widget, chunks[12]);

        // Remote Control checkbox (chunks[14]) — Claude only.
        // Requires claude.ai OAuth; unavailable for z.ai / third-party
        // provider sessions, in which case it renders disabled with a reason.
        let rc_available = state.remote_control_available;
        let rc_active =
            rc_available && state.step == CreateFeatureStep::Mode && state.mode_focus == 5;
        let rc_check = if state.remote_control && rc_available {
            "[x]"
        } else {
            "[ ]"
        };
        let rc_label_style = if rc_active {
            Style::default().fg(theme.primary.to_color())
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };
        let rc_value_style = if rc_active {
            Style::default().fg(theme.text.to_color())
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };
        let rc_value = if rc_available {
            format!("{} Enable claude.ai/mobile sync", rc_check)
        } else {
            let reason = state
                .remote_control_block_reason
                .as_deref()
                .unwrap_or("Unavailable");
            format!("{} {}", rc_check, reason)
        };
        let rc_focused = state.step == CreateFeatureStep::Mode && state.mode_focus == 5;
        let rc_lines = vec![Line::from(vec![
            if rc_focused {
                form_label("Remote Control: ", true, theme)
            } else {
                Span::styled(" Remote Control: ", rc_label_style)
            },
            Span::styled(rc_value, rc_value_style),
        ])];
        let rc_widget = Paragraph::new(rc_lines);
        put(buf, rc_widget, chunks[14]);
    }

    // Steering coach — focus 6 for Claude, 4 for others.
    let steering_focus = if state.agent == AgentKind::Claude {
        6
    } else {
        4
    };
    let steering_active =
        state.step == CreateFeatureStep::Mode && state.mode_focus == steering_focus;
    let steering_check = if state.steering_enabled { "[x]" } else { "[ ]" };
    let steering_lines = vec![Line::from(vec![
        form_label("Steering: ", steering_active, theme),
        Span::styled(
            format!("{} Prompt coach (experimental)", steering_check),
            if steering_active {
                Style::default().fg(theme.text.to_color())
            } else {
                Style::default().fg(theme.text_muted.to_color())
            },
        ),
    ])];
    let steering_widget = Paragraph::new(steering_lines);
    put(buf, steering_widget, chunks[16]);

    // Session name (chunks[17]): the agent session's label, defaulting to
    // "<Harness> 1". It used to be a separate prompt after the form.
    let session_active = state.step == CreateFeatureStep::SessionName;
    let mut session_spans = vec![
        form_label("Session: ", session_active, theme),
        Span::styled(
            state.session_name.clone(),
            if session_active {
                Style::default().fg(theme.text.to_color())
            } else {
                Style::default().fg(theme.text_muted.to_color())
            },
        ),
    ];
    if session_active {
        session_spans.push(Span::styled(
            "\u{2588}",
            Style::default().fg(theme.primary.to_color()),
        ));
    }
    put(buf, Paragraph::new(Line::from(session_spans)), chunks[17]);

    if let Some(help_lines) = focused_help_lines(state, theme) {
        let help = Paragraph::new(help_lines)
            .block(
                Block::default()
                    .borders(Borders::LEFT)
                    .border_style(Style::default().fg(theme.primary.to_color()))
                    .style(Style::default().bg(theme.effective_selection_bg())),
            )
            .style(Style::default().bg(theme.effective_selection_bg()))
            .wrap(Wrap { trim: true });
        // As tall as the wrapped text (at least three rows), never taller
        // than the space left.
        let wanted = (help.line_count(chunks[19].width) as u16).max(3);
        let help_area = if chunks[19].height > wanted {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(wanted), Constraint::Min(0)])
                .split(chunks[19])[0]
        } else {
            chunks[19]
        };
        put(buf, help, help_area);
    }

    if compact {
        let hints_area = Rect::new(inner.x, inner.y + inner.height - 2, inner.width, 2);
        let viewport = inner.height.saturating_sub(2);
        // The chunk holding the focused field, to keep it in the window.
        let focus_chunk = match state.step {
            CreateFeatureStep::Branch => 0,
            CreateFeatureStep::Worktree => 2,
            CreateFeatureStep::SessionName => 17,
            _ => match (state.mode_focus, state.agent == AgentKind::Claude) {
                (0, _) => 4,
                (1, _) => 6,
                (2, _) => 8,
                (3, _) => 10,
                (4, true) => 12,
                (5, true) => 14,
                _ => 16,
            },
        };
        let focus_bottom = chunks[focus_chunk].bottom();
        let offset = focus_bottom
            .saturating_sub(viewport)
            .min(form_height.saturating_sub(viewport));
        for row in 0..viewport.min(form_height) {
            for col in 0..inner.width {
                if let Some(cell) = scratch.cell((col, row + offset))
                    && let Some(dst) = frame_buf_cell(frame, inner.x + col, inner.y + row)
                {
                    *dst = cell.clone();
                }
            }
        }
        // Drawn on the right border, so it never covers form text.
        let more = |frame: &mut Frame, y: u16, symbol: &str| {
            if let Some(cell) = frame_buf_cell(frame, inner.x + inner.width, y) {
                cell.set_symbol(symbol)
                    .set_style(Style::default().fg(theme.warning.to_color()));
            }
        };
        if offset > 0 {
            more(frame, inner.y, "\u{25b2}");
        }
        if offset + viewport < form_height {
            more(frame, inner.y + viewport - 1, "\u{25bc}");
        }
        frame.render_widget(form_hints(state, theme, inner.width), hints_area);
    } else {
        put(buf, form_hints(state, theme, inner.width), chunks[20]);
    }
}

fn frame_buf_cell<'a>(
    frame: &'a mut Frame<'_>,
    x: u16,
    y: u16,
) -> Option<&'a mut ratatui::buffer::Cell> {
    frame.buffer_mut().cell_mut((x, y))
}

/// Render `widget` into `buf`; the feature form draws through this so it can
/// target either the frame or an off-screen scratch buffer.
fn put<W: ratatui::widgets::Widget>(buf: &mut ratatui::buffer::Buffer, widget: W, area: Rect) {
    widget.render(area, buf);
}

/// The explanation box under the form for the focused row. On the Mode row
/// it spells out, one plain line per mode, what the agent may do without
/// asking — the list above only has room for a short phrase.
fn focused_help_lines(state: &CreateFeatureState, theme: &Theme) -> Option<Vec<Line<'static>>> {
    let label = || Span::styled("  Details  ", Style::default().fg(theme.primary.to_color()));
    let text = |t: &'static str| Span::styled(t, Style::default().fg(theme.text.to_color()));
    match state.step {
        CreateFeatureStep::Mode if state.mode_focus == 1 => Some(
            VibeMode::ALL
                .iter()
                .map(|mode| {
                    let mut name = Style::default().fg(theme.primary.to_color());
                    if *mode == state.mode {
                        name = name.add_modifier(Modifier::BOLD);
                    }
                    Line::from(vec![
                        Span::styled(format!("  {:<10} ", mode.display_name()), name),
                        text(mode.permission_summary()),
                    ])
                })
                .collect(),
        ),
        CreateFeatureStep::Mode => state
            .focused_mode_description()
            .map(|description| vec![Line::from(vec![label(), text(description)])]),
        CreateFeatureStep::SessionName => Some(vec![Line::from(vec![
            label(),
            text(
                "The name of this feature's first agent session. Enter creates the feature and opens it.",
            ),
        ])]),
        _ => None,
    }
}

/// The form's footer: one line of keys that work on every row, and one for
/// the focused row. "Enter" is labelled for what it does on that row.
/// Narrower than this, the common line uses its short form so `Ctrl+S` and
/// `Esc` are not cut off (80-column terminals).
const FORM_HINTS_FULL_WIDTH: u16 = 64;

fn form_hints(state: &CreateFeatureState, theme: &Theme, width: u16) -> Paragraph<'static> {
    let key =
        |text: &'static str| Span::styled(text, Style::default().fg(theme.warning.to_color()));
    let on_last_row = state.step == CreateFeatureStep::SessionName;
    let cancels = state.step == CreateFeatureStep::Branch && state.worktrees.is_empty();
    let full = width >= FORM_HINTS_FULL_WIDTH;
    // Narrow: Tab moves to the second line so the first keeps create/back.
    let mut common = Vec::new();
    if full {
        common.push(key(" Tab/Shift+Tab"));
        common.push(Span::raw(" field  "));
    }
    common.extend([
        key(if full { "Enter" } else { " Enter" }),
        Span::raw(match (on_last_row, full) {
            (true, _) => " create  ",
            (false, true) => " next field  ",
            (false, false) => " next  ",
        }),
        key("Ctrl+S"),
        Span::raw(" create  "),
        key("Esc"),
        Span::raw(if cancels { " cancel" } else { " back" }),
    ]);

    let focused: Vec<Span<'static>> = match state.step {
        CreateFeatureStep::Branch | CreateFeatureStep::SessionName => vec![
            Span::raw(" Type to edit  "),
            key("Ctrl+U"),
            Span::raw(" clear"),
        ],
        CreateFeatureStep::Worktree => vec![
            key(" Space"),
            Span::raw(" or "),
            key("j/k"),
            Span::raw(" toggle Yes/No"),
        ],
        CreateFeatureStep::Mode if state.mode_focus <= 1 => vec![
            key(" j/k"),
            Span::raw(" or "),
            key("\u{2191}/\u{2193}"),
            Span::raw(if state.mode_focus == 0 {
                " choose harness"
            } else {
                " choose mode"
            }),
        ],
        CreateFeatureStep::Mode if state.mode_focus == 3 => vec![
            key(" Space"),
            Span::raw(" or "),
            key("j/k"),
            Span::raw(" cycle None / Quick / Full"),
        ],
        CreateFeatureStep::Mode => vec![
            key(" Space"),
            Span::raw(" or "),
            key("j/k"),
            Span::raw(" toggle [x]"),
        ],
        _ => Vec::new(),
    };
    let mut second = Vec::new();
    if !full {
        second.push(key(" Tab"));
        second.push(Span::raw(" field "));
    }
    second.extend(focused);
    Paragraph::new(vec![Line::from(common), Line::from(second)])
}

fn draw_create_feature_prompt_coach(frame: &mut Frame, state: &CreateFeatureState, theme: &Theme) {
    let area = centered_rect(84, 82, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

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
            .map(|line| {
                Line::from(Span::styled(
                    line,
                    Style::default().fg(theme.text.to_color()),
                ))
            })
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
        Span::styled(
            "Type / Paste",
            Style::default().fg(theme.warning.to_color()),
        ),
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
            ("[x]", theme.success.to_color(), "covered")
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
    crate::ui::draw_modal_overlay(frame, area, theme);

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
    state: &mut SteeringPromptState,
    theme: &Theme,
) {
    let area = centered_rect(84, 82, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

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
            Span::styled(
                " Session: ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
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

    let prompt_text = super::editor_view::editor_lines(
        &state.editor,
        theme,
        "Describe the task, then add boundaries, validation, and watch-outs.",
    );
    let prompt_title = match state.editor.vim_mode() {
        Some(VimMode::Insert) => " Prompt To Inject [Vim Insert] ",
        Some(VimMode::Normal) => " Prompt To Inject [Vim Normal] ",
        None => " Prompt To Inject ",
    };

    let prompt_block = Block::default()
        .title(prompt_title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.primary.to_color()));
    let prompt_inner = prompt_block.inner(chunks[1]);
    let visible_lines = prompt_inner.height as usize;
    let mut wrap_width = prompt_inner.width as usize;
    let mut total_visual_lines =
        super::editor_view::count_wrapped_editor_lines(&prompt_text, wrap_width);
    if total_visual_lines > visible_lines && wrap_width > 1 {
        wrap_width -= 1;
        total_visual_lines =
            super::editor_view::count_wrapped_editor_lines(&prompt_text, wrap_width);
    }
    super::editor_view::sync_editor_scroll(
        &state.editor,
        &mut state.scroll_offset,
        &mut state.sync_scroll_to_cursor,
        visible_lines,
        wrap_width,
        total_visual_lines,
    );

    let prompt = Paragraph::new(prompt_text)
        .block(prompt_block)
        .wrap(Wrap { trim: false })
        .scroll((state.scroll_offset.min(u16::MAX as usize) as u16, 0));
    frame.render_widget(prompt, chunks[1]);

    if total_visual_lines > visible_lines {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(total_visual_lines)
            .position(state.scroll_offset)
            .viewport_content_length(visible_lines);
        frame.render_stateful_widget(scrollbar, chunks[1], &mut scrollbar_state);
    }

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
        Span::raw(" edit  "),
        Span::styled(
            if matches!(state.editor.vim_mode(), Some(VimMode::Normal)) {
                "h/j/k/l"
            } else {
                "Esc"
            },
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::raw(
            if matches!(state.editor.vim_mode(), Some(VimMode::Normal)) {
                " move  "
            } else {
                " normal  "
            },
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(
            if matches!(state.editor.vim_mode(), Some(VimMode::Normal)) {
                " ignored  "
            } else {
                " newline  "
            },
        ),
        Span::styled("Ctrl+J/K", Style::default().fg(theme.warning.to_color())),
        Span::raw(" scroll  "),
        Span::styled("PgUp/PgDn", Style::default().fg(theme.warning.to_color())),
        Span::raw(" page  "),
        Span::styled("Ctrl+L", Style::default().fg(theme.warning.to_color())),
        Span::raw(" clear  "),
        Span::styled("Ctrl+T", Style::default().fg(theme.warning.to_color())),
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

fn confirm_prompt_line(theme: &Theme) -> Line<'static> {
    let key = |k: &'static str| {
        Span::styled(
            k,
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        )
    };
    Line::from(vec![
        Span::raw(" Press "),
        key("y"),
        Span::raw(" to confirm, "),
        key("n"),
        Span::raw(" or "),
        key("Esc"),
        Span::raw(" to cancel"),
    ])
}

fn draw_small_confirm(
    frame: &mut Frame,
    title: &'static str,
    lines: Vec<Line<'static>>,
    theme: &Theme,
) {
    let width = frame.area().width.min(76);
    let text = Paragraph::new(lines).wrap(Wrap { trim: false });
    let height = text.line_count(width.saturating_sub(2)).saturating_add(2);
    let area = crate::ui::dialog_rect(frame.area(), width, height.min(u16::MAX as usize) as u16);
    crate::ui::draw_modal_overlay(frame, area, theme);
    let text = text.block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.danger.to_color())),
    );
    frame.render_widget(text, area);
}

pub fn draw_confirm_quit(frame: &mut Frame, theme: &Theme) {
    draw_small_confirm(
        frame,
        " Confirm Quit ",
        vec![
            Line::from(""),
            Line::from(" Quit AMF?"),
            Line::from(""),
            confirm_prompt_line(theme),
        ],
        theme,
    );
}

pub fn draw_confirm_remove_session(
    frame: &mut Frame,
    state: &crate::app::RemoveSessionConfirm,
    theme: &Theme,
) {
    let (detail, color) = if state.agent_running {
        (
            " An agent is running in this session; removing it will stop the agent.",
            theme.warning.to_color(),
        )
    } else if state.window_live {
        (
            " This session is running; removing it will stop its process.",
            theme.warning.to_color(),
        )
    } else {
        (
            " This session is not running; its record will be removed.",
            theme.text_muted.to_color(),
        )
    };
    let closes_line = if state.closes_tmux_session {
        Line::from(Span::styled(
            " It is the last session, so the feature's tmux session will be closed.",
            Style::default().fg(theme.warning.to_color()),
        ))
    } else {
        Line::from("")
    };
    draw_small_confirm(
        frame,
        " Confirm Remove Session ",
        vec![
            Line::from(""),
            Line::from(vec![
                Span::raw(" Remove session "),
                Span::styled(
                    state.label.clone(),
                    Style::default()
                        .fg(theme.danger.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("?"),
            ]),
            Line::from(""),
            Line::from(Span::styled(detail, Style::default().fg(color))),
            closes_line,
            Line::from(""),
            confirm_prompt_line(theme),
        ],
        theme,
    );
}

pub fn draw_delete_feature_confirm(
    frame: &mut Frame,
    project_name: &str,
    feature_name: &str,
    theme: &Theme,
) {
    let width = frame.area().width.min(76);
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
            " This will stop all sessions and remove the worktree, if any.",
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

pub fn draw_deleting_feature_dialog(
    frame: &mut Frame,
    state: &DeletingFeatureState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let has_error = state.error.is_some();
    let area = if has_error {
        centered_rect(50, 46, frame.area())
    } else {
        centered_rect(50, 30, frame.area())
    };
    crate::ui::draw_modal_overlay(frame, area, theme);

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
            Constraint::Min(if has_error { 3 } else { 2 }),
            Constraint::Length(1),
            Constraint::Length(1),
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
    frame.render_widget(
        Paragraph::new(status_text).wrap(Wrap { trim: false }),
        chunks[0],
    );

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
    crate::ui::draw_modal_overlay(frame, area, theme);

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

    let mut agent_lines = vec![Line::from(Span::styled(" Harness:", agent_label_style))];

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
