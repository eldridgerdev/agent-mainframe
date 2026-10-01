use crate::{
    app::model_analysis::{AdviceScope, State, Status},
    model_evidence::{Priority, Recommendation},
    theme::Theme,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Cell, Padding, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, TableState, Wrap,
    },
};

struct AdviceView<'a> {
    status: &'a Status,
    selected: usize,
    scroll: u16,
    show_sources: bool,
    scope: AdviceScope,
    checking_setting: bool,
    message: Option<&'a str>,
}

pub fn draw_model_analysis(frame: &mut Frame, state: &State, message: Option<&str>, theme: &Theme) {
    draw(
        frame,
        AdviceView {
            status: &state.status,
            selected: state.selected,
            scroll: state.scroll,
            show_sources: state.show_sources,
            scope: state.scope(),
            checking_setting: state.is_checking_setting(),
            message,
        },
        theme,
    );
}

fn dialog_area(size: Rect, sources: bool) -> Rect {
    let width = if size.width < 8 {
        size.width
    } else {
        size.width.saturating_sub(4).min(120)
    };
    let height = if size.height < 8 {
        size.height
    } else {
        size.height
            .saturating_sub(2)
            .min(if sources { 34 } else { 21 })
    };
    Rect::new(
        size.x + size.width.saturating_sub(width) / 2,
        size.y + size.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn panel<'a>(title: &'a str, theme: &Theme) -> Block<'a> {
    Block::default()
        .title(format!(" {title} "))
        .title_style(Style::default().fg(theme.text_muted.to_color()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border.to_color()))
        .padding(Padding::horizontal(1))
}

fn draw(frame: &mut Frame, view: AdviceView<'_>, theme: &Theme) {
    let area = dialog_area(frame.area(), view.show_sources);
    crate::ui::draw_modal_overlay(frame, area, theme);
    let base = Style::default()
        .fg(theme.text.to_color())
        .bg(theme.effective_bg());
    let block = panel("Model & reasoning", theme)
        .title_style(
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.border_focus.to_color()))
        .style(base);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let compact = inner.height < 10;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if compact { 0 } else { 3 }),
            Constraint::Min(0),
            Constraint::Length(if view.message.is_some() && !compact {
                2
            } else {
                0
            }),
            Constraint::Length(if compact { 1 } else { 2 }),
        ])
        .split(inner);
    let muted = Style::default().fg(theme.text_muted.to_color());
    let (phase, scope) = match view.scope {
        AdviceScope::ExistingSession => (
            "Existing session",
            "Advice for this harness. Change settings in its own model picker.",
        ),
        AdviceScope::ExistingPlan => (
            "Reviewed plan → implementation",
            "Advice only. Accept the plan separately; change settings in the harness's picker.",
        ),
        AdviceScope::HostTodoPlan => (
            "TODO plan → implementation",
            "Advice only. Accept separately to start the TODO agent; use its model picker.",
        ),
        AdviceScope::InitialLaunch => (
            "Plan → implementation",
            "Apply a setting, then accept the plan to start the agent.",
        ),
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                phase,
                Style::default()
                    .fg(theme.secondary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(scope, muted),
        ])
        .wrap(Wrap { trim: false }),
        chunks[0],
    );
    match view.status {
        Status::Ready(choices) => draw_choices(frame, chunks[1], choices, &view, theme),
        _ => draw_status(frame, chunks[1], &view, theme),
    }
    if let Some(message) = view.message {
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(theme.warning.to_color()))
                .wrap(Wrap { trim: false }),
            chunks[2],
        );
    }
    frame.render_widget(Paragraph::new(footer(&view, inner.width, theme)), chunks[3]);
}

