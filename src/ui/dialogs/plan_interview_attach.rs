use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::AttachDocState;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

/// The file browser for attaching a reference document to a plan interview.
/// A trimmed cousin of [`super::draw_browse_path_dialog`]: no folder creation,
/// and `Enter` selects the highlighted *file*.
pub fn draw_plan_interview_attach_doc_dialog(
    frame: &mut Frame,
    state: &AttachDocState,
    theme: &Theme,
) {
    let area = centered_rect(80, 70, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let attached = state.interview.attached_docs.len();
    let block = Block::default()
        .title(format!(
            " Attach a reference document ({attached}/{} attached) ",
            crate::plan_interview::MAX_ATTACHED_DOCS
        ))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                state.explorer.cwd().to_string_lossy().to_string(),
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        chunks[0],
    );

    frame.render_widget(&state.explorer.widget(), chunks[1]);

    let status_line = if let Some(err) = &state.error {
        Line::from(Span::styled(
            format!(" {err}"),
            Style::default().fg(theme.danger.to_color()),
        ))
    } else {
        Line::from(Span::styled(
            " Attaching a doc grants this interview read-only access to the repository.",
            Style::default().fg(theme.text_muted.to_color()),
        ))
    };

    let hints = Paragraph::new(vec![
        status_line,
        Line::from(Span::styled(
            "\u{2500}".repeat(inner.width as usize),
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(vec![
            Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" open dir / attach file  "),
            Span::styled("h/BS", Style::default().fg(theme.warning.to_color())),
            Span::raw(" parent  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" cancel"),
        ]),
    ]);
    frame.render_widget(hints, chunks[2]);
}
