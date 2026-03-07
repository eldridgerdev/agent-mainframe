use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use crate::app::{
    ClaudeSessionPickerState, CommandPickerState, OpencodeSessionPickerState, PendingInput,
    SessionPickerState, SessionSwitcherState,
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
    frame.render_widget(Clear, area);

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
    frame.render_widget(Clear, area);

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
        let line = Line::from(vec![
            Span::styled("    /", Style::default().fg(theme.text_muted.to_color())),
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
        Span::styled(" send  ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
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
    frame.render_widget(Clear, area);

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
    frame.render_widget(Clear, area);

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

    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, session)| {
            let is_selected = i == state.selected;
            let title_preview = if session.title.len() > 60 {
                format!("{}...", &session.title[..57])
            } else {
                session.title.clone()
            };

            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "  > " } else { "    " },
                    Style::default().fg(theme.primary.to_color()),
                ),
                Span::styled(
                    title_preview,
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
    frame.render_widget(Clear, area);

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

    let items: Vec<ListItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, session)| {
            let is_selected = i == state.selected;
            let title_preview = if session.title.len() > 60 {
                format!("{}...", &session.title[..57])
            } else {
                session.title.clone()
            };

            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "  > " } else { "    " },
                    Style::default().fg(theme.success.to_color()),
                ),
                Span::styled(
                    title_preview,
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

pub fn draw_claude_session_confirm(frame: &mut Frame, theme: &Theme) {
    let area = centered_rect(50, 35, frame.area());
    frame.render_widget(Clear, area);

    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Feature is already running.",
            Style::default().fg(theme.warning.to_color()),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Restart with selected claude session?",
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(Span::styled(
            "  This will kill the current tmux session",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            "  and start a new one with the session restored.",
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
    );

    frame.render_widget(text, area);
}

pub fn draw_opencode_session_confirm(frame: &mut Frame, theme: &Theme) {
    let area = centered_rect(50, 35, frame.area());
    frame.render_widget(Clear, area);

    let text = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Feature is already running.",
            Style::default().fg(theme.warning.to_color()),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Restart with selected opencode session?",
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(Span::styled(
            "  This will kill the current tmux session",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            "  and start a new one with the session restored.",
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
    );

    frame.render_widget(text, area);
}

pub fn draw_session_picker(
    frame: &mut Frame,
    state: &SessionPickerState,
    nerd_font: bool,
    theme: &Theme,
) {
    let area = centered_rect(55, 50, frame.area());
    frame.render_widget(Clear, area);

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
                items.push(item.style(Style::default().bg(theme.effective_selection_bg())));
            } else {
                items.push(item);
            }
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
        Span::styled(" start  ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[1]);
}
