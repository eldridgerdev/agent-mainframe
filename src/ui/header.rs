use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn draw(frame: &mut Frame, area: Rect, pending_count: usize) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut title_spans = vec![
        Span::styled(
            " Agent Mainframe ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "| Multi-Project Agent Manager",
            Style::default().fg(Color::DarkGray),
        ),
    ];

    if pending_count > 0 {
        title_spans.push(Span::styled(
            format!(
                "  [{} input request{}]",
                pending_count,
                if pending_count == 1 { "" } else { "s" },
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let title = Paragraph::new(Line::from(title_spans));
    frame.render_widget(title, inner);

    let help_hint = Line::from(vec![
        Span::styled("?", Style::default().fg(Color::Yellow)),
        Span::styled(" help ", Style::default().fg(Color::DarkGray)),
    ]);
    let hint_width: u16 = help_hint.spans.iter().map(|s| s.content.len() as u16).sum();
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
