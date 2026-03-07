use std::path::Path;

use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::app::{App, Selection, VisibleItem};
use crate::project::{ProjectStatus, SessionKind, VibeMode};
use crate::theme::Theme;

fn format_age(dt: DateTime<Utc>) -> String {
    let secs = Utc::now().signed_duration_since(dt).num_seconds();
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 7 * 86400 {
        format!("{}d ago", secs / 86400)
    } else {
        dt.format("%b %d").to_string()
    }
}

pub fn rainbow_spans(text: &str, theme: &Theme) -> Vec<Span<'static>> {
    let colors = [
        theme.danger.to_color(),
        theme.warning.to_color(),
        theme.success.to_color(),
        theme.primary.to_color(),
        theme.info.to_color(),
        theme.secondary.to_color(),
    ];
    text.chars()
        .enumerate()
        .map(|(i, ch)| {
            let color = colors[i % colors.len()];
            Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
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

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height.saturating_sub(2) as usize;
    app.ensure_selection_visible(visible_height);

    let theme = app.theme.clone();

    if app.store.projects.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled(
                "No projects yet. Press ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled(
                "N",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to create one.",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]))
        .block(
            Block::default()
                .title(Span::styled(
                    " Projects ",
                    Style::default()
                        .fg(theme.primary.to_color())
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border.to_color())),
        );
        frame.render_widget(empty, area);
        return;
    }

    let visible = app.visible_items();

    let start = app.scroll_offset;
    let end_idx = (start + visible_height).min(visible.len());
    let visible_slice = &visible[start..end_idx];

    let items: Vec<ListItem> = visible_slice
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let is_selected = match (&app.selection, item) {
                (Selection::Project(a), VisibleItem::Project(b)) => a == b,
                (Selection::Feature(a1, a2), VisibleItem::Feature(b1, b2)) => a1 == b1 && a2 == b2,
                (Selection::Session(a1, a2, a3), VisibleItem::Session(b1, b2, b3)) => {
                    a1 == b1 && a2 == b2 && a3 == b3
                }
                _ => false,
            };

            let muted = if is_selected {
                theme.text.to_color()
            } else {
                theme.text_muted.to_color()
            };

            let line = match item {
                VisibleItem::Project(pi) => {
                    let project = &app.store.projects[*pi];
                    let collapse_icon = if project.collapsed { ">" } else { "v" };

                    let mut spans = vec![
                        Span::styled(format!(" {} ", collapse_icon), Style::default().fg(muted)),
                        Span::styled(
                            &project.name,
                            Style::default()
                                .fg(theme.project_title.to_color())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  {}", shorten_path(&project.repo)),
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
                    let project = &app.store.projects[*pi];
                    let feature = &project.features[*fi];
                    let is_last_feature = !visible_slice[idx + 1..].iter().any(|i| {
                        matches!(
                            i,
                            VisibleItem::Feature(p, _)
                                if *p == *pi
                        )
                    });

                    let connector = if is_last_feature {
                        "  └─"
                    } else {
                        "  ├─"
                    };

                    let is_waiting_for_input = app.is_feature_waiting_for_input(&feature.name);
                    let is_thinking = app.is_feature_thinking(&feature.tmux_session);
                    let is_being_deleted =
                        app.is_feature_being_deleted(&project.name, &feature.name);
                    let is_hook_running = app.is_hook_running(&feature.workdir);
                    let status_dot = if is_being_deleted {
                        let throbber = throbber_widgets_tui::Throbber::default()
                            .throbber_style(
                                Style::default()
                                    .fg(theme.danger.to_color())
                                    .add_modifier(Modifier::BOLD),
                            )
                            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
                            .use_type(throbber_widgets_tui::WhichUse::Spin);
                        let mut span = throbber.to_symbol_span(&app.throbber_state);
                        span.content = format!(" {} ", span.content).into();
                        span
                    } else if is_hook_running {
                        let throbber = throbber_widgets_tui::Throbber::default()
                            .throbber_style(
                                Style::default()
                                    .fg(theme.info.to_color())
                                    .add_modifier(Modifier::BOLD),
                            )
                            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
                            .use_type(throbber_widgets_tui::WhichUse::Spin);
                        let mut span = throbber.to_symbol_span(&app.throbber_state);
                        span.content = format!(" {} ", span.content).into();
                        span
                    } else if is_waiting_for_input {
                        Span::styled(
                            " ? ",
                            Style::default()
                                .fg(theme.status_waiting.to_color())
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if is_thinking {
                        let throbber = throbber_widgets_tui::Throbber::default()
                            .throbber_style(
                                Style::default()
                                    .fg(theme.primary.to_color())
                                    .add_modifier(Modifier::BOLD),
                            )
                            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
                            .use_type(throbber_widgets_tui::WhichUse::Spin);
                        let mut span = throbber.to_symbol_span(&app.throbber_state);
                        span.content = format!(" {} ", span.content).into();
                        span
                    } else {
                        if feature.ready {
                            Span::styled(
                                " ✓ ",
                                Style::default()
                                    .fg(theme.success.to_color())
                                    .add_modifier(Modifier::BOLD),
                            )
                        } else {
                            match feature.status {
                                ProjectStatus::Active => Span::styled(
                                    " ● ",
                                    Style::default().fg(theme.status_active.to_color()),
                                ),
                                ProjectStatus::Idle => Span::styled(
                                    " ○ ",
                                    Style::default().fg(theme.status_idle.to_color()),
                                ),
                                ProjectStatus::Stopped => Span::styled(
                                    " ■ ",
                                    Style::default().fg(theme.status_stopped.to_color()),
                                ),
                            }
                        }
                    };

                    let collapse_icon = if feature.sessions.is_empty() {
                        " "
                    } else if feature.collapsed {
                        ">"
                    } else {
                        "v"
                    };

                    let name_style = if is_being_deleted {
                        Style::default()
                            .fg(theme.text_muted.to_color())
                            .add_modifier(Modifier::CROSSED_OUT)
                    } else if is_selected {
                        Style::default()
                            .fg(theme.feature_title.to_color())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.feature_title.to_color())
                    };

                    let session_count = feature.sessions.len();
                    let badge = if session_count > 0 {
                        format!(" [{}]", session_count)
                    } else {
                        String::new()
                    };

                    let mode_badge_spans: Vec<Span> = match feature.mode {
                        VibeMode::Vibeless => vec![Span::styled(
                            " [vibeless]",
                            Style::default().fg(theme.mode_vibeless.to_color()),
                        )],
                        VibeMode::Vibe => vec![Span::styled(
                            " [vibe]",
                            Style::default().fg(theme.mode_vibe.to_color()),
                        )],
                        VibeMode::SuperVibe => {
                            let mut spans = vec![Span::raw(" [")];
                            spans.extend(rainbow_spans("supervibe", &theme));
                            spans.push(Span::raw("]"));
                            spans
                        }
                        VibeMode::Review => vec![Span::styled(
                            " [review]",
                            Style::default().fg(theme.mode_review.to_color()),
                        )],
                    };

                    let has_pending_input = app.pending_inputs.iter().any(|p| {
                        p.project_name.as_deref() == Some(&project.name)
                            && p.feature_name.as_deref() == Some(&feature.name)
                            && p.notification_type != "diff-review"
                    });

                    let display_name = feature.nickname.as_ref().unwrap_or(&feature.name);
                    let mut line_spans = vec![
                        Span::styled(connector, Style::default().fg(muted)),
                        status_dot,
                        Span::styled(format!("{} ", collapse_icon), Style::default().fg(muted)),
                        Span::styled(display_name, name_style),
                    ];
                    if !feature.is_worktree {
                        line_spans.push(Span::styled(
                            " [repo]",
                            Style::default()
                                .fg(theme.warning.to_color())
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                    if feature.nickname.is_some() {
                        line_spans.push(Span::styled(
                            format!(" ({})", feature.branch),
                            Style::default().fg(theme.text_muted.to_color()),
                        ));
                    }
                    line_spans.extend(mode_badge_spans);
                    if is_being_deleted {
                        line_spans.push(Span::styled(
                            " [deleting...]",
                            Style::default()
                                .fg(theme.danger.to_color())
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                    if is_hook_running {
                        line_spans.push(Span::styled(
                            " [hook running...]",
                            Style::default()
                                .fg(theme.info.to_color())
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                    line_spans.push(Span::styled(
                        format!(" {}", format_age(feature.created_at)),
                        Style::default().fg(theme.warning.to_color()),
                    ));
                    line_spans.push(Span::styled(badge, Style::default().fg(muted)));
                    if has_pending_input {
                        line_spans.push(Span::styled(
                            " ?",
                            Style::default()
                                .fg(theme.warning.to_color())
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                    line_spans.push(Span::styled(
                        format!("  {}", shorten_path(&feature.workdir)),
                        Style::default().fg(muted),
                    ));
                    if app.summary_state.generating.contains(&feature.tmux_session) {
                        let throbber = throbber_widgets_tui::Throbber::default()
                            .throbber_style(Style::default().fg(theme.warning.to_color()))
                            .throbber_set(throbber_widgets_tui::CLOCK)
                            .use_type(throbber_widgets_tui::WhichUse::Spin);
                        let mut span = throbber.to_symbol_span(&app.throbber_state);
                        span.content = format!(" — {}", span.content).into();
                        line_spans.push(span);
                    } else if let Some(summary) = &feature.summary {
                        line_spans.push(Span::styled(
                            format!(" — {}", summary),
                            Style::default().fg(theme.warning.to_color()),
                        ));
                    }
                    Line::from(line_spans)
                }
                VisibleItem::Session(pi, fi, si) => {
                    let project = &app.store.projects[*pi];
                    let feature = &project.features[*fi];
                    let session = &feature.sessions[*si];

                    let is_last_feature = !visible_slice[idx + 1..].iter().any(|i| {
                        matches!(
                            i,
                            VisibleItem::Feature(p, _)
                                if *p == *pi
                        )
                    });
                    let is_last_session = *si == feature.sessions.len() - 1;

                    let vert = if is_last_feature { "  " } else { "  │" };
                    let branch = if is_last_session {
                        "   └─ "
                    } else {
                        "   ├─ "
                    };

                    let kind_icon = match session.kind {
                        SessionKind::Claude => Span::styled(
                            "* ",
                            Style::default().fg(theme.session_icon_claude.to_color()),
                        ),
                        SessionKind::Opencode => Span::styled(
                            "* ",
                            Style::default().fg(theme.session_icon_opencode.to_color()),
                        ),
                        SessionKind::Codex => Span::styled(
                            "* ",
                            Style::default().fg(theme.session_icon_codex.to_color()),
                        ),
                        SessionKind::Terminal => Span::styled(
                            "> ",
                            Style::default().fg(theme.session_icon_terminal.to_color()),
                        ),
                        SessionKind::Nvim => {
                            let icon = if app.config.nerd_font {
                                "\u{e6ae} "
                            } else {
                                "~ "
                            };
                            Span::styled(
                                icon,
                                Style::default().fg(theme.session_icon_nvim.to_color()),
                            )
                        }
                        SessionKind::Vscode => {
                            let icon = if app.config.nerd_font {
                                "\u{E70C} "
                            } else {
                                "V "
                            };
                            Span::styled(
                                icon,
                                Style::default().fg(theme.session_icon_vscode.to_color()),
                            )
                        }
                        SessionKind::Custom => {
                            let cfg = app
                                .active_extension
                                .custom_sessions
                                .iter()
                                .find(|c| c.name == session.label);
                            let raw = cfg
                                .and_then(|c| {
                                    if app.config.nerd_font {
                                        c.icon_nerd.as_deref().or(c.icon.as_deref())
                                    } else {
                                        c.icon.as_deref()
                                    }
                                })
                                .unwrap_or("$");
                            Span::styled(
                                format!("{} ", raw),
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

                    let main_line = Line::from(vec![
                        Span::styled(vert, Style::default().fg(muted)),
                        Span::styled(branch, Style::default().fg(muted)),
                        kind_icon,
                        Span::styled(&session.label, name_style),
                    ]);

                    if let Some(ref text) = session.status_text {
                        let status_vert = if is_last_feature { "  " } else { "  │" };
                        let status_pad = if is_last_session {
                            "       "
                        } else {
                            "   │   "
                        };
                        let status_line = Line::from(vec![
                            Span::styled(status_vert, Style::default().fg(muted)),
                            Span::styled(status_pad, Style::default().fg(muted)),
                            Span::styled(
                                text.as_str(),
                                Style::default().fg(theme.status_detail.to_color()),
                            ),
                        ]);
                        return if is_selected {
                            ListItem::new(vec![main_line, status_line])
                                .style(Style::default().bg(theme.effective_selection_bg()))
                        } else {
                            ListItem::new(vec![main_line, status_line])
                        };
                    }

                    main_line
                }
            };

            if is_selected {
                ListItem::new(line).style(Style::default().bg(theme.effective_selection_bg()))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(Span::styled(
                " Projects ",
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border.to_color())),
    );

    frame.render_widget(list, area);
}
