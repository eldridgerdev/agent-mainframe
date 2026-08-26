use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::super::dashboard::centered_rect;
use crate::app::FreshContextPromptState;
use crate::theme::Theme;

const CURSOR: &str = "\u{2588}";

/// One-line prompt overlaid on a session view, collecting the instruction to
/// seed into a brand-new fresh-context agent session before it's created.
pub fn draw_fresh_context_prompt_dialog(
    frame: &mut Frame,
    state: &FreshContextPromptState,
    theme: &Theme,
) {
    let area = centered_rect(60, 22, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(format!(" Fresh context · {} ", state.feature_name))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let prompt_line = Paragraph::new(Line::from(vec![
        Span::styled(" Prompt: ", Style::default().fg(theme.primary.to_color())),
        Span::styled(&state.input, Style::default().fg(theme.text.to_color())),
        Span::styled(CURSOR, Style::default().fg(theme.primary.to_color())),
    ]));
    frame.render_widget(prompt_line, chunks[0]);

    let caption = Paragraph::new(Line::from(Span::styled(
        " A new session in this feature will start with your plan, changed files, \
         and this instruction already loaded.",
        Style::default().fg(theme.text_muted.to_color()),
    )))
    .wrap(Wrap { trim: true });
    frame.render_widget(caption, chunks[1]);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " start session  ",
            Style::default().fg(theme.text.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text.to_color())),
    ]));
    frame.render_widget(hint, chunks[3]);
}
