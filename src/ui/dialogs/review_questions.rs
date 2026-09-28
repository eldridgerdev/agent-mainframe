use crate::{app::review_questions::Questions, theme::Theme};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub(crate) fn draw(frame: &mut Frame, q: &mut Questions, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(96, 90, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);
    let block = Block::default()
        .title(format!(
            " Ask AI · {} · read-only ",
            q.harness.display_name()
        ))
        .borders(Borders::ALL)
        .style(
            Style::default()
                .bg(theme.effective_bg())
                .fg(theme.text.to_color()),
        )
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(5),
            Constraint::Length(3),
        ])
        .split(inner);
    let turn = q.turns.get(q.selected);
    let mut header = vec![Line::from(format!(
        "Question {} of {} · history clears when this review closes",
        q.selected + usize::from(!q.turns.is_empty()),
        q.turns.len()
    ))];
    if let Some(turn) = turn {
        header.push(Line::from(turn.context.label.clone()));
        header.push(Line::from(if turn.context.version != q.current_version {
            "Previous code context — ask again before drafting"
        } else {
            &turn.question
        }));
    } else {
        header.push(Line::from(
            "Ask about the whole repository or the selected code.",
        ));
    }
    frame.render_widget(Paragraph::new(header).wrap(Wrap { trim: false }), areas[0]);
    let answer = turn.and_then(|t| t.answer.as_deref()).unwrap_or_else(|| {
        if q.request.is_some() { "Working… You can close this overlay and continue reviewing. Ctrl+X cancels this request." }
        else { "Answers include repository references. Ctrl+Enter asks your question." }
    });
    let mut width = 0;
    let mut lines = Vec::new();
    super::markdown::draw_markdown_document(
        frame,
        areas[1],
        answer,
        std::path::Path::new("review-answer.md"),
        &mut q.scroll,
        &mut width,
        &mut lines,
        theme,
    );
    let (title, editor) = if let Some((destination, editor)) = &q.draft {
        (
            format!(" Editable {:?} comment draft ", destination),
            editor,
        )
    } else {
        (
            if q.editing {
                " Question / follow-up "
            } else {
                " Last question (e to write a follow-up) "
            }
            .into(),
            &q.editor,
        )
    };
    let editor_lines = super::editor_view::editor_lines(editor, theme, "Enter your question");
    let wrap_width = areas[2].width.saturating_sub(2).max(1) as usize;
    let total = super::editor_view::count_wrapped_editor_lines(&editor_lines, wrap_width);
    let mut sync = true;
    super::editor_view::sync_editor_scroll(
        editor,
        &mut q.editor_scroll,
        &mut sync,
        areas[2].height.saturating_sub(2) as usize,
        wrap_width,
        total,
    );
    frame.render_widget(
        Paragraph::new(editor_lines)
            .wrap(Wrap { trim: false })
            .scroll((q.editor_scroll.min(u16::MAX as usize) as u16, 0))
            .block(Block::default().title(title).borders(Borders::ALL)),
        areas[2],
    );
    let error = q
        .error
        .as_deref()
        .or_else(|| turn.and_then(|t| t.error.as_deref()))
        .unwrap_or("");
    let hints = if q.draft.is_some() {
        "Ctrl+Enter: open existing comment editor · Esc: discard draft · Ctrl+X: cancel"
    } else if q.editing {
        "Ctrl+Enter: ask · Ctrl+H: harness · Esc: return to review · Ctrl+X: cancel"
    } else {
        "e: follow-up · r: retry · [/]: history · j/k: scroll · i/g: inline/general draft · Esc: return"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(error),
            Line::from(hints),
            Line::from(if let Some(started) = q.started_at {
                format!("Request in progress… {}s", started.elapsed().as_secs())
            } else {
                "Drafting and answering never publish a comment.".into()
            }),
        ])
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(theme.text_muted.to_color())),
        areas[3],
    );
}
