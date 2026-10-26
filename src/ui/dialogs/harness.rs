use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
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
        .style(
            Style::default()
                .fg(theme.text.to_color())
                .bg(theme.effective_bg()),
        )
        .border_style(Style::default().fg(theme.border.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(inner);

    // Header: says what the keys in the footer do, in the same words.
    let muted = Style::default().fg(theme.text_muted.to_color());
    let header_lines = if state.is_startup {
        vec![
            Line::from(Span::styled(
                "  Choose the agent harnesses AMF may start.",
                muted,
            )),
            Line::from(Span::styled(
                "  Enter/Space checks a harness and turns it on or off.",
                muted,
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "  Enter/Space checks a harness and turns it on or off.",
                muted,
            )),
            Line::from(Span::styled(
                "  c saves the list; Esc closes without saving.",
                muted,
            )),
        ]
    };
    let header = Paragraph::new(header_lines).wrap(Wrap { trim: false });
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
            Span::styled("[ ] ", Style::default().fg(theme.text_muted.to_color()))
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
                    Span::styled(" checking...", Style::default().fg(theme.info.to_color())),
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
    let key_style = Style::default()
        .fg(theme.warning.to_color())
        .add_modifier(Modifier::BOLD);
    let hints = if state.is_startup {
        // c and Esc both finish here (there's nothing to cancel back to on
        // first run), so label them together; q is the way out of AMF.
        Line::from(vec![
            Span::styled("  j/k", key_style),
            Span::raw(" move  "),
            Span::styled("Enter/Space", key_style),
            Span::raw(" toggle  "),
            Span::styled("c", key_style),
            Span::raw("/"),
            Span::styled("Esc", key_style),
            Span::raw(" done  "),
            Span::styled("q", key_style),
            Span::raw(" quit AMF"),
        ])
    } else {
        Line::from(vec![
            Span::styled("  j/k", key_style),
            Span::raw(" move  "),
            Span::styled("Enter/Space", key_style),
            Span::raw(" toggle  "),
            Span::styled("c", key_style),
            Span::raw(" save  "),
            Span::styled("Esc", key_style),
            Span::raw(" cancel"),
        ])
    };
    let hints_widget = Paragraph::new(hints).wrap(Wrap { trim: false });
    frame.render_widget(hints_widget, chunks[2]);
}
