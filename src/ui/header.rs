use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::theme::Theme;

/// Needs-attention counts as `(questions, completed, waiting)`.
pub type AttentionCounts = (usize, usize, usize);

/// Which needs-attention group a badge segment describes.
enum AttentionGroup {
    Question,
    Completed,
    Waiting,
    /// Pending inputs the attention layer cannot explain: diff reviews, change
    /// reasons, review-ready prompts, and harnesses that report no lifecycle
    /// events at all.
    Pending,
}

/// The badge's segments, in display order, with zero-count groups omitted
/// rather than shown as `0`.
///
/// Questions and completions are named separately because they call for
/// different things from the user — an answer versus a look — and the whole
/// point of the attention layer is that the dashboard can tell them apart.
/// Unexplained pending inputs are always appended rather than replaced by the
/// breakdown: they are separate work, so one session's question must not hide
/// another session's diff review from the count.
fn badge_segments(counts: AttentionCounts, pending_count: usize) -> Vec<(String, AttentionGroup)> {
    let (questions, completed, waiting) = counts;
    let mut segments = Vec::new();

    if questions > 0 {
        segments.push((
            format!(
                "{questions} question{}",
                if questions == 1 { "" } else { "s" }
            ),
            AttentionGroup::Question,
        ));
    }
    if completed > 0 {
        segments.push((format!("{completed} to review"), AttentionGroup::Completed));
    }
    if waiting > 0 {
        segments.push((format!("{waiting} waiting"), AttentionGroup::Waiting));
    }
    if pending_count > 0 {
        segments.push((
            format!(
                "{pending_count} input request{}",
                if pending_count == 1 { "" } else { "s" }
            ),
            AttentionGroup::Pending,
        ));
    }
    segments
}

/// The header badge's plain text, so hit-testing can measure exactly what
/// [`draw`] renders instead of reimplementing the decision. `None` when there
/// is no badge.
///
/// `pending_count` is the number of pending inputs *not* already described by
/// an attention row, so a session that both raised an input request and told
/// us why it stopped is counted once.
pub fn badge_text(attention: AttentionCounts, pending_count: usize) -> Option<String> {
    let segments = badge_segments(attention, pending_count);
    if segments.is_empty() {
        return None;
    }
    let body: Vec<String> = segments.into_iter().map(|(text, _)| text).collect();
    Some(format!("  [{}]", body.join(", ")))
}

/// The badge breakdown, as coloured spans.
fn badge_spans(counts: AttentionCounts, pending_count: usize, theme: &Theme) -> Vec<Span<'static>> {
    let segments = badge_segments(counts, pending_count);
    if segments.is_empty() {
        return Vec::new();
    }

    let muted = Style::default().fg(theme.text_muted.to_color());
    let mut spans = vec![Span::styled("  [", muted)];
    for (i, (text, group)) in segments.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(", ", muted));
        }
        let style = match group {
            AttentionGroup::Question => Style::default()
                .fg(theme.status_waiting.to_color())
                .add_modifier(Modifier::BOLD),
            AttentionGroup::Completed => Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
            AttentionGroup::Waiting => muted,
            AttentionGroup::Pending => Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        };
        spans.push(Span::styled(text, style));
    }
    spans.push(Span::styled("]", muted));
    spans
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    cwd: &str,
    version: &str,
    pending_count: usize,
    attention: AttentionCounts,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focus.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut title_spans = vec![
        Span::styled(
            " Agent Mainframe ",
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("v{version} "),
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("| ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(cwd, Style::default().fg(theme.text.to_color())),
    ];

    // Same segments as `badge_text`, which hit-testing measures.
    title_spans.extend(badge_spans(attention, pending_count, theme));

    let title = Paragraph::new(Line::from(title_spans));
    frame.render_widget(title, inner);

    let help_hint = Line::from(vec![
        Span::styled("?", Style::default().fg(theme.primary.to_color())),
        Span::styled(" help ", Style::default().fg(theme.text_muted.to_color())),
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
