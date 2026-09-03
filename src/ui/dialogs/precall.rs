use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::precall::PendingPrecall;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_prompt_precall(frame: &mut Frame, pending: &PendingPrecall, theme: &Theme) {
    let (w, h) = if pending.viewing { (78, 78) } else { (56, 34) };
    let area = centered_rect(w, h, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Headless AI call ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.warning.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let key = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme.primary.to_color()));
    let muted = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default().fg(theme.text_muted.to_color()),
        )
    };

    if pending.viewing {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                muted(" prompt: "),
                Span::styled(
                    pending.prompt_id.as_str().to_string(),
                    Style::default().fg(theme.text.to_color()),
                ),
                muted("   (rendered with this run's context)"),
            ])),
            chunks[0],
        );
        let lines: Vec<Line> = pending
            .preview
            .lines()
            .skip(pending.scroll)
            .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(theme.text.to_color()))))
            .collect();
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            chunks[1],
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                key(" j/k"),
                muted(" scroll  "),
                key("v"),
                muted(" hide  "),
                key("e"),
                muted(" edit  "),
                key("Enter"),
                muted(" continue  "),
                key("Esc"),
                muted(" cancel"),
            ])),
            chunks[2],
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(inner);

    let body = vec![
        Line::raw(""),
        Line::from(Span::styled(
            " AMF is about to make a headless AI call.",
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(vec![
            muted("   Prompt:  "),
            Span::styled(
                pending.prompt_id.spec().title.to_string(),
                Style::default().fg(theme.text.to_color()),
            ),
        ]),
        Line::from(vec![
            muted("   ID:      "),
            Span::styled(
                pending.prompt_id.as_str().to_string(),
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]),
        Line::from(vec![
            muted("   Harness: "),
            Span::styled(
                pending.harness.display_name().to_string(),
                Style::default().fg(theme.text.to_color()),
            ),
        ]),
        Line::raw(""),
        Line::from(muted("   v  view the exact prompt")),
        Line::from(muted("   e  edit its template (override manager)")),
        Line::from(muted("   Enter  make the call    Esc  cancel it")),
    ];
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), chunks[0]);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            key(" v"),
            muted(" view  "),
            key("e"),
            muted(" edit  "),
            key("Enter"),
            muted(" continue  "),
            key("Esc"),
            muted(" cancel"),
        ])),
        chunks[1],
    );
}
