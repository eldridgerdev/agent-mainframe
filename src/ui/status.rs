use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::{App, AppMode, Selection};
use crate::project::SessionKind;
use crate::usage::Provider;

fn utilization_color(pct: f64) -> Color {
    if pct >= 80.0 {
        Color::Red
    } else if pct >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn usage_bar_spans<'a>(
    label: &'a str,
    pct: f64,
    bar_width: usize,
) -> Vec<Span<'a>> {
    let color = utilization_color(pct);
    let filled =
        ((pct / 100.0) * bar_width as f64).round() as usize;
    let empty = bar_width.saturating_sub(filled);

    vec![
        Span::styled(
            format!("{} ", label),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "┃".repeat(filled),
            Style::default().fg(color),
        ),
        Span::styled(
            "░".repeat(empty),
            Style::default().fg(Color::Rgb(60, 60, 60)),
        ),
        Span::styled(
            format!(" {:.0}%", pct),
            Style::default()
                .fg(color)
                .add_modifier(Modifier::BOLD),
        ),
    ]
}

fn shorten_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

pub fn draw(
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
                        "S",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" opencode  "),
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
                        "S",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" opencode  "),
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
        | AppMode::SessionSwitcher(_)
        | AppMode::OpencodeSessionPicker(_) => Line::from(vec![
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
        AppMode::ConfirmingOpencodeSession { .. } => Line::from(vec![
            Span::styled(
                "y",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" restart  "),
            Span::styled(
                "n/Esc",
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

    let message_line = if let Some(ref msg) = app.message {
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
                    SessionKind::Opencode => "opencode",
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

    let usage = app.usage.get_data();
    let mut right_spans: Vec<Span> = Vec::new();

    let provider_label = Span::styled(
        format!("[{}] ", usage.visible_provider.label()),
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    );
    right_spans.push(provider_label);

    match usage.visible_provider {
        Provider::Claude => {
            if let Some(pct5) = usage.claude.five_hour_pct {
                right_spans.extend(usage_bar_spans("5h", pct5, 15));
                right_spans.push(Span::raw(" "));
            }

            if let Some(pct7) = usage.claude.seven_day_pct {
                right_spans.extend(usage_bar_spans("7d", pct7, 15));
                right_spans.push(Span::raw(" "));
            } else if let Some(ref err) = usage.claude.last_error {
                right_spans.push(Span::styled(
                    format!("{} ", err),
                    Style::default().fg(Color::Red),
                ));
                right_spans.push(Span::raw(" "));
            }

            right_spans.push(Span::styled(
                format!("{} msgs ", usage.claude.today_messages),
                Style::default().fg(Color::DarkGray),
            ));
        }
        Provider::Opencode => {
            let format_tokens = |n: u64| {
                if n >= 1_000_000 {
                    format!("{:.1}M", n as f64 / 1_000_000.0)
                } else if n >= 1_000 {
                    format!("{:.1}K", n as f64 / 1_000.0)
                } else {
                    n.to_string()
                }
            };

            if let Some(pct) = usage.opencode.five_hour_usage_pct {
                right_spans.extend(usage_bar_spans("5h", pct, 15));
                right_spans.push(Span::raw(" "));
            }

            if let Some(pct) = usage.opencode.weekly_usage_pct {
                right_spans.extend(usage_bar_spans("7d", pct, 15));
                right_spans.push(Span::raw(" "));
            } else if usage.opencode.zai_today_tokens > 0 {
                right_spans.push(Span::styled(
                    format!(
                        "zai:{} ",
                        format_tokens(usage.opencode.zai_today_tokens)
                    ),
                    Style::default().fg(Color::Cyan),
                ));
            }

            right_spans.push(Span::styled(
                format!(
                    "in:{} out:{} ",
                    format_tokens(usage.opencode.today_input_tokens),
                    format_tokens(usage.opencode.today_output_tokens)
                ),
                Style::default().fg(Color::DarkGray),
            ));

            right_spans.push(Span::styled(
                format!("{} msgs ", usage.opencode.today_messages),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    let right_width: u16 = right_spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()) as u16)
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
