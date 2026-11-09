//! An intentionally local, disposable mock for exploring the defaults screen.
use crate::theme::Theme;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

pub fn draw_model_defaults_prototype(
    frame: &mut Frame,
    selected: usize,
    harness: usize,
    benchmark_notice: bool,
    models: &[usize; 4],
    reasoning: &[usize; 4],
    theme: &Theme,
) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(theme.text.to_color())
                .bg(theme.effective_bg()),
        ),
        area,
    );
    let sections = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .margin(1)
    .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Model & reasoning",
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   EXPERIMENTAL · PROTOTYPE",
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::styled(
                "   Sample choices · nothing is saved",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]))
        .wrap(Wrap { trim: true }),
        sections[0],
    );

    let harness_names = ["Claude Code", "Codex", "OpenCode", "Pi"];
    let tabs = harness_names
        .iter()
        .enumerate()
        .flat_map(|(i, name)| {
            let style = if i == harness {
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_muted.to_color())
            };
            [Span::styled(format!(" {} ", name), style), Span::raw("  ")]
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Line::from(tabs)), sections[1]);

    let panel = Block::default()
        .title(format!(" {} · Roles ", harness_names[harness]))
        .title_style(Style::default().add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border.to_color()));
    let inner = panel.inner(sections[2]);
    frame.render_widget(panel, sections[2]);
    let panel_sections = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(inner);
    frame.render_widget(Paragraph::new("Select a role to explore its model and reasoning defaults. Each run could override these choices.")
        .style(Style::default().fg(theme.text_muted.to_color())).wrap(Wrap { trim: true }), panel_sections[0]);

    let names = [
        "Code work",
        "Orchestration",
        "Plan review",
        "PR / final review",
    ];
    let model_names = match harness {
        0 => ["Opus", "Sonnet", "Haiku"],
        1 => ["GPT-6.1 Sol", "GPT-6 Astra", "GPT-6 Luna"],
        2 => ["anthropic/sonnet", "openai/gpt-6.1-sol", "anthropic/opus"],
        _ => ["anthropic/sonnet", "openai/gpt-6.1-sol", "anthropic/opus"],
    };
    let levels = ["Harness default", "Low", "Medium", "High"];
    let descriptions = [
        "Implement features, debug issues, and make everyday changes.",
        "Break down work and coordinate agents and tasks.",
        "Examine a proposed plan before implementation begins.",
        "Review changes for correctness, risks, and missing coverage.",
    ];
    // Keep the selected role visible on shorter terminals.
    let list = panel_sections[1];
    let row_height = if list.height >= 24 { 6 } else { 5 };
    let visible = usize::from(list.height / row_height).clamp(1, 4);
    let first = (selected + 1).saturating_sub(visible);
    for i in first..(first + visible).min(4) {
        let y = list.y.saturating_add((i - first) as u16 * row_height);
        let row = Rect::new(
            list.x.saturating_add(1),
            y,
            list.width.saturating_sub(2),
            row_height.min(list.bottom().saturating_sub(y)),
        );
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(row);
        let heading =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(27.min(row.width))])
                .split(parts[0]);
        let active = i == selected;
        let name_style = Style::default()
            .fg(if active {
                theme.primary.to_color()
            } else {
                theme.text.to_color()
            })
            .add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(format!("{} {}", if active { "›" } else { " " }, names[i]))
                .style(name_style),
            heading[0],
        );
        frame.render_widget(
            Paragraph::new(format!(
                "● {} · {}",
                model_names[models[i]], levels[reasoning[i]]
            ))
            .alignment(ratatui::layout::Alignment::Right)
            .style(Style::default().fg(theme.primary.to_color())),
            heading[1],
        );
        frame.render_widget(
            Paragraph::new(format!("  {}", descriptions[i]))
                .style(Style::default().fg(theme.text_muted.to_color())),
            parts[1],
        );
        let mut options = vec![Span::styled(
            "  Model  ",
            Style::default().fg(theme.text_muted.to_color()),
        )];
        // Put the current choice first so it stays visible at narrow widths.
        for (position, model) in std::iter::once(models[i])
            .chain((0..model_names.len()).filter(|m| *m != models[i]))
            .enumerate()
        {
            if position > 0 {
                options.push(Span::styled(
                    "  ",
                    Style::default().fg(theme.text_muted.to_color()),
                ));
            }
            let style = if model == models[i] {
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_muted.to_color())
            };
            options.push(Span::styled(format!("[● {}]", model_names[model]), style));
        }
        frame.render_widget(Paragraph::new(Line::from(options)), parts[2]);
        let suggested = match (harness, i) {
            (0, 0 | 3) => 1,
            (0, _) => 0,
            (1, 0 | 3) => 0,
            (1, _) => 1,
            (_, 0 | 3) => 0,
            (_, _) => 2,
        };
        let suggested_level = if i == 0 || i == 3 { "Medium" } else { "High" };
        frame.render_widget(
            Paragraph::new(format!(
                "  Example recommendation: {} · {}",
                model_names[suggested], suggested_level
            ))
            .style(Style::default().fg(theme.text_muted.to_color())),
            parts[3],
        );
        if i < 3 && parts[4].height > 0 {
            frame.render_widget(
                Paragraph::new("─".repeat(usize::from(row.width)))
                    .style(Style::default().fg(theme.border.to_color())),
                parts[4],
            );
        }
    }
    let footer =
        Layout::vertical([Constraint::Length(1), Constraint::Length(2)]).split(sections[3]);
    let benchmark = if benchmark_notice {
        "[ b Run benchmarks ]  Prototype only: no benchmark was run."
    } else {
        "[ b Run benchmarks ]  Prototype button · recommendations are illustrative"
    };
    frame.render_widget(
        Paragraph::new(benchmark)
            .style(Style::default().fg(theme.warning.to_color()))
            .wrap(Wrap { trim: true }),
        footer[0],
    );
    frame.render_widget(
        Paragraph::new("Tab / Shift+Tab harness   ↑/↓ role   ←/→ model   r reasoning   Esc close")
            .style(Style::default().fg(theme.text_muted.to_color()))
            .wrap(Wrap { trim: true }),
        footer[1],
    );
}
