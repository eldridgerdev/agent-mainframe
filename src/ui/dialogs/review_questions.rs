use crate::{
    app::review_questions::{DraftDestination, Questions},
    theme::Theme,
};
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
            Constraint::Length(2),
            Constraint::Min(3),
            // Room to type only when typing goes somewhere.
            Constraint::Length(if q.editing || q.draft.is_some() { 5 } else { 3 }),
            Constraint::Length(2),
        ])
        .split(inner);
    let muted = Style::default().fg(theme.text_muted.to_color());
    let turn = q.turns.get(q.selected);
    let stale = turn.is_some_and(|t| t.context.version != q.current_version);

    // Header: what is being asked about, and where in the history we are.
    let mut header = vec![Line::styled(
        turn.map_or_else(
            || "Ask about the selected code or anything in the repository.".to_string(),
            |t| t.context.label.clone(),
        ),
        muted,
    )];
    header.push(if stale {
        Line::styled(
            "This answer is about an earlier version of the code — ask again before drafting a comment.",
            Style::default().fg(theme.warning.to_color()),
        )
    } else if q.turns.len() > 1 {
        Line::styled(
            format!(
                "Question {} of {} · [ and ] browse earlier questions · history clears when this review closes",
                q.selected + 1,
                q.turns.len()
            ),
            muted,
        )
    } else {
        Line::styled("History clears when this review closes.", muted)
    });
    frame.render_widget(Paragraph::new(header).wrap(Wrap { trim: false }), areas[0]);

    // Conversation: the question and its answer, each labelled, so neither
    // reads as part of the other or of the header.
    let failure = turn.and_then(|t| t.error.as_deref()).or(q.error.as_deref());
    let conversation = match turn {
        None => "Type a question below and press **Ctrl+S**. The AI can read the whole repository, not only this diff, and answers point at files and lines.".to_string(),
        Some(t) => {
            let asked = t
                .question
                .lines()
                .map(|l| format!("> {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            let reply = match (t.answer.as_deref(), failure) {
                (Some(answer), _) => answer.to_string(),
                (None, _) if q.request.is_some() => "_Working on it… you can press Esc to keep reviewing; the answer will be here when you come back._".into(),
                // A failed question must say why where the answer would be: a
                // reason only in the footer reads as "nothing happened".
                (None, Some(reason)) => format!(
                    "**Not answered.** {reason}\n\nFix that, then press `r` to retry — or `e` to change the question."
                ),
                (None, None) => "_No answer._ Press `r` to retry.".into(),
            };
            format!(
                "**You asked**\n\n{asked}\n\n**{} answered**\n\n{reply}",
                q.harness.display_name()
            )
        }
    };
    let mut width = 0;
    let mut lines = Vec::new();
    super::markdown::draw_markdown_document(
        frame,
        areas[1],
        &conversation,
        std::path::Path::new("review-answer.md"),
        &mut q.scroll,
        &mut width,
        &mut lines,
        theme,
    );

    // Input: only shows a cursor when typing goes somewhere.
    let input_block = |title: String| {
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if q.editing || q.draft.is_some() {
                theme.primary.to_color()
            } else {
                theme.text_muted.to_color()
            }))
    };
    let editor = if let Some((destination, editor)) = &q.draft {
        let kind = match destination {
            DraftDestination::Inline => "inline",
            DraftDestination::General => "general",
        };
        Some((
            format!(
                " Draft {kind} comment — edit it, then Ctrl+S opens it in the review's comment editor "
            ),
            editor,
        ))
    } else if q.editing {
        Some((" Your question — Ctrl+S sends ".to_string(), &q.editor))
    } else {
        None
    };
    match editor {
        Some((title, editor)) => {
            let editor_lines =
                super::editor_view::editor_lines(editor, theme, "Type your question");
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
                    .block(input_block(title)),
                areas[2],
            );
        }
        None => {
            let prompt = if q.request.is_some() {
                "Waiting for the answer."
            } else if turn.is_some_and(|t| t.answer.is_some()) {
                "Press e to ask a follow-up, or i / g to turn this answer into a review comment."
            } else {
                "Press e to change the question, or r to retry it."
            };
            frame.render_widget(
                Paragraph::new(Line::styled(prompt, muted))
                    .wrap(Wrap { trim: false })
                    .block(input_block(" Follow-up ".into())),
                areas[2],
            );
        }
    }

    // Footer: live status (progress, else the error), then the keys that do
    // something right now.
    let status = match (q.started_at, q.error.as_deref()) {
        (Some(started), _) => Some(progress_line(q, started.elapsed(), theme)),
        (None, Some(error)) => Some(Line::styled(
            error.to_string(),
            Style::default()
                .fg(theme.danger.to_color())
                .add_modifier(Modifier::BOLD),
        )),
        (None, None) => None,
    };
    let answered = turn.is_some_and(|t| t.answer.is_some());
    let hints = if q.draft.is_some() {
        "Ctrl+S open in comment editor · Esc discard draft".to_string()
    } else if q.editing {
        "Ctrl+S send · Ctrl+H switch harness · Esc back to review".to_string()
    } else {
        let mut keys = vec!["e follow-up"];
        if answered {
            keys.push("i draft inline comment");
            keys.push("g draft general comment");
        }
        if turn.is_some() {
            keys.push("r retry");
        }
        if q.turns.len() > 1 {
            keys.push("[ ] history");
        }
        keys.extend(["j/k scroll", "Esc back to review"]);
        keys.join(" · ")
    };
    frame.render_widget(
        // With no status line, the hints may use both rows.
        Paragraph::new(
            status
                .into_iter()
                .chain([Line::styled(hints, muted)])
                .collect::<Vec<_>>(),
        )
        .wrap(Wrap { trim: false }),
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
