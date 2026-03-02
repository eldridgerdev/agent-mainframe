use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

pub fn draw(frame: &mut Frame, area: Rect, cwd: &str, pending_count: usize, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_accent.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut title_spans = vec![
        Span::styled(
            " Agent Mainframe ",
            Style::default()
                .fg(theme.accent.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("| ", Style::default().fg(theme.muted.to_color())),
        Span::styled(cwd, Style::default().fg(theme.fg.to_color())),
    ];

    if pending_count > 0 {
        title_spans.push(Span::styled(
            format!(
                "  [{} input request{}]",
                pending_count,
                if pending_count == 1 { "" } else { "s" },
            ),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ));
    }

    let title = Paragraph::new(Line::from(title_spans));
    frame.render_widget(title, inner);

    let help_hint = Line::from(vec![
        Span::styled("?", Style::default().fg(theme.accent.to_color())),
        Span::styled(" help ", Style::default().fg(theme.muted.to_color())),
    ]);
    let hint_width: u16 = help_hint.spans.iter().map(|s| s.content.len() as u16).sum();
    let hint_width = hint_width.min(inner.width);
    if hint_width > 0 {
        let hint_area = Rect {
            x: inner
                .x
                .saturating_add(inner.width.saturating_sub(hint_width)),
            y: inner.y,
            width: hint_width,
            height: 1,
        };
        let hint = Paragraph::new(help_hint);
        frame.render_widget(hint, hint_area);
    }
}
