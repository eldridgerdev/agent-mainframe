use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{App, AppMode, Selection, SessionFilter};
use crate::project::SessionKind;
use crate::theme::Theme;
use crate::usage::Model;

fn utilization_color(pct: f64, theme: &Theme) -> Color {
    if pct >= 80.0 {
        theme.usage_high.to_color()
    } else if pct >= 50.0 {
        theme.usage_medium.to_color()
    } else {
        theme.usage_low.to_color()
    }
}

fn usage_bar_spans<'a>(label: &'a str, pct: f64, bar_width: usize, theme: &Theme) -> Vec<Span<'a>> {
    let color = utilization_color(pct, theme);
    let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
    let empty = bar_width.saturating_sub(filled);

    vec![
        Span::styled(
            format!("{} ", label),
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("┃".repeat(filled), Style::default().fg(color)),
        Span::styled(
            "░".repeat(empty),
            Style::default().fg(theme.scrollbar.to_color()),
        ),
        Span::styled(
            format!(" {:.0}%", pct),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn shorten_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let key_style = || Style::default().fg(theme.warning.to_color());
    let hint_style = || Style::default().fg(theme.text_muted.to_color());

    let filter_spans = if app.session_filter != SessionFilter::All {
        vec![
            Span::styled(" [", hint_style()),
            Span::styled(
                app.session_filter.display_name(),
                Style::default().fg(theme.primary.to_color()),
            ),
            Span::styled("] ", hint_style()),
        ]
    } else {
        vec![]
    };

    let keybinds = match &app.mode {
        AppMode::Normal => {
            let on_session = matches!(app.selection, Selection::Session(_, _, _));
            let on_feature = matches!(app.selection, Selection::Feature(_, _));
            if on_session {
                let mut spans = filter_spans;
                spans.extend(vec![
                    Span::styled(" Enter", key_style()),
                    Span::raw(" view  "),
                    Span::styled("r", key_style()),
                    Span::raw(" rename  "),
                    Span::styled("x", key_style()),
                    Span::raw(" remove  "),
                    Span::styled("d", key_style()),
                    Span::raw(" delete  "),
                    Span::styled("s", key_style()),
                    Span::raw(" switch  "),
                    Span::styled("S", key_style()),
                    Span::raw(" resume  "),
                    Span::styled("f", key_style()),
                    Span::raw(" filter  "),
                    Span::styled("q", key_style()),
                    Span::raw(" quit"),
                ]);
                Line::from(spans)
            } else if on_feature {
                let mut spans = filter_spans;
                spans.extend(vec![
                    Span::styled(" n", key_style()),
                    Span::raw(" feature  "),
                    Span::styled("Enter", key_style()),
                    Span::raw(" expand  "),
                    Span::styled("c", key_style()),
                    Span::raw(" start  "),
                    Span::styled("x", key_style()),
                    Span::raw(" stop  "),
                    Span::styled("y", key_style()),
                    Span::raw(" ready  "),
                    Span::styled("f", key_style()),
                    Span::raw(" filter  "),
                    Span::styled("s", key_style()),
                    Span::raw(" switch  "),
                    Span::styled("S", key_style()),
                    Span::raw(" resume  "),
                    Span::styled("d", key_style()),
                    Span::raw(" delete  "),
                ]);
                if !app.active_extension.custom_sessions.is_empty() {
                    spans.push(Span::styled("p", key_style()));
                    spans.push(Span::raw(" sessions  "));
                }
                spans.extend(vec![Span::styled("q", key_style()), Span::raw(" quit")]);
                Line::from(spans)
            } else {
                let mut spans = filter_spans;
                spans.extend(vec![
                    Span::styled(" n", key_style()),
                    Span::raw(" feature  "),
                    Span::styled("N", key_style()),
                    Span::raw(" project  "),
                    Span::styled("Enter", key_style()),
                    Span::raw(" expand  "),
                    Span::styled("f", key_style()),
                    Span::raw(" filter  "),
                    Span::styled("d", key_style()),
                    Span::raw(" delete  "),
                    Span::styled("R", key_style()),
                    Span::raw(" refresh  "),
                    Span::styled("q", key_style()),
                    Span::raw(" quit"),
                ]);
                Line::from(spans)
            }
        }
        AppMode::CreatingProject(_)
        | AppMode::CreatingFeature(_)
        | AppMode::CreatingBatchFeatures(_)
        | AppMode::RenamingSession(_)
        | AppMode::RenamingFeature(_)
        | AppMode::BrowsingPath(_) => Line::from(vec![
            Span::styled("Enter", key_style()),
            Span::raw(" confirm  "),
            Span::styled("Esc", key_style()),
            Span::raw(" cancel"),
        ]),
        AppMode::DeletingProject(_) | AppMode::DeletingFeature(_, _) => Line::from(vec![
            Span::styled("y", key_style()),
            Span::raw(" confirm  "),
            Span::styled("n/Esc", key_style()),
            Span::raw(" cancel"),
        ]),
        AppMode::Help(_) => Line::from(vec![
            Span::styled("Esc/q/?", key_style()),
            Span::raw(" close help"),
        ]),
        AppMode::CommandPicker(_)
        | AppMode::NotificationPicker(_, _)
        | AppMode::SessionSwitcher(_)
        | AppMode::Searching(_)
        | AppMode::OpencodeSessionPicker(_)
        | AppMode::ClaudeSessionPicker(_)
        | AppMode::SessionPicker(_)
        | AppMode::BookmarkPicker(_) => Line::from(vec![
            Span::styled("j/k or \u{2191}/\u{2193}", key_style()),
            Span::raw(" navigate  "),
            Span::styled("Enter", key_style()),
            Span::raw(" select  "),
            Span::styled("Esc", key_style()),
            Span::raw(" cancel"),
        ]),
        AppMode::ConfirmingOpencodeSession { .. } | AppMode::ConfirmingClaudeSession { .. } => {
            Line::from(vec![
                Span::styled("y", key_style()),
                Span::raw(" restart  "),
                Span::styled("n/Esc", key_style()),
                Span::raw(" cancel"),
            ])
        }
        AppMode::ChangeReasonPrompt(_) => Line::from(vec![
            Span::styled("Enter", key_style()),
            Span::raw(" accept  "),
            Span::styled("Esc", key_style()),
            Span::raw(" skip  "),
            Span::styled("r", Style::default().fg(theme.danger.to_color())),
            Span::raw(" reject"),
        ]),
        AppMode::Viewing(_) => {
            let mut spans = vec![
                Span::styled("Ctrl+Space", key_style()),
                Span::raw(" commands  "),
                Span::styled("Ctrl+Q", key_style()),
                Span::raw(" exit view"),
            ];
            let labels = app.bookmark_status_labels();
            if !labels.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled("marks ", hint_style()));
                for (idx, label) in labels.iter().enumerate() {
                    spans.push(Span::styled(
                        label.clone(),
                        Style::default().fg(theme.info.to_color()),
                    ));
                    if idx + 1 < labels.len() {
                        spans.push(Span::raw(" "));
                    }
                }
            }
            Line::from(spans)
        }
        AppMode::RunningHook(state) => {
            if state.child.is_some() {
                Line::from(Span::styled(
                    "Running hook...",
                    Style::default().fg(theme.info.to_color()),
                ))
            } else {
                Line::from(vec![
                    Span::styled("Enter", key_style()),
                    Span::raw(" continue  "),
                    Span::styled("Esc", key_style()),
                    Span::raw(" skip"),
                ])
            }
        }
        AppMode::DeletingFeatureInProgress(state) => {
            if state.child.is_some() {
                Line::from(Span::styled(
                    "Deleting feature...",
                    Style::default().fg(theme.warning.to_color()),
                ))
            } else if state.error.is_some() {
                Line::from(vec![
                    Span::styled("Enter", key_style()),
                    Span::raw(" acknowledge"),
                ])
            } else {
                Line::from(Span::styled("Press any key to continue...", hint_style()))
            }
        }
        AppMode::HookPrompt(_) => Line::from(vec![
            Span::styled(" j/k", key_style()),
            Span::raw(" move  "),
            Span::styled("Enter", key_style()),
            Span::raw(" confirm  "),
            Span::styled("Esc", key_style()),
            Span::raw(" cancel"),
        ]),
        AppMode::LatestPrompt(_, _) => Line::from(vec![
            Span::styled(" Esc", key_style()),
            Span::styled("/q", key_style()),
            Span::raw(" close"),
        ]),
        AppMode::ForkingFeature(_) => Line::from(vec![
            Span::styled(" Enter", key_style()),
            Span::raw(" confirm  "),
            Span::styled("Esc", key_style()),
            Span::raw(" cancel"),
        ]),
        AppMode::ThemePicker(_) => Line::from(vec![
            Span::styled(" j/k", key_style()),
            Span::raw(" navigate  "),
            Span::styled("Enter", key_style()),
            Span::raw(" apply  "),
            Span::styled("Esc", key_style()),
            Span::raw(" cancel"),
        ]),
        AppMode::DebugLog(_) => Line::from(vec![
            Span::styled(" j/k", key_style()),
            Span::raw(" scroll  "),
            Span::styled("c", key_style()),
            Span::raw(" clear  "),
            Span::styled("Esc", key_style()),
            Span::raw(" close"),
        ]),
    };

    let message_line = if let Some(ref msg) = app.message {
        let color = if msg.starts_with("Error:") {
            theme.danger.to_color()
        } else {
            theme.success.to_color()
        };
        Line::from(Span::styled(msg.as_str(), Style::default().fg(color)))
    } else {
        match &app.selection {
            Selection::Project(pi) if *pi < app.store.projects.len() => {
                let project = &app.store.projects[*pi];
                Line::from(vec![
                    Span::styled(
                        format!(" {}", project.name),
                        Style::default()
                            .fg(theme.project_title.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", shorten_path(&project.repo)), hint_style()),
                ])
            }
            Selection::Feature(pi, fi)
                if *pi < app.store.projects.len()
                    && *fi < app.store.projects[*pi].features.len() =>
            {
                let feature = &app.store.projects[*pi].features[*fi];
                let branch_info = if feature.branch.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", feature.branch)
                };
                Line::from(vec![
                    Span::styled(
                        format!(" {}", feature.name),
                        Style::default()
                            .fg(theme.feature_title.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(branch_info, Style::default().fg(theme.warning.to_color())),
                    Span::styled(
                        format!("  {}", shorten_path(&feature.workdir)),
                        hint_style(),
                    ),
                ])
            }
            Selection::Session(pi, fi, si)
                if *pi < app.store.projects.len()
                    && *fi < app.store.projects[*pi].features.len()
                    && *si < app.store.projects[*pi].features[*fi].sessions.len() =>
            {
                let feature = &app.store.projects[*pi].features[*fi];
                let session = &feature.sessions[*si];
                let kind_label = match session.kind {
                    SessionKind::Claude => "claude",
                    SessionKind::Opencode => "opencode",
                    SessionKind::Codex => "codex",
                    SessionKind::Terminal => "terminal",
                    SessionKind::Nvim => "nvim",
                    SessionKind::Vscode => "vscode",
                    SessionKind::Custom => "custom",
                };
                Line::from(vec![
                    Span::styled(
                        format!(" {} ({})", session.label, kind_label),
                        Style::default()
                            .fg(theme.text.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", feature.name), hint_style()),
                    Span::styled(
                        format!("  {}", shorten_path(&feature.workdir)),
                        hint_style(),
                    ),
                ])
            }
            _ => {
                let project_count = app.store.projects.len();
                let feature_count: usize =
                    app.store.projects.iter().map(|p| p.features.len()).sum();
                Line::from(Span::styled(
                    format!(
                        " {} project{}, {} feature{}",
                        project_count,
                        if project_count == 1 { "" } else { "s" },
                        feature_count,
                        if feature_count == 1 { "" } else { "s" },
                    ),
                    hint_style(),
                ))
            }
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border.to_color()));
    let inner = block.inner(area);

    let status = Paragraph::new(vec![message_line, keybinds]).block(block);
    frame.render_widget(status, area);

    let usage = app.usage.get_data();
    let mut right_spans: Vec<Span> = Vec::new();

    let model_label = Span::styled(
        format!("[{}] ", usage.visible_model.label()),
        Style::default()
            .fg(theme.secondary.to_color())
            .add_modifier(Modifier::BOLD),
    );
    right_spans.push(model_label);

    match usage.visible_model {
        Model::Claude => {
            if let Some(pct5) = usage.claude.five_hour_pct {
                right_spans.extend(usage_bar_spans("5h", pct5, 15, theme));
                right_spans.push(Span::raw(" "));
            }

            if let Some(pct7) = usage.claude.seven_day_pct {
                right_spans.extend(usage_bar_spans("7d", pct7, 15, theme));
                right_spans.push(Span::raw(" "));
            } else if let Some(ref err) = usage.claude.last_error {
                right_spans.push(Span::styled(
                    format!("{} ", err),
                    Style::default().fg(theme.danger.to_color()),
                ));
                right_spans.push(Span::raw(" "));
            }

            right_spans.push(Span::styled(
                format!("{} msgs ", usage.claude.today_messages),
                hint_style(),
            ));

            if usage.claude.today_tokens > 0 {
                let tok = usage.claude.today_tokens;
                let tok_str = if tok >= 1_000_000 {
                    format!("{:.1}M tok ", tok as f64 / 1_000_000.0)
                } else if tok >= 1_000 {
                    format!("{:.1}K tok ", tok as f64 / 1_000.0)
                } else {
                    format!("{} tok ", tok)
                };
                right_spans.push(Span::styled(
                    tok_str,
                    Style::default().fg(theme.info.to_color()),
                ));
            }
        }
        Model::Codex => {
            if let Some(pct5) = usage.codex.five_hour_usage_pct {
                right_spans.extend(usage_bar_spans("5h", pct5, 15, theme));
                right_spans.push(Span::raw(" "));
            }

            if let Some(pct7) = usage.codex.weekly_usage_pct {
                right_spans.extend(usage_bar_spans("7d", pct7, 15, theme));
                right_spans.push(Span::raw(" "));
            } else if usage.codex.five_hour_tokens > 0 {
                right_spans.push(Span::styled(
                    format!("5h {} ", format_tokens(usage.codex.five_hour_tokens)),
                    Style::default().fg(theme.warning.to_color()),
                ));
            }

            if usage.codex.today_tokens > 0 {
                right_spans.push(Span::styled(
                    format!("{} tok ", format_tokens(usage.codex.today_tokens)),
                    Style::default().fg(theme.info.to_color()),
                ));
            }

            right_spans.push(Span::styled(
                format!("{} calls ", usage.codex.today_calls),
                hint_style(),
            ));
        }
        Model::Zai => {
            if let Some(pct) = usage.zai.five_hour_usage_pct {
                right_spans.extend(usage_bar_spans("5h", pct, 15, theme));
                right_spans.push(Span::raw(" "));
            }

            if let Some(pct) = usage.zai.weekly_usage_pct {
                right_spans.extend(usage_bar_spans("7d", pct, 15, theme));
                right_spans.push(Span::raw(" "));
            } else if usage.zai.today_tokens > 0 {
                right_spans.push(Span::styled(
                    format!("{} ", format_tokens(usage.zai.today_tokens)),
                    Style::default().fg(theme.info.to_color()),
                ));
            }

            right_spans.push(Span::styled(
                format!("{} calls ", usage.zai.today_calls),
                hint_style(),
            ));
        }
    }

    let right_width: u16 = right_spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()) as u16)
        .sum();
    let max_right_width = inner.width.saturating_sub(1);
    let right_width = right_width.min(max_right_width);
    if right_width == 0 {
        return;
    }
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
