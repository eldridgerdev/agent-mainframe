use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{
    BookmarkPickerState, ClaudeSessionPickerState, CodexSessionPickerState, CommandAction,
    CommandPickerState, MarkdownFilePickerState, OpencodeSessionPickerState, PendingInput,
    SessionPickerState, SessionSwitcherState, SyntaxLanguagePickerState, SyntaxOperationAction,
};
use crate::project::SessionKind;
use crate::theme::Theme;

use super::dashboard::centered_rect;

pub fn draw_notification_picker(
    frame: &mut Frame,
    pending: &[PendingInput],
    selected: usize,
    theme: &Theme,
) {
    let area = centered_rect(60, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = format!(" Input Requests ({}) ", pending.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.warning.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if pending.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No pending input requests.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    let items: Vec<ListItem> = pending
        .iter()
        .enumerate()
        .map(|(i, input)| {
            let is_selected = i == selected;

            let proj = input.project_name.as_deref().unwrap_or("unknown");
            let feat = input.feature_name.as_deref().unwrap_or("unknown");

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
                        .fg(theme.project_title.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("/ {} ", feat),
                    Style::default().fg(theme.feature_title.to_color()),
                ),
                Span::styled(
                    format!("- {}", msg_preview),
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

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k or \u{2191}/\u{2193}",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " select  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("x", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " delete  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_command_picker(frame: &mut Frame, state: &CommandPickerState, theme: &Theme) {
    let area = centered_rect(50, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = format!(" Custom Commands ({}) ", state.commands.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.commands.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No custom commands found.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    let mut items: Vec<ListItem> = Vec::new();
    let mut current_source = String::new();

    for (i, cmd) in state.commands.iter().enumerate() {
        if cmd.source != current_source {
            if !current_source.is_empty() {
                items.push(ListItem::new(Line::from("")));
            }
            current_source = cmd.source.clone();
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  {} Commands", cmd.source),
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ))));
        }

        let is_selected = i == state.selected;
        let prefix = match cmd.action {
            CommandAction::SlashCommand => "/",
            CommandAction::Local { .. } => "*",
            CommandAction::CodexLiveDemo(_) => "*",
        };
        let line = Line::from(vec![
            Span::styled(
                format!("    {prefix}"),
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled(
                &cmd.name,
                if is_selected {
                    Style::default()
                        .fg(theme.text.to_color())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text.to_color())
                },
            ),
        ]);

        if is_selected {
            items.push(
                ListItem::new(line).style(Style::default().bg(theme.effective_selection_bg())),
            );
        } else {
            items.push(ListItem::new(line));
        }
    }

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k or \u{2191}/\u{2193}",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(" run  ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_syntax_language_picker(
    frame: &mut Frame,
    state: &SyntaxLanguagePickerState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let installed = state
        .languages
        .iter()
        .filter(|row| {
            matches!(
                row.status,
                crate::highlight::HighlightInstallState::Installed
            )
        })
        .count();
    let area = centered_rect(68, 58, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = format!(" Syntax Parsers ({}/{}) ", installed, state.languages.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.info.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(inner);

    let status_line = if let Some(operation) = &state.operation {
        let verb = match operation.action {
            SyntaxOperationAction::Install => "Installing",
            SyntaxOperationAction::Uninstall => "Removing",
        };
        let throbber = throbber_widgets_tui::Throbber::default()
            .throbber_style(
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            )
            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
            .use_type(throbber_widgets_tui::WhichUse::Spin);
        let spinner = throbber.to_symbol_span(throbber_state);
        let detail = operation
            .last_output
            .clone()
            .unwrap_or_else(|| "Working...".to_string());
        let elapsed = operation.started_at.elapsed().as_secs();
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!("  {} {}  ", verb, operation.language.picker_title()),
                    Style::default()
                        .fg(theme.warning.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
                spinner,
                Span::styled(
                    format!("  {}s elapsed", elapsed),
                    Style::default().fg(theme.text_muted.to_color()),
                ),
            ]),
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(detail, Style::default().fg(theme.text_muted.to_color())),
            ]),
        ])
    } else if let Some(notice) = &state.notice {
        let color = if notice.starts_with("Error:") {
            theme.danger.to_color()
        } else {
            theme.success.to_color()
        };
        Paragraph::new(Line::from(Span::styled(
            format!("  {}", notice),
            Style::default().fg(color),
        )))
    } else {
        Paragraph::new(Line::from(Span::styled(
            "  Install only the tree-sitter parsers this workspace actually needs.",
            Style::default().fg(theme.text_muted.to_color()),
        )))
    };
    frame.render_widget(status_line, chunks[0]);

    let items: Vec<ListItem> = state
        .languages
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let is_selected = i == state.selected;
            let is_operating = state
                .operation
                .as_ref()
                .is_some_and(|op| op.language == row.language);

            let (badge_label, badge_style) = if is_operating {
                (
                    match state.operation.as_ref().map(|op| op.action) {
                        Some(SyntaxOperationAction::Install) => " INSTALLING ",
                        Some(SyntaxOperationAction::Uninstall) => " REMOVING ",
                        None => " WORKING ",
                    },
                    Style::default()
                        .fg(theme.effective_bg())
                        .bg(theme.warning.to_color())
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                match row.status {
                    crate::highlight::HighlightInstallState::Installed => (
                        " INSTALLED ",
                        Style::default()
                            .fg(theme.effective_bg())
                            .bg(theme.success.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    crate::highlight::HighlightInstallState::Available => (
                        " AVAILABLE ",
                        Style::default()
                            .fg(theme.effective_bg())
                            .bg(theme.text_muted.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    crate::highlight::HighlightInstallState::Broken => (
                        " BROKEN ",
                        Style::default()
                            .fg(theme.effective_bg())
                            .bg(theme.danger.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                }
            };

            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "  > " } else { "    " },
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(badge_label, badge_style),
                Span::styled(" ", Style::default()),
                Span::styled(
                    row.language.picker_title(),
                    if is_selected {
                        Style::default()
                            .fg(theme.text.to_color())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.text.to_color())
                    },
                ),
                Span::styled(
                    format!("  {}  ", row.language.extension_summary()),
                    Style::default().fg(theme.primary.to_color()),
                ),
                Span::styled(
                    row.language.picker_description(),
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

    frame.render_widget(List::new(items), chunks[1]);

    let hints = if state.operation.is_some() {
        Paragraph::new(Line::from(vec![
            Span::styled("  wait", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " for the current parser operation to finish",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  j/k or \u{2191}/\u{2193}",
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::styled(
                " navigate  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("Enter/i", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " install or reinstall  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("x", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " uninstall  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("r", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " refresh  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
        ]))
    };
    frame.render_widget(hints, chunks[2]);
}

pub fn draw_markdown_file_picker(
    frame: &mut Frame,
    state: &MarkdownFilePickerState,
    theme: &Theme,
) {
    let visible_indices: Vec<usize> = state
        .files
        .iter()
        .enumerate()
        .filter(|(_, path)| {
            !state.plan_only
                || crate::markdown::markdown_view_relative_label(
                    path,
                    &state.workdir,
                    state.repo_root.as_deref(),
                )
                .to_ascii_lowercase()
                .contains("plan")
        })
        .map(|(idx, _)| idx)
        .collect();
    let visible_count = visible_indices.len();
    let selected_visible = visible_indices
        .iter()
        .position(|&idx| idx == state.selected)
        .or_else(|| (!visible_indices.is_empty()).then_some(0));
    let showing_repo_root = state.repo_root.is_some();
    let area = if showing_repo_root {
        centered_rect(70, 60, frame.area())
    } else {
        centered_rect(62, 52, frame.area())
    };
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = if showing_repo_root {
        format!(
            " Markdown Files: Worktree + Repo Root ({}/{}) ",
            visible_count,
            state.files.len()
        )
    } else {
        format!(" Markdown Files ({}/{}) ", visible_count, state.files.len())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.info.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if showing_repo_root {
            vec![
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ]
        } else {
            vec![Constraint::Min(1), Constraint::Length(2)]
        })
        .split(inner);

    if showing_repo_root {
        let legend = Paragraph::new(Line::from(vec![
            Span::styled(
                "  WORKTREE",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " = current feature dir    ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled(
                "REPO ROOT",
                Style::default()
                    .fg(theme.info.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " = main repo dir",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]));
        frame.render_widget(legend, chunks[0]);
    }

    let list_chunk = if showing_repo_root {
        chunks[1]
    } else {
        chunks[0]
    };
    let hint_chunk = if showing_repo_root {
        chunks[2]
    } else {
        chunks[1]
    };

    let items: Vec<ListItem> = visible_indices
        .iter()
        .map(|&idx| {
            let path = &state.files[idx];
            let is_selected = idx == state.selected;
            let scope = crate::markdown::markdown_view_scope(
                path,
                &state.workdir,
                state.repo_root.as_deref(),
            );
            let scope_label = match scope {
                crate::markdown::MarkdownViewScope::Worktree => " WORKTREE ",
                crate::markdown::MarkdownViewScope::RepoRoot => " REPO ROOT ",
                crate::markdown::MarkdownViewScope::Other => " PATH ",
            };
            let scope_style = match scope {
                crate::markdown::MarkdownViewScope::Worktree => Style::default()
                    .fg(theme.effective_bg())
                    .bg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
                crate::markdown::MarkdownViewScope::RepoRoot => Style::default()
                    .fg(theme.effective_bg())
                    .bg(theme.info.to_color())
                    .add_modifier(Modifier::BOLD),
                crate::markdown::MarkdownViewScope::Other => Style::default()
                    .fg(theme.effective_bg())
                    .bg(theme.text_muted.to_color())
                    .add_modifier(Modifier::BOLD),
            };
            let label = crate::markdown::markdown_view_relative_label(
                path,
                &state.workdir,
                state.repo_root.as_deref(),
            );
            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "  > " } else { "    " },
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(scope_label, scope_style),
                Span::styled(" ", Style::default().fg(theme.text_muted.to_color())),
                Span::styled(
                    label,
                    if is_selected {
                        Style::default()
                            .fg(theme.text.to_color())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.text.to_color())
                    },
                ),
            ]);
            if is_selected {
                ListItem::new(line).style(Style::default().bg(theme.effective_selection_bg()))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    if items.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No markdown files match the current filter.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
        frame.render_widget(empty, list_chunk);
    } else {
        let list = List::new(items);
        let mut list_state = ListState::default();
        list_state.select(selected_visible);
        frame.render_stateful_widget(list, list_chunk, &mut list_state);
    }

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k or \u{2191}/\u{2193}",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(" open  ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("p", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            if state.plan_only {
                " all-files  "
            } else {
                " plan-only  "
            },
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, hint_chunk);
}

pub fn draw_bookmark_picker(
    frame: &mut Frame,
    state: &BookmarkPickerState,
    rows: &[String],
    theme: &Theme,
) {
    let area = centered_rect(56, 42, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Harpoon Bookmarks ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    if rows.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No bookmarks yet. Use leader+m on a session.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
        frame.render_widget(empty, chunks[0]);
        let hints = Paragraph::new(Line::from(vec![
            Span::styled("  Esc", Style::default().fg(theme.warning.to_color())),
            Span::styled(" close", Style::default().fg(theme.text_muted.to_color())),
        ]));
        frame.render_widget(hints, chunks[1]);
        return;
    }

    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let is_selected = i == state.selected;
            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "  > " } else { "    " },
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(
                    row.clone(),
                    if is_selected {
                        Style::default()
                            .fg(theme.text.to_color())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.text.to_color())
                    },
                ),
            ]);

            if is_selected {
                ListItem::new(line).style(Style::default().bg(theme.effective_selection_bg()))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    frame.render_widget(List::new(items), chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k or \u{2191}/\u{2193}",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(" jump  ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("d", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " remove  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("1-9", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " quick jump  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" close", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_session_switcher(
    frame: &mut Frame,
    state: &SessionSwitcherState,
    nerd_font: bool,
    theme: &Theme,
) {
    let area = centered_rect(40, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = format!(" {} / {} ", state.project_name, state.feature_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.sessions.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No sessions.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = i == state.selected;
            let is_current = entry.tmux_window == state.return_window;

            let icon = match entry.kind {
                SessionKind::Claude => Span::styled(
                    "  * ",
                    Style::default().fg(theme.session_icon_claude.to_color()),
                ),
                SessionKind::Opencode => Span::styled(
                    "  * ",
                    Style::default().fg(theme.session_icon_opencode.to_color()),
                ),
                SessionKind::Codex => Span::styled(
                    "  * ",
                    Style::default().fg(theme.session_icon_codex.to_color()),
                ),
                SessionKind::Terminal => Span::styled(
                    "  > ",
                    Style::default().fg(theme.session_icon_terminal.to_color()),
                ),
                SessionKind::Nvim => {
                    let icon = if nerd_font { "  \u{e6ae} " } else { "  ~ " };
                    Span::styled(
                        icon,
                        Style::default().fg(theme.session_icon_nvim.to_color()),
                    )
                }
                SessionKind::Vscode => {
                    let icon = if nerd_font { "  \u{E70C} " } else { "  V " };
                    Span::styled(
                        icon,
                        Style::default().fg(theme.session_icon_vscode.to_color()),
                    )
                }
                SessionKind::Custom => {
                    let raw = if nerd_font {
                        entry
                            .icon_nerd
                            .as_deref()
                            .or(entry.icon.as_deref())
                            .unwrap_or("$")
                    } else {
                        entry.icon.as_deref().unwrap_or("$")
                    };
                    Span::styled(
                        format!("  {} ", raw),
                        Style::default().fg(theme.session_icon_custom.to_color()),
                    )
                }
            };

            let name_style = if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };

            let mut spans = vec![icon, Span::styled(&entry.label, name_style)];

            if is_current {
                spans.push(Span::styled(
                    " (current)",
                    Style::default().fg(theme.text_muted.to_color()),
                ));
            }

            let line = Line::from(spans);
            if is_selected {
                ListItem::new(line).style(Style::default().bg(theme.effective_selection_bg()))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k or \u{2191}/\u{2193}",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " select  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("s", Style::default().fg(theme.warning.to_color())),
        Span::styled(" new  ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("r", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " rename  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_opencode_session_picker(
    frame: &mut Frame,
    state: &OpencodeSessionPickerState,
    theme: &Theme,
) {
    let area = centered_rect(60, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = format!(" Opencode Sessions ({}) ", state.sessions.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.sessions.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No sessions for this worktree.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);
    let title_width = inner.width.saturating_sub(4) as usize;

    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, session)| {
            let is_selected = i == state.selected;
            let title_style = if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let lines: Vec<Line> = wrap_text_to_width(&session.title, title_width)
                .into_iter()
                .enumerate()
                .map(|(line_idx, chunk)| {
                    Line::from(vec![
                        Span::styled(
                            if line_idx == 0 {
                                if is_selected { "  > " } else { "    " }
                            } else {
                                "    "
                            },
                            Style::default().fg(theme.primary.to_color()),
                        ),
                        Span::styled(chunk, title_style),
                    ])
                })
                .collect();

            if is_selected {
                ListItem::new(lines).style(Style::default().bg(theme.effective_selection_bg()))
            } else {
                ListItem::new(lines)
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k or \u{2191}/\u{2193}",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " select  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_claude_session_picker(
    frame: &mut Frame,
    state: &ClaudeSessionPickerState,
    theme: &Theme,
) {
    let area = centered_rect(60, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = format!(" Claude Sessions ({}) ", state.sessions.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.success.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.sessions.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No sessions for this worktree.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);
    let title_width = inner.width.saturating_sub(4) as usize;

    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, session)| {
            let is_selected = i == state.selected;
            let title_style = if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let lines: Vec<Line> = wrap_text_to_width(&session.title, title_width)
                .into_iter()
                .enumerate()
                .map(|(line_idx, chunk)| {
                    Line::from(vec![
                        Span::styled(
                            if line_idx == 0 {
                                if is_selected { "  > " } else { "    " }
                            } else {
                                "    "
                            },
                            Style::default().fg(theme.success.to_color()),
                        ),
                        Span::styled(chunk, title_style),
                    ])
                })
                .collect();

            if is_selected {
                ListItem::new(lines).style(Style::default().bg(theme.effective_selection_bg()))
            } else {
                ListItem::new(lines)
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k or \u{2191}/\u{2193}",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " select  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_codex_session_picker(
    frame: &mut Frame,
    state: &CodexSessionPickerState,
    theme: &Theme,
) {
    let area = centered_rect(60, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = format!(" Codex Sessions ({}) ", state.sessions.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.session_icon_codex.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.sessions.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No sessions for this worktree.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);
    let title_width = inner.width.saturating_sub(4) as usize;

    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, session)| {
            let is_selected = i == state.selected;
            let title_style = if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let lines: Vec<Line> = wrap_text_to_width(&session.title, title_width)
                .into_iter()
                .enumerate()
                .map(|(line_idx, chunk)| {
                    Line::from(vec![
                        Span::styled(
                            if line_idx == 0 {
                                if is_selected { "  > " } else { "    " }
                            } else {
                                "    "
                            },
                            Style::default().fg(theme.session_icon_codex.to_color()),
                        ),
                        Span::styled(chunk, title_style),
                    ])
                })
                .collect();

            if is_selected {
                ListItem::new(lines).style(Style::default().bg(theme.effective_selection_bg()))
            } else {
                ListItem::new(lines)
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k or \u{2191}/\u{2193}",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " select  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[1]);
}

fn draw_session_restore_confirm(frame: &mut Frame, theme: &Theme, agent_name: &str) {
    let area = centered_rect(62, 35, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Feature is already running.",
            Style::default().fg(theme.warning.to_color()),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Restart with selected {agent_name} session?"),
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(Span::styled(
            "  This will kill the current tmux session and start",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            "  a new one with the session restored.",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  y", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " restart  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("n/Esc", Style::default().fg(theme.warning.to_color())),
            Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
        ]),
    ])
    .block(
        Block::default()
            .title(" Confirm Restart ")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.warning.to_color())),
    )
    .wrap(Wrap { trim: false });

    frame.render_widget(text, area);
}

fn wrap_text_to_width(text: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let word_len = word.chars().count();

        if current.is_empty() {
            if word_len <= max_chars {
                current.push_str(word);
            } else {
                let mut chunk = String::new();
                for ch in word.chars() {
                    chunk.push(ch);
                    if chunk.chars().count() == max_chars {
                        lines.push(std::mem::take(&mut chunk));
                    }
                }
                current = chunk;
            }
            continue;
        }

        let projected_len = current.chars().count() + 1 + word_len;
        if projected_len <= max_chars {
            current.push(' ');
            current.push_str(word);
            continue;
        }

        lines.push(std::mem::take(&mut current));
        if word_len <= max_chars {
            current.push_str(word);
        } else {
            let mut chunk = String::new();
            for ch in word.chars() {
                chunk.push(ch);
                if chunk.chars().count() == max_chars {
                    lines.push(std::mem::take(&mut chunk));
                }
            }
            current = chunk;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

pub fn draw_claude_session_confirm(frame: &mut Frame, theme: &Theme) {
    draw_session_restore_confirm(frame, theme, "claude");
}

pub fn draw_codex_session_confirm(frame: &mut Frame, theme: &Theme) {
    draw_session_restore_confirm(frame, theme, "codex");
}

pub fn draw_opencode_session_confirm(frame: &mut Frame, theme: &Theme) {
    draw_session_restore_confirm(frame, theme, "opencode");
}

pub fn draw_session_picker(
    frame: &mut Frame,
    state: &SessionPickerState,
    nerd_font: bool,
    theme: &Theme,
) {
    let area = centered_rect(55, 60, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let total = state.builtin_sessions.len() + state.custom_sessions.len();
    let title = format!(" Start Session ({}) ", total);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if total == 0 {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No sessions available.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_item_idx: Option<usize> = None;

    if !state.builtin_sessions.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  Built-in Sessions",
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ))));

        for (i, session) in state.builtin_sessions.iter().enumerate() {
            let idx = i;
            let is_selected = idx == state.selected;
            let is_disabled = session.disabled.is_some();

            let icon = match session.kind {
                crate::project::SessionKind::Claude => Span::styled(
                    "  * ",
                    Style::default().fg(theme.session_icon_claude.to_color()),
                ),
                crate::project::SessionKind::Opencode => Span::styled(
                    "  * ",
                    Style::default().fg(theme.session_icon_opencode.to_color()),
                ),
                crate::project::SessionKind::Codex => Span::styled(
                    "  * ",
                    Style::default().fg(theme.session_icon_codex.to_color()),
                ),
                crate::project::SessionKind::Terminal => Span::styled(
                    "  > ",
                    Style::default().fg(theme.session_icon_terminal.to_color()),
                ),
                crate::project::SessionKind::Nvim => {
                    let icon = if nerd_font { "  \u{e6ae} " } else { "  ~ " };
                    Span::styled(
                        icon,
                        Style::default().fg(theme.session_icon_nvim.to_color()),
                    )
                }
                crate::project::SessionKind::Vscode => Span::styled(
                    "  V ",
                    Style::default().fg(theme.session_icon_vscode.to_color()),
                ),
                _ => Span::styled("    ", Style::default().fg(theme.text_muted.to_color())),
            };

            let (label_style, msg) = if is_disabled {
                (
                    Style::default().fg(theme.text_muted.to_color()),
                    session.disabled.as_ref(),
                )
            } else if is_selected {
                (
                    Style::default()
                        .fg(theme.text.to_color())
                        .add_modifier(Modifier::BOLD),
                    None,
                )
            } else {
                (Style::default().fg(theme.text.to_color()), None)
            };

            let mut spans = vec![
                if is_selected && !is_disabled {
                    Span::styled("  > ", Style::default().fg(theme.warning.to_color()))
                } else {
                    Span::styled("    ", Style::default().fg(theme.text_muted.to_color()))
                },
                icon,
                Span::styled(&session.label, label_style),
            ];

            if let Some(reason) = msg {
                spans.push(Span::styled(
                    format!(" ({})", reason),
                    Style::default().fg(theme.danger.to_color()),
                ));
            }

            let line = Line::from(spans);

            if is_selected && !is_disabled {
                selected_item_idx = Some(items.len());
                items.push(
                    ListItem::new(line).style(Style::default().bg(theme.effective_selection_bg())),
                );
            } else {
                items.push(ListItem::new(line));
            }
        }
    }

    if !state.custom_sessions.is_empty() {
        if !items.is_empty() {
            items.push(ListItem::new(Line::from("")));
        }

        items.push(ListItem::new(Line::from(Span::styled(
            "  Custom Sessions",
            Style::default()
                .fg(theme.secondary.to_color())
                .add_modifier(Modifier::BOLD),
        ))));

        let builtin_len = state.builtin_sessions.len();
        for (i, cfg) in state.custom_sessions.iter().enumerate() {
            let idx = builtin_len + i;
            let is_selected = idx == state.selected;

            let name_style = if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };

            let raw_icon = if nerd_font {
                cfg.icon_nerd
                    .as_deref()
                    .or(cfg.icon.as_deref())
                    .unwrap_or("$")
            } else {
                cfg.icon.as_deref().unwrap_or("$")
            };
            let icon_str = format!("  {} ", raw_icon);

            let mut lines: Vec<Line> = vec![Line::from(vec![
                if is_selected {
                    Span::styled("  > ", Style::default().fg(theme.warning.to_color()))
                } else {
                    Span::styled("    ", Style::default().fg(theme.text_muted.to_color()))
                },
                Span::styled(icon_str, Style::default().fg(theme.secondary.to_color())),
                Span::styled(&cfg.name, name_style),
            ])];

            let subtitle = cfg.description.as_deref().or(cfg.command.as_deref());
            if let Some(text) = subtitle {
                let preview = if text.len() > 50 {
                    format!("{}...", &text[..47])
                } else {
                    text.to_string()
                };
                lines.push(Line::from(Span::styled(
                    format!("      {}", preview),
                    Style::default().fg(if is_selected {
                        theme.text.to_color()
                    } else {
                        theme.text_muted.to_color()
                    }),
                )));
            }

            let item = ListItem::new(lines);
            if is_selected {
                selected_item_idx = Some(items.len());
                items.push(item.style(Style::default().bg(theme.effective_selection_bg())));
            } else {
                items.push(item);
            }
        }
    }

    let list = List::new(items);
    let mut list_state = ListState::default();
    list_state.select(selected_item_idx);
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "  j/k or \u{2191}/\u{2193}",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled(
            " navigate  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(" start  ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[1]);
}
