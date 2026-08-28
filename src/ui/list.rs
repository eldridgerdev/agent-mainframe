use std::path::Path;

use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Selection, VisibleItem};
use crate::context_display::{ContextIndicator, format_context_indicator};
use crate::context_tracking::ContextBand;
use crate::custom_session_icons::resolve_custom_session_icon;
use crate::project::{ProjectStatus, SessionKind, VibeMode};
use crate::theme::Theme;
use crate::token_tracking::{
    aggregate_token_usage, format_feature_token_usage, provider_for_session_kind,
};

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

fn truncate_right_to_width(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }

    let content_width = width - 1;
    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > content_width {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push('…');
    result
}

/// Reserve room for the full context label without sacrificing the session
/// name entirely. At widths where both cannot be meaningful, the name wins.
fn fit_session_label(
    label: &str,
    prefix_width: usize,
    row_width: usize,
    indicator: Option<&ContextIndicator>,
) -> (String, bool) {
    const INDICATOR_GAP: usize = 2;
    const MIN_SESSION_NAME_WIDTH: usize = 3;

    let Some(indicator) = indicator else {
        return (label.to_string(), false);
    };
    let indicator_width = UnicodeWidthStr::width(indicator.text.as_str());
    let reserved = prefix_width + INDICATOR_GAP + indicator_width;
    if row_width < reserved + MIN_SESSION_NAME_WIDTH {
        return (label.to_string(), false);
    }
    (truncate_right_to_width(label, row_width - reserved), true)
}

