use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::ReviewHarnessPickState;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

/// Modal shown when a finished review dispatches its fixes to a fresh dedicated
/// session: the reviewer picks which harness runs them. The feedback file is
/// already written, so skipping (Esc) just leaves it for later.
pub fn draw_review_harness_pick(frame: &mut Frame, state: &ReviewHarnessPickState, theme: &Theme) {
    let area = centered_rect(50, 40, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()))
        .title(" Run review fixes in… ");

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::from(Span::styled(
            " Pick the harness for the dedicated review session:",
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(""),
    ];

    for (index, harness) in state.harnesses.iter().enumerate() {
        let style = if index == state.selected {
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };
        lines.push(Line::from(Span::styled(
            format!("  {}", harness.display_name()),
            style,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(" run fixes here  ", Style::default().fg(theme.text.to_color())),
        Span::styled("q/Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " skip (feedback already saved)",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));

    frame.render_widget(Paragraph::new(lines), inner);
}
