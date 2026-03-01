use std::path::Path;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::app::{App, Selection, VisibleItem};
use crate::project::{ProjectStatus, SessionKind, VibeMode};

const RAINBOW_COLORS: &[Color] = &[
    Color::Red,
    Color::Rgb(255, 127, 0),
    Color::Yellow,
    Color::Green,
    Color::Cyan,
    Color::Blue,
    Color::Magenta,
];

pub fn rainbow_spans(text: &str) -> Vec<Span<'static>> {
    text.chars()
        .enumerate()
        .map(|(i, ch)| {
            let color = RAINBOW_COLORS[i % RAINBOW_COLORS.len()];
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(color)
                    .add_modifier(Modifier::BOLD),
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

const SELECTED_GRAY: Color = Color::Rgb(140, 140, 140);

pub fn draw(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
) {
    let visible_height = area.height.saturating_sub(2) as usize;
    app.ensure_selection_visible(visible_height);

    if app.store.projects.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::styled(
                "No projects yet. Press ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "N",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to create one.",
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(
            Block::default()
                .title(" Projects ")
                .borders(Borders::ALL),
        );
        frame.render_widget(empty, area);
        return;
    }

    let visible = app.visible_items();

    let start = app.scroll_offset;
    let end = (start + visible_height).min(visible.len());
    let visible_slice = &visible[start..end];

    let items: Vec<ListItem> = visible_slice
        .iter()
        .map(|item| {
            let is_selected =
                match (&app.selection, item) {
                    (
                        Selection::Project(a),
                        VisibleItem::Project(b),
                    ) => a == b,
                    (
                        Selection::Feature(a1, a2),
                        VisibleItem::Feature(b1, b2),
                    ) => a1 == b1 && a2 == b2,
                    (
                        Selection::Session(a1, a2, a3),
                        VisibleItem::Session(b1, b2, b3),
                    ) => a1 == b1 && a2 == b2 && a3 == b3,
                    _ => false,
                };

            let muted = if is_selected {
                SELECTED_GRAY
            } else {
                Color::DarkGray
            };

            let line = match item {
                VisibleItem::Project(pi) => {
                    let project =
                        &app.store.projects[*pi];
                    let collapse_icon =
                        if project.collapsed {
                            ">"
                        } else {
                            "v"
                        };

                    let mut spans = vec![
                        Span::styled(
                            format!(
                                " {} ",
                                collapse_icon
                            ),
                            Style::default().fg(muted),
                        ),
                        Span::styled(
                            &project.name,
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(
                                    Modifier::BOLD,
                                ),
                        ),
                        Span::styled(
                            format!(
                                "  {}",
                                shorten_path(&project.repo)
                            ),
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
                    let project =
                        &app.store.projects[*pi];
                    let feature = &project.features[*fi];
                    let is_last_feature =
                        *fi == project.features.len() - 1;

                    let connector = if is_last_feature {
                        "  └─"
                    } else {
                        "  ├─"
                    };

                    let is_waiting_for_input =
                        app.is_feature_waiting_for_input(&feature.name);
                    let is_thinking =
                        app.is_feature_thinking(&feature.tmux_session);
                    let status_dot = if is_waiting_for_input {
                        Span::styled(
                            " ? ",
                            Style::default()
                                .fg(Color::Rgb(255, 165, 0))
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if is_thinking {
                        let throbber = throbber_widgets_tui::Throbber::default()
                            .throbber_style(
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            )
                            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
                            .use_type(throbber_widgets_tui::WhichUse::Spin);
                        let mut span = throbber.to_symbol_span(&app.throbber_state);
                        span.content = format!(" {} ", span.content).into();
                        span
                    } else {
                        match feature.status {
                            ProjectStatus::Active => {
                                Span::styled(
                                    " ● ",
                                    Style::default()
                                        .fg(Color::Green),
                                )
                            }
                            ProjectStatus::Idle => {
                                Span::styled(
                                    " ○ ",
                                    Style::default()
                                        .fg(Color::Yellow),
                                )
                            }
                            ProjectStatus::Stopped => {
                                Span::styled(
                                    " ■ ",
                                    Style::default()
                                        .fg(Color::Red),
                                )
                            }
                        }
                    };

                    let collapse_icon =
                        if feature.sessions.is_empty() {
                            " "
                        } else if feature.collapsed {
                            ">"
                        } else {
                            "v"
                        };

                    let name_style = if is_selected {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    let session_count =
                        feature.sessions.len();
                    let badge = if session_count > 0 {
                        format!(" [{}]", session_count)
                    } else {
                        String::new()
                    };

                    let mode_badge_spans: Vec<Span> = match feature.mode {
                        VibeMode::Vibeless => vec![Span::styled(
                            " [vibeless]",
                            Style::default()
                                .fg(Color::Green),
                        )],
                        VibeMode::Vibe => vec![Span::styled(
                            " [vibe]",
                            Style::default()
                                .fg(Color::Yellow),
                        )],
                        VibeMode::SuperVibe => {
                            let mut spans = vec![Span::raw(" [")];
                            spans.extend(rainbow_spans("supervibe"));
                            spans.push(Span::raw("]"));
                            spans
                        }
                        VibeMode::Review => vec![Span::styled(
                            " [review]",
                            Style::default().fg(Color::Magenta),
                        )],
                    };

                    let has_pending_input =
                        app.pending_inputs.iter().any(|p| {
                            p.project_name.as_deref()
                                == Some(&project.name)
                                && p.feature_name.as_deref()
                                    == Some(&feature.name)
                                && p.notification_type
                                    != "diff-review"
                        });

                    let mut line_spans = vec![
                        Span::styled(
                            connector,
                            Style::default().fg(muted),
                        ),
                        status_dot,
                        Span::styled(
                            format!("{} ", collapse_icon),
                            Style::default().fg(muted),
                        ),
                        Span::styled(
                            &feature.name,
                            name_style,
                        ),
                    ];
                    line_spans.extend(mode_badge_spans);
                    line_spans.push(Span::styled(
                        badge,
                        Style::default().fg(muted),
                    ));
                    if has_pending_input {
                        line_spans.push(Span::styled(
                            " ?",
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                    line_spans.push(Span::styled(
                        format!(
                            "  {}",
                            shorten_path(&feature.workdir)
                        ),
                        Style::default().fg(muted),
                    ));
                    Line::from(line_spans)
                }
                VisibleItem::Session(pi, fi, si) => {
                    let project =
                        &app.store.projects[*pi];
                    let feature = &project.features[*fi];
                    let session =
                        &feature.sessions[*si];

                    let is_last_feature =
                        *fi == project.features.len() - 1;
                    let is_last_session =
                        *si == feature.sessions.len() - 1;

                    let vert = if is_last_feature {
                        "  "
                    } else {
                        "  │"
                    };
                    let branch = if is_last_session {
                        "   └─ "
                    } else {
                        "   ├─ "
                    };

                    let kind_icon = match session.kind {
                        SessionKind::Claude => {
                            Span::styled(
                                "* ",
                                Style::default()
                                    .fg(Color::Magenta),
                            )
                        }
                        SessionKind::Opencode => {
                            Span::styled(
                                "* ",
                                Style::default()
                                    .fg(Color::Cyan),
                            )
                        }
                        SessionKind::Terminal => {
                            Span::styled(
                                "> ",
                                Style::default()
                                    .fg(Color::Green),
                            )
                        }
                        SessionKind::Nvim => {
                            let icon =
                                if app.config.nerd_font {
                                    "\u{e6ae} "
                                } else {
                                    "~ "
                                };
                            Span::styled(
                                icon,
                                Style::default()
                                    .fg(Color::Cyan),
                            )
                        }
                        SessionKind::Custom => {
                            Span::styled(
                                "$ ",
                                Style::default()
                                    .fg(Color::Yellow),
                            )
                        }
                    };

                    let name_style = if is_selected {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    Line::from(vec![
                        Span::styled(
                            vert,
                            Style::default().fg(muted),
                        ),
                        Span::styled(
                            branch,
                            Style::default().fg(muted),
                        ),
                        kind_icon,
                        Span::styled(
                            &session.label,
                            name_style,
                        ),
                    ])
                }
            };

            if is_selected {
                ListItem::new(line).style(
                    Style::default().bg(Color::DarkGray),
                )
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Projects ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::White)),
    );

    frame.render_widget(list, area);
}
