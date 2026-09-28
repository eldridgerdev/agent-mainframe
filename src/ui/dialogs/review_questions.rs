use crate::{app::review_questions::Questions, theme::Theme};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
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
    let failure = turn.and_then(|t| t.error.as_deref()).or(q.error.as_deref());
    // A failed question must say why where the answer would be: a reason only
    // in the footer reads as "nothing happened" and every retry looks the same.
    let answer = match (turn.and_then(|t| t.answer.as_deref()), failure) {
        (Some(answer), _) => answer.to_string(),
        (None, _) if q.request.is_some() => "Working… You can close this overlay and continue reviewing. Ctrl+X cancels this request.".into(),
        (None, Some(reason)) if turn.is_some() => format!(
            "**The question was not answered.**\n\n{reason}\n\nFix that, then press `r` to retry — or `e` to ask something else."
        ),
        _ => "Answers include repository references. Ctrl+S asks your question.".into(),
    };
    let mut width = 0;
    let mut lines = Vec::new();
    super::markdown::draw_markdown_document(
        frame,
        areas[1],
        &answer,
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
        "Ctrl+S: open existing comment editor · Esc: discard draft · Ctrl+X: cancel"
    } else if q.editing {
        "Ctrl+S: ask · Ctrl+H: harness · Esc: return to review · Ctrl+X: cancel"
    } else {
        "e: follow-up · r: retry · [/]: history · j/k: scroll · i/g: inline/general draft · Esc: return"
    };
    let status = match q.started_at {
        Some(started) => progress_line(q, started.elapsed(), theme),
        None => Line::from("Drafting and answering never publish a comment."),
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                error,
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Line::from(hints),
            status,
        ])
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(theme.text_muted.to_color())),
        areas[3],
    );
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The one thing on screen that says a request is live, so it has to read as
/// activity rather than as another muted footer line: a spinner driven by the
/// elapsed time (the overlay redraws on `ANIMATED_REDRAW_INTERVAL` while a
/// request is open), what is being done, by which harness, and for how long.
fn progress_line(q: &Questions, elapsed: std::time::Duration, theme: &Theme) -> Line<'static> {
    let frame = SPINNER[(elapsed.as_millis() / 100) as usize % SPINNER.len()];
    let action = if q.draft.is_some() {
        "Checking the draft"
    } else if q.turns.get(q.selected).is_some_and(|t| t.answer.is_some()) {
        "Drafting a review comment"
    } else {
        "Asking"
    };
    let accent = Style::default()
        .fg(theme.info.to_color())
        .add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(
            format!("{frame} {action} {}… ", q.harness.display_name()),
            accent,
        ),
        Span::raw(format!("{}s · Ctrl+X cancels", elapsed.as_secs())),
    ])
}