fn draw_choices(
    frame: &mut Frame,
    area: Rect,
    choices: &[Recommendation],
    view: &AdviceView<'_>,
    theme: &Theme,
) {
    let table_height = (choices.len() as u16 * 2 + 4).min(area.height.saturating_sub(3));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(table_height), Constraint::Min(0)])
        .split(area);
    let muted = Style::default().fg(theme.text_muted.to_color());
    let rows = choices.iter().enumerate().map(|(i, recommendation)| {
        let selected = i == view.selected;
        let choice = &recommendation.choice;
        let (focus, color) = match recommendation.priority {
            Priority::Speed => ("Speed", theme.success.to_color()),
            Priority::Balance => ("Balanced", theme.info.to_color()),
            Priority::Depth => ("Depth", theme.secondary.to_color()),
        };
        Row::new(vec![
            Cell::from(if selected { "›" } else { " " })
                .style(Style::default().fg(theme.primary.to_color())),
            Cell::from(Text::from(vec![
                Line::styled(
                    choice.model(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Line::styled(choice.harness().display_name(), muted),
            ])),
            Cell::from(choice.reasoning().unwrap_or("default"))
                .style(Style::default().fg(theme.secondary.to_color())),
            Cell::from(focus).style(Style::default().fg(color)),
        ])
        .height(2)
        .style(if selected {
            Style::default()
                .bg(theme.effective_selection_bg())
                .fg(theme.text.to_color())
        } else {
            Style::default().fg(theme.text.to_color())
        })
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(9),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(["", "MODEL", "EFFORT", "FOCUS"])
            .style(muted)
            .bottom_margin(1),
    )
    .column_spacing(2)
    .block(panel("Available settings", theme));
    frame.render_stateful_widget(
        table,
        chunks[0],
        &mut TableState::default().with_selected(Some(view.selected)),
    );
    if let Some(selected) = choices.get(view.selected) {
        if view.show_sources {
            draw_sources(frame, chunks[1], selected, view.scroll, theme);
        } else {
            let lines = vec![
                Line::styled(
                    if *selected.choice.harness() == crate::project::AgentKind::Claude
                        && matches!(selected.priority, Priority::Speed)
                    {
                        "Favor speed / fewer tokens"
                    } else {
                        selected.priority.label()
                    },
                    Style::default()
                        .fg(theme.text.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Line::styled(
                    "Provider guidance; task-specific quality, time and tokens are unknown.",
                    muted,
                ),
                Line::from(vec![
                    Span::styled(
                        "s",
                        Style::default()
                            .fg(theme.primary.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  Read {} research sources", selected.evidence.len()),
                        muted,
                    ),
                ]),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[1]);
        }
    }
}

fn draw_sources(
    frame: &mut Frame,
    area: Rect,
    selected: &Recommendation,
    scroll: u16,
    theme: &Theme,
) {
    let muted = Style::default().fg(theme.text_muted.to_color());
    let mut lines = vec![];
    for (i, note) in selected.evidence.iter().enumerate() {
        let title = if note.source.ends_with("/reasoning") || note.source.ends_with("/effort") {
            "Reasoning effort"
        } else if note.source.ends_with("/model-selection")
            || note.source.ends_with("/model-config")
        {
            "Model selection"
        } else if note.source.ends_with("/gpt-6.1-sol") {
            "GPT-6.1 Sol"
        } else {
            "Research source"
        };
        lines.push(Line::styled(
            format!("{}. {title}", i + 1),
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(note.statement.clone()));
        lines.push(Line::styled(
            note.source.clone(),
            Style::default().fg(theme.secondary.to_color()),
        ));
        lines.push(Line::styled(
            format!(
                "Checked {} · expires {}",
                note.checked_at.date_naive(),
                note.expires_at.date_naive()
            ),
            muted,
        ));
        lines.push(Line::from(""));
    }
    let block = panel("Research · PgUp/PgDn to scroll", theme);
    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = paragraph.line_count(inner.width);
    let offset = usize::from(scroll).min(total.saturating_sub(usize::from(inner.height)));
    frame.render_widget(
        paragraph
            .block(block)
            .scroll((offset.min(u16::MAX as usize) as u16, 0)),
        area,
    );
    if total > usize::from(inner.height) && area.width > 2 {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .style(muted),
            area,
            &mut ScrollbarState::new(total)
                .position(offset)
                .viewport_content_length(usize::from(inner.height)),
        );
    }
}

fn draw_status(frame: &mut Frame, area: Rect, view: &AdviceView<'_>, theme: &Theme) {
    let (title, detail, color) = match view.status {
        Status::Loading if view.checking_setting => (
            "Checking selected setting…",
            "Rechecking model availability and reasoning support before applying your selection.",
            theme.primary.to_color(),
        ),
        Status::Loading => (
            "Analyzing this task…",
            if view.scope == AdviceScope::ExistingSession {
                "A configured agent evaluates feature context and any current plan using verified options and provider research."
            } else {
                "A configured agent evaluates the reviewed plan using verified model options and provider research."
            },
            theme.primary.to_color(),
        ),
        Status::Insufficient => (
            "No supported recommendation yet",
            "No recommendations could be verified. Claude Code and Codex are supported; check sign-in, model access and effort support.",
            theme.warning.to_color(),
        ),
        Status::Error(error) => ("Analysis failed", error.as_str(), theme.danger.to_color()),
        Status::Ready(_) => return,
    };
    let block = panel("Status", theme);
    let inner = block.inner(area);
    let paragraph = Paragraph::new(vec![
        Line::from(""),
        Line::styled(
            title,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
        Line::from(detail),
    ])
    .wrap(Wrap { trim: false });
    let offset = usize::from(view.scroll).min(
        paragraph
            .line_count(inner.width)
            .saturating_sub(usize::from(inner.height)),
    );
    frame.render_widget(
        paragraph
            .scroll((offset.min(u16::MAX as usize) as u16, 0))
            .block(block),
        area,
    );
}

fn footer(view: &AdviceView<'_>, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let key = Style::default()
        .fg(theme.primary.to_color())
        .add_modifier(Modifier::BOLD);
    let text = Style::default().fg(theme.text_muted.to_color());
    let mut first = vec![Span::styled("Esc", key), Span::styled(" Back   ", text)];
    if matches!(view.status, Status::Ready(_)) {
        first.extend([Span::styled("↑/↓", key), Span::styled(" Choose   ", text)]);
        if view.scope == AdviceScope::InitialLaunch {
            first.extend([Span::styled("Enter", key), Span::styled(" Apply   ", text)]);
        }
    }
    let mut second = vec![Span::styled("r", key), Span::styled(" Retry   ", text)];
    if matches!(view.status, Status::Ready(_)) {
        second.extend([
            Span::styled("s", key),
            Span::styled(
                if view.show_sources {
                    " Hide sources   "
                } else {
                    " Sources   "
                },
                text,
            ),
        ]);
        if view.show_sources {
            second.extend([
                Span::styled("PgUp/PgDn", key),
                Span::styled(" Scroll", text),
            ]);
        }
    }
    if width >= 85 {
        first.extend(second);
        vec![Line::from(first)]
    } else {
        vec![Line::from(first), Line::from(second)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model_evidence::research_notes,
        model_options::{
            Availability, EligibleOptions, HarnessCapability, LaunchPath, ModelCapability,
        },
        project::AgentKind,
    };
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn ready() -> Status {
        let caps = HarnessCapability {
            harness: AgentKind::Codex,
            availability: Availability::Available,
            model_flag: true,
            reasoning_flag: true,
            models: ["gpt-6-sol", "gpt-6-luna", "gpt-6-astra"]
                .into_iter()
                .map(|model| ModelCapability {
                    model: model.into(),
                    availability: Availability::Available,
                    reasoning_levels: Some(vec!["medium".into()]),
                })
                .collect(),
        };
        let options = EligibleOptions::new(&[AgentKind::Codex], &[caps], LaunchPath::Interactive);
        Status::Ready(
            options
                .choices()
                .iter()
                .filter(|choice| choice.reasoning() == Some("medium"))
                .map(|choice| Recommendation {
                    choice: choice.clone(),
                    priority: Priority::Balance,
                    evidence: research_notes()
                        .into_iter()
                        .filter(|note| {
                            note.applies(choice, "2026-09-30T12:00:00Z".parse().unwrap())
                        })
                        .collect(),
                })
                .collect(),
        )
    }

    fn render(
        width: u16,
        height: u16,
        sources: bool,
        scroll: u16,
        session: bool,
        selected: usize,
    ) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let status = ready();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    AdviceView {
                        status: &status,
                        selected,
                        scroll,
                        show_sources: sources,
                        scope: if session {
                            AdviceScope::ExistingSession
                        } else {
                            AdviceScope::InitialLaunch
                        },
                        checking_setting: false,
                        message: None,
                    },
                    &Theme::default(),
                )
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn contents(buffer: &Buffer) -> String {
        buffer
            .content
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn default_view_prioritizes_choices_and_highlights_selection() {
        let buffer = render(198, 28, false, 0, false, 0);
        let text = contents(&buffer);
        assert_eq!(text.matches("Model & reasoning").count(), 1);
        assert!(text.contains("Available settings"));
        assert!(text.contains("gpt-6-sol"));
        assert!(text.contains("EFFORT"));
        assert!(text.contains("Apply"));
        assert!(!text.contains("https://"));
        assert!(!text.contains("openai-reasoning-effort"));
        let selected = buffer
            .content
            .iter()
            .position(|cell| cell.symbol() == "›")
            .unwrap();
        assert_eq!(
            buffer.content[selected].bg,
            Theme::default().effective_selection_bg()
        );
        assert_eq!(dialog_area(Rect::new(0, 0, 198, 28), false).width, 120);
    }

    #[test]
    fn research_scroll_keeps_choices_and_actions_visible() {
        let text = contents(&render(120, 25, true, u16::MAX, false, 0));
        assert!(text.contains("Available settings"));
        assert!(text.contains("gpt-6-sol"));
        assert!(text.contains("expires 2026-10-29"));
        assert!(text.contains("Hide sources"));
        assert!(text.contains("Esc"));
        assert!(text.contains("Apply"));
    }

    #[test]
    fn small_terminals_and_session_advice_keep_a_return_action() {
        for (width, height) in [(46, 18), (80, 24), (120, 40), (12, 6)] {
            let text = contents(&render(width, height, false, 0, true, 0));
            assert!(
                text.contains("Esc"),
                "missing return action at {width}x{height}"
            );
            assert!(!text.contains("Apply"));
        }
    }

    #[test]
    fn selection_stays_visible_when_only_one_row_fits() {
        let Status::Ready(choices) = ready() else {
            unreachable!()
        };
        let text = contents(&render(46, 18, false, 0, false, 2));
        assert!(text.contains(choices[2].choice.model()));
        assert!(text.contains("›"));
    }
    #[test]
    fn reviewed_existing_and_host_todo_plans_explain_their_view_only_scope() {
        for scope in [AdviceScope::ExistingPlan, AdviceScope::HostTodoPlan] {
            let mut terminal = Terminal::new(TestBackend::new(120, 28)).unwrap();
            let status = ready();
            terminal
                .draw(|frame| {
                    draw(
                        frame,
                        AdviceView {
                            status: &status,
                            selected: 0,
                            scroll: 0,
                            show_sources: false,
                            scope,
                            checking_setting: false,
                            message: None,
                        },
                        &Theme::default(),
                    )
                })
                .unwrap();
            let text = contents(terminal.backend().buffer());
            assert!(text.contains("Advice only"));
            assert!(text.contains("Accept"));
            assert!(!text.contains("Apply"));
            assert!(text.contains("Esc"));
            assert!(text.contains(if scope == AdviceScope::HostTodoPlan {
                "TODO plan → implementation"
            } else {
                "Reviewed plan → implementation"
            }));
        }
    }
}
