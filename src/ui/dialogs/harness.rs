use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::super::dashboard::centered_rect;
use crate::app::{HarnessCheckStatus, HarnessSetupState};
use crate::theme::Theme;

pub fn draw_harness_setup_dialog(
    frame: &mut Frame,
    state: &HarnessSetupState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(50, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = if state.is_startup {
        " Configure Agent Harnesses "
    } else {
        " Manage Agent Harnesses "
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(theme.text.to_color()).bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.border.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(inner);

    // Header
    let header_text = if state.is_startup {
        "Select which agent harnesses to enable:"
    } else {
        "Toggle harnesses (Enter to check & enable):"
    };
    let header = Paragraph::new(Line::from(Span::styled(
        format!("  {}", header_text),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    frame.render_widget(header, chunks[0]);

    // Harness list
    let mut lines: Vec<Line> = Vec::new();
    for (i, harness) in state.harnesses.iter().enumerate() {
        let is_selected = i == state.selected;
        let marker = if is_selected { ">" } else { " " };

        let check = if harness.enabled {
            Span::styled(
                "[x] ",
                Style::default()
                    .fg(theme.success.to_color())
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                "[ ] ",
                Style::default().fg(theme.text_muted.to_color()),
            )
        };

        let name_style = if is_selected {
            Style::default()
                .fg(theme.text.to_color())
                .bg(theme.effective_selection_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };

        let status_spans: Vec<Span> = match &harness.status {
            HarnessCheckStatus::Unchecked => vec![],
            HarnessCheckStatus::Checking => {
                let throbber = throbber_widgets_tui::Throbber::default()
                    .throbber_style(
                        Style::default()
                            .fg(theme.info.to_color())
                            .add_modifier(Modifier::BOLD),
                    )
                    .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
                    .use_type(throbber_widgets_tui::WhichUse::Spin);
                let spin = throbber.to_symbol_span(throbber_state);
                vec![
                    Span::raw(" "),
                    spin,
                    Span::styled(
                        " checking...",
                        Style::default().fg(theme.info.to_color()),
                    ),
                ]
            }
            HarnessCheckStatus::Installed => vec![Span::styled(
                " (installed)",
                Style::default().fg(theme.success.to_color()),
            )],
            HarnessCheckStatus::NotFound(hint) => vec![Span::styled(
                format!(" (not found: {})", hint),
                Style::default().fg(theme.danger.to_color()),
            )],
        };

        let mut spans = vec![
            Span::styled(
                format!("  {} ", marker),
                Style::default().fg(theme.warning.to_color()),
            ),
            check,
            Span::styled(harness.kind.display_name().to_string(), name_style),
        ];
        spans.extend(status_spans);
        lines.push(Line::from(spans));
    }
    let list = Paragraph::new(lines);
    frame.render_widget(list, chunks[1]);

    // Hints
    let hints = Line::from(vec![
        Span::styled(
            "  Enter/Space",
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" toggle  "),
        Span::styled(
            "c",
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" confirm  "),
        Span::styled(
            "Esc",
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(if state.is_startup { " confirm" } else { " cancel" }),
    ]);
    let hints_widget = Paragraph::new(hints);
    frame.render_widget(hints_widget, chunks[2]);
}