fn context_indicator_style(indicator: &ContextIndicator, theme: &Theme) -> Style {
    let color = match indicator.band {
        ContextBand::Normal => theme.success.to_color(),
        ContextBand::Warning => theme.warning.to_color(),
        ContextBand::Critical => theme.danger.to_color(),
    };
    let modifier = if indicator.band == ContextBand::Normal {
        Modifier::empty()
    } else {
        Modifier::BOLD
    };
    Style::default().fg(color).add_modifier(modifier)
}

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height.saturating_sub(2) as usize;
    let theme = app.theme.clone();

    if app.store.projects.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled(
                " No projects yet. Press ",
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
    app.ensure_selection_visible_for_items(&visible, visible_height);

    let start = if visible.is_empty() {
        0
    } else {
        app.scroll_offset.min(visible.len() - 1)
    };
    let end_idx = app.visible_window_end_for_items(&visible, visible_height);
    let visible_slice = &visible[start..end_idx];

    let items: Vec<ListItem> = visible_slice
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let absolute_idx = start + idx;
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

                    if let Some(feature_name) = app.paused_plan_interview_for_project(&project.name)
                    {
                        spans.push(Span::styled(
                            format!("  [plan paused: {feature_name} · Enter to resume]"),
                            Style::default()
                                .fg(theme.warning.to_color())
                                .add_modifier(Modifier::BOLD),
                        ));
                    }

                    Line::from(spans)
                }
                VisibleItem::Feature(pi, fi) => {
                    let project = &app.store.projects[*pi];
                    let feature = &project.features[*fi];
                    let is_last_feature = !visible[absolute_idx + 1..].iter().any(|i| {
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
                    let attention = app.feature_attention(&feature.tmux_session);
                    let is_thinking = app.is_feature_thinking(&feature.tmux_session);
                    let is_being_deleted =
                        app.is_feature_being_deleted(&project.name, &feature.name);
                    let is_hook_running = app.is_hook_running(&feature.workdir);
                    let is_pending_worktree_script = feature.pending_worktree_script;
                    let status_dot = if is_pending_worktree_script {
                        Span::styled(
                            " ⚙ ",
                            Style::default()
                                .fg(theme.info.to_color())
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if is_being_deleted {
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
                    } else if let Some(state) = attention {
                        // Why the session stopped, when the harness could say.
                        Span::styled(
                            format!(" {} ", state.glyph(app.config.nerd_font)),
                            Style::default()
                                .fg(state.color(&theme))
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if is_waiting_for_input {
                        // Stopped, but nothing told us why — the pre-attention
                        // signal, kept for harnesses that report no lifecycle
                        // events at all.
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
                    } else if feature.ready {
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
                    } else if is_pending_worktree_script {
                        Style::default()
                            .fg(theme.info.to_color())
                            .add_modifier(Modifier::BOLD)
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
                    if let Some(pr) = app.active_pr_for_feature(&feature.id) {
                        // A background AI Review outlives the pane it was
                        // started from, so mirror the in-session PR badge's
                        // activity marker here (see `pr_triage_badge_span`) —
                        // the dashboard is where the user waits it out.
                        let ai_review_running = app.ai_review_running_for_workdir(&feature.workdir);
                        let mut label = match pr.unresolved_threads {
                            Some(0) => format!(" [PR #{} · 0 open", pr.number),
                            Some(count) => format!(" [PR #{} · {} open", pr.number, count),
                            None => format!(" [PR #{}", pr.number),
                        };
                        if ai_review_running {
                            label.push_str(" · AI review");
                        }
                        label.push(']');
                        let color = if ai_review_running {
                            theme.warning.to_color()
                        } else if pr.unresolved_threads == Some(0) {
                            theme.success.to_color()
                        } else {
                            theme.info.to_color()
                        };
                        line_spans.push(Span::styled(
                            label,
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ));
                    } else if let Some(pr) = app.terminal_pr_for_feature(&feature.id) {
                        // A branch with no open PR but a merged/closed one is
                        // finished work, not "no PR" — show that instead of
                        // nothing (see D3 in AMF_PLAN.md).
                        let (word, color) = match pr.state {
                            crate::github::TerminalPrState::Merged => {
                                (pr.state.label(), theme.pr_merged.to_color())
                            }
                            crate::github::TerminalPrState::Closed => {
                                (pr.state.label(), theme.pr_closed.to_color())
                            }
                        };
                        line_spans.push(Span::styled(
                            format!(" [PR #{} {word}]", pr.number),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ));
                    }
                    if let Some(usage) = aggregate_token_usage(
                        feature
                            .sessions
                            .iter()
                            .filter(|session| provider_for_session_kind(&session.kind).is_some())
                            .filter_map(|session| session.token_usage.as_ref()),
                    ) {
                        line_spans.push(Span::styled(
                            format!(
                                " [{}]",
                                format_feature_token_usage(&usage, &app.config.token_pricing)
                            ),
                            Style::default().fg(theme.status_detail.to_color()),
                        ));
                    }
                    line_spans.extend(mode_badge_spans);
                    if feature.review {
                        line_spans.push(Span::styled(
                            " [review]",
                            Style::default().fg(theme.mode_review.to_color()),
                        ));
                    }
                    if feature.plan_mode {
                        line_spans.push(Span::styled(
                            " [plan]",
                            Style::default().fg(theme.info.to_color()),
                        ));
                    }
                    if app.paused_plan_interview_for_feature(&feature.id) {
                        line_spans.push(Span::styled(
                            " [plan paused · Enter to resume]",
                            Style::default()
                                .fg(theme.warning.to_color())
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                    if feature.remote_control {
                        line_spans.push(Span::styled(
                            " [remote]",
                            Style::default().fg(theme.info.to_color()),
                        ));
                    }
                    if is_being_deleted {
                        line_spans.push(Span::styled(
                            " [deleting...]",
                            Style::default()
                                .fg(theme.danger.to_color())
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                    if is_pending_worktree_script {
                        line_spans.push(Span::styled(
                            " [running worktree script...]",
                            Style::default()
                                .fg(theme.info.to_color())
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

                    let is_last_feature = !visible[absolute_idx + 1..].iter().any(|i| {
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
                        SessionKind::Pi => Span::styled(
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
                                        c.icon_nerd
                                            .as_deref()
                                            .map(resolve_custom_session_icon)
                                            .or(c.icon.as_deref())
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
                        SessionKind::Todos => {
                            let icon = if app.config.nerd_font {
                                "\u{f0ae} "
                            } else {
                                "= "
                            };
                            Span::styled(
                                icon,
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

                    let context_indicator = session
                        .kind
                        .is_agent_harness()
                        .then(|| {
                            app.context_states
                                .get(&session.id)
                                .and_then(|state| state.snapshot.as_ref())
                                .map(format_context_indicator)
                        })
                        .flatten();
                    let prefix_width = UnicodeWidthStr::width(vert)
                        + UnicodeWidthStr::width(branch)
                        + UnicodeWidthStr::width(kind_icon.content.as_ref());
                    let row_width = usize::from(area.width.saturating_sub(2));
                    let (display_label, show_context_indicator) = fit_session_label(
                        &session.label,
                        prefix_width,
                        row_width,
                        context_indicator.as_ref(),
                    );
                    let mut main_spans = vec![
                        Span::styled(vert, Style::default().fg(muted)),
                        Span::styled(branch, Style::default().fg(muted)),
                        kind_icon,
                        Span::styled(display_label, name_style),
                    ];
                    if show_context_indicator && let Some(indicator) = context_indicator {
                        let style = context_indicator_style(&indicator, &theme);
                        main_spans.push(Span::raw("  "));
                        main_spans.push(Span::styled(indicator.text, style));
                    }
                    let main_line = Line::from(main_spans);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    use chrono::Utc;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    use crate::app::App;
    use crate::context_tracking::{
        ContextFreshness, ContextPercentage, ContextProvenance, ContextResetMetadata,
        SessionContextSnapshot, SessionContextState,
    };
    use crate::project::{AgentKind, Feature, FeatureSession, Project, ProjectStore};
    use crate::token_tracking::{SessionTokenUsage, TokenUsageProvider, TokenUsageSource};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};

    fn usage(
        provider: TokenUsageProvider,
        id: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        reasoning_tokens: u64,
    ) -> SessionTokenUsage {
        SessionTokenUsage {
            source: TokenUsageSource {
                provider,
                id: id.to_string(),
            },
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens,
            total_tokens: input_tokens
                .saturating_add(output_tokens)
                .saturating_add(cache_read_tokens)
                .saturating_add(cache_write_tokens)
                .saturating_add(reasoning_tokens),
        }
    }

    fn session(
        kind: SessionKind,
        label: &str,
        token_usage: Option<SessionTokenUsage>,
    ) -> FeatureSession {
        FeatureSession {
            id: format!("session-{label}"),
            kind,
            label: label.to_string(),
            tmux_window: label.to_ascii_lowercase().replace(' ', "-"),
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage,
        }
    }

    /// Mark a background AI Review as running for `workdir`, the way
    /// `App::ai_review_running_for_workdir` observes it.
    fn set_ai_review_running(app: &mut App, workdir: PathBuf) {
        let (_tx, rx) = std::sync::mpsc::channel();
        // Dropping the sender is fine: the badge only checks that the
        // background slot is occupied, it never reads progress here.
        app.ai_review_bg = Some(rx);
        app.ai_review_pending = Some(crate::app::AiReviewState {
            workdir,
            pr: crate::github::PrRef {
                number: 321,
                head_sha: "abc123".to_string(),
                url: "https://github.com/o/r/pull/321".to_string(),
                owner: "o".to_string(),
                repo: "r".to_string(),
                head_ref: "usage-feat".to_string(),
            },
            findings: Vec::new(),
            summary: None,
            attribution: None,
            selected: 0,
            detail_scroll: 0,
            detail_content_lines: 0,
            last_run: None,
            harness: None,
            harness_pick: None,
            harness_pick_origin: None,
            model: None,
            model_picked: false,
            model_pick: None,
            finding_editor: None,
            post_confirm: None,
        });
    }

    fn render_feature_row_with_pr(
        sessions: Vec<FeatureSession>,
        active_pr: Option<crate::app::ActivePrStatus>,
    ) -> String {
        render_feature_row_configured(sessions, active_pr, |_| {})
    }

    /// Render the single `usage-feat` feature row (workdir `/tmp/usage-feat`),
    /// letting `configure` adjust app state after the store is built.
    fn render_feature_row_configured(
        sessions: Vec<FeatureSession>,
        active_pr: Option<crate::app::ActivePrStatus>,
        configure: impl FnOnce(&mut App),
    ) -> String {
        let now = Utc::now();
        let feature = Feature {
            id: "feat-1".to_string(),
            name: "usage-feat".to_string(),
            branch: "usage-feat".to_string(),
            workdir: PathBuf::from("/tmp/usage-feat"),
            is_worktree: true,
            tmux_session: "amf-usage-feat".to_string(),
            sessions,
            collapsed: true,
            mode: VibeMode::default(),
            review: false,
            plan_mode: false,
            agent: AgentKind::Claude,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
            selected_plan_path: None,
            triage_source: None,
        };
        let store = ProjectStore {
            version: 5,
            projects: vec![Project {
                id: "proj-1".to_string(),
                name: "usage-project".to_string(),
                repo: PathBuf::from("/tmp/usage-project"),
                collapsed: false,
                features: vec![feature],
                created_at: now,
                preferred_agent: AgentKind::Claude,
                is_git: false,
            }],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        };
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.selection = crate::app::Selection::Feature(0, 0);
        if let Some(active_pr) = active_pr {
            app.active_prs.insert("feat-1".to_string(), active_pr);
        }
        configure(&mut app);

        let backend = TestBackend::new(140, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn render_feature_row(sessions: Vec<FeatureSession>) -> String {
        render_feature_row_with_pr(sessions, None)
    }

    fn context_snapshot(
        percentage: u8,
        band: ContextBand,
        provenance: ContextProvenance,
        freshness: ContextFreshness,
    ) -> SessionContextSnapshot {
        let now = Utc::now();
        SessionContextSnapshot {
            used_tokens: u64::from(percentage) * 1_000,
            context_limit: std::num::NonZeroU64::new(100_000).unwrap(),
            percentage: ContextPercentage::clamped(i64::from(percentage)),
            band,
            provenance,
            freshness,
            sampled_at: now,
            checked_at: now,
            reset: ContextResetMetadata::default(),
        }
    }

    fn render_session_row(
        kind: SessionKind,
        label: &str,
        snapshot: Option<SessionContextSnapshot>,
        width: u16,
    ) -> Buffer {
        let now = Utc::now();
        let session = session(kind, label, None);
        let session_id = session.id.clone();
        let feature = Feature {
            id: "feat-1".to_string(),
            name: "usage-feat".to_string(),
            branch: "usage-feat".to_string(),
            workdir: PathBuf::from("/tmp/usage-feat"),
            is_worktree: true,
            tmux_session: "amf-usage-feat".to_string(),
            sessions: vec![session],
            collapsed: false,
            mode: VibeMode::default(),
            review: false,
            plan_mode: false,
            agent: AgentKind::Claude,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
            selected_plan_path: None,
            triage_source: None,
        };
        let store = ProjectStore {
            version: 5,
            projects: vec![Project {
                id: "proj-1".to_string(),
                name: "usage-project".to_string(),
                repo: PathBuf::from("/tmp/usage-project"),
                collapsed: false,
                features: vec![feature],
                created_at: now,
                preferred_agent: AgentKind::Claude,
                is_git: false,
            }],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        };
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.selection = crate::app::Selection::Session(0, 0, 0);
        if let Some(snapshot) = snapshot {
            app.context_states.insert(
                session_id,
                SessionContextState {
                    reset: snapshot.reset.clone(),
                    snapshot: Some(snapshot),
                    awaiting_post_reset: false,
                },
            );
        }

        let backend = TestBackend::new(width, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn every_supported_agent_row_shows_its_context_indicator() {
        for kind in [
            SessionKind::Claude,
            SessionKind::Codex,
            SessionKind::Opencode,
            SessionKind::Pi,
        ] {
            let rendered = buffer_text(&render_session_row(
                kind.clone(),
                "Agent session",
                Some(context_snapshot(
                    64,
                    ContextBand::Normal,
                    ContextProvenance::Direct,
                    ContextFreshness::Fresh,
                )),
                60,
            ));
            assert!(
                rendered.contains("Ctx 64%"),
                "missing indicator for {kind:?}"
            );
        }
    }

    #[test]
    fn non_agent_rows_never_render_context_even_if_state_leaks_in() {
        for kind in [
            SessionKind::Terminal,
            SessionKind::Nvim,
            SessionKind::Vscode,
            SessionKind::Custom,
            SessionKind::Todos,
        ] {
            let rendered = buffer_text(&render_session_row(
                kind.clone(),
                "Utility session",
                Some(context_snapshot(
                    90,
                    ContextBand::Critical,
                    ContextProvenance::Direct,
                    ContextFreshness::Fresh,
                )),
                60,
            ));
            assert!(!rendered.contains("Ctx "), "indicator leaked onto {kind:?}");
        }
    }

    #[test]
    fn warning_critical_estimated_and_stale_labels_reach_the_row() {
        let warning = buffer_text(&render_session_row(
            SessionKind::Claude,
            "Claude",
            Some(context_snapshot(
                74,
                ContextBand::Warning,
                ContextProvenance::Estimated,
                ContextFreshness::Fresh,
            )),
            60,
        ));
        assert!(warning.contains("Ctx ~74% WARNING"));

        let critical_stale = buffer_text(&render_session_row(
            SessionKind::Codex,
            "Codex",
            Some(context_snapshot(
                91,
                ContextBand::Critical,
                ContextProvenance::Direct,
                ContextFreshness::Stale,
            )),
            60,
        ));
        assert!(critical_stale.contains("Ctx 91% CRITICAL STALE"));
    }

    #[test]
    fn pending_reset_has_no_old_percentage_to_render() {
        let rendered = buffer_text(&render_session_row(SessionKind::Pi, "Pi", None, 60));

        assert!(!rendered.contains("Ctx "));
    }

    #[test]
    fn narrow_rows_truncate_the_name_and_keep_the_critical_text() {
        let rendered = buffer_text(&render_session_row(
            SessionKind::Opencode,
            "A very long OpenCode session name",
            Some(context_snapshot(
                91,
                ContextBand::Critical,
                ContextProvenance::Estimated,
                ContextFreshness::Fresh,
            )),
            48,
        ));

        assert!(rendered.contains('…'));
        assert!(rendered.contains("Ctx ~91% CRITICAL"));
    }

    #[test]
    fn band_styles_use_healthy_warning_and_danger_colors() {
        let theme = Theme::default();
        for (band, expected) in [
            (ContextBand::Normal, theme.success.to_color()),
            (ContextBand::Warning, theme.warning.to_color()),
            (ContextBand::Critical, theme.danger.to_color()),
        ] {
            let indicator = ContextIndicator {
                text: "Ctx 85%".to_string(),
                band,
                stale: false,
            };
            let style = context_indicator_style(&indicator, &theme);
            assert_eq!(style.fg, Some(expected));
            assert_eq!(
                style.add_modifier.contains(Modifier::BOLD),
                band != ContextBand::Normal
            );
        }
    }

    #[test]
    fn feature_row_omits_usage_when_no_agent_usage_exists() {
        let rendered = render_feature_row(vec![session(SessionKind::Claude, "Claude 1", None)]);

        assert!(rendered.contains("usage-feat"));
        assert!(!rendered.contains("[usage "));
    }

    #[test]
    fn feature_row_shows_single_agent_usage() {
        let rendered = render_feature_row(vec![session(
            SessionKind::Claude,
            "Claude 1",
            Some(usage(
                TokenUsageProvider::Claude,
                "claude-1",
                10_000,
                2_000,
                5_000,
                1_000,
                0,
            )),
        )]);

        assert!(rendered.contains("[usage 21.8k eff · $0.07]"));
    }

    #[test]
    fn feature_row_sums_multiple_agent_usages_and_excludes_non_agents() {
        let rendered = render_feature_row(vec![
            session(
                SessionKind::Claude,
                "Claude 1",
                Some(usage(
                    TokenUsageProvider::Claude,
                    "claude-1",
                    10_000,
                    2_000,
                    5_000,
                    1_000,
                    0,
                )),
            ),
            session(
                SessionKind::Codex,
                "Codex 1",
                Some(usage(
                    TokenUsageProvider::Codex,
                    "codex-1",
                    1_000,
                    500,
                    0,
                    0,
                    1_000,
                )),
            ),
            session(
                SessionKind::Terminal,
                "Terminal",
                Some(usage(
                    TokenUsageProvider::Claude,
                    "terminal-ignored",
                    1_000_000,
                    1_000_000,
                    0,
                    0,
                    0,
                )),
            ),
        ]);

        assert!(rendered.contains("[usage 30.2k eff · $0.09]"));
        assert!(!rendered.contains("5.0M"));
    }

    #[test]
    fn feature_row_shows_active_pr_and_unresolved_thread_count() {
        let rendered = render_feature_row_with_pr(
            vec![],
            Some(crate::app::ActivePrStatus {
                branch: "usage-feat".to_string(),
                head_sha: "abc123".to_string(),
                number: 321,
                unresolved_threads: Some(4),
            }),
        );

        assert!(rendered.contains("[PR #321 · 4 open]"));
    }

    #[test]
    fn feature_row_marks_a_paused_on_demand_plan_interview() {
        let rendered = render_feature_row_configured(vec![], None, |app| {
            app.paused_plan_interview = Some(crate::app::PlanInterviewState::for_feature(
                "usage-feat".into(),
                "feat-1".into(),
                Vec::new(),
                PathBuf::from("/tmp/usage-feat"),
                AgentKind::Claude,
            ));
        });

        assert!(rendered.contains("[plan paused · Enter to resume]"));
    }

    #[test]
    fn project_row_marks_a_paused_pending_feature_interview() {
        let rendered = render_feature_row_configured(vec![], None, |app| {
            app.paused_plan_interview = Some(crate::app::PlanInterviewState::for_feature_creation(
                crate::app::PreparedFeatureLaunch {
                    project_name: "usage-project".into(),
                    branch: "planned-feature".into(),
                    workdir: PathBuf::from("/tmp/planned-feature"),
                    is_worktree: true,
                    mode: VibeMode::default(),
                    review: false,
                    plan_mode: true,
                    agent: AgentKind::Claude,
                    create_terminal: false,
                    session_name: "Claude 1".into(),
                    enable_chrome: false,
                    remote_control: false,
                    steering_enabled: false,
                    hook_succeeded: None,
                    startup_prompt: None,
                    todo_origin: None,
                },
                Vec::new(),
            ));
        });

        assert!(rendered.contains("[plan paused: planned-feature · Enter to resume]"));
    }

    fn pr_status() -> crate::app::ActivePrStatus {
        crate::app::ActivePrStatus {
            branch: "usage-feat".to_string(),
            head_sha: "abc123".to_string(),
            number: 321,
            unresolved_threads: Some(4),
        }
    }

    #[test]
    fn feature_row_marks_a_running_ai_review_on_its_own_pr_badge() {
        let rendered = render_feature_row_configured(vec![], Some(pr_status()), |app| {
            set_ai_review_running(app, PathBuf::from("/tmp/usage-feat"));
        });

        assert!(rendered.contains("[PR #321 · 4 open · AI review]"));
    }

    #[test]
    fn feature_row_omits_the_ai_review_marker_for_another_features_review() {
        let rendered = render_feature_row_configured(vec![], Some(pr_status()), |app| {
            set_ai_review_running(app, PathBuf::from("/tmp/other-feat"));
        });

        assert!(rendered.contains("[PR #321 · 4 open]"));
        assert!(!rendered.contains("AI review"));
    }

    #[test]
    fn feature_row_omits_the_ai_review_marker_once_generation_finishes() {
        // Completion clears the background slot before taking the pending
        // snapshot (`App::poll_ai_pr_review_bg`), so the marker must be gone
        // at the intermediate state, not just after both fields are empty.
        let rendered = render_feature_row_configured(vec![], Some(pr_status()), |app| {
            set_ai_review_running(app, PathBuf::from("/tmp/usage-feat"));
            app.ai_review_bg = None;
        });

        assert!(rendered.contains("[PR #321 · 4 open]"));
        assert!(!rendered.contains("AI review"));
    }
}
