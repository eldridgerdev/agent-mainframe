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
}

/// The badge's segments, in display order, with zero-count groups omitted
/// rather than shown as `0`.
///
/// Questions and completions are named separately because they call for
/// different things from the user — an answer versus a look — and the whole
/// point of the attention layer is that the dashboard can tell them apart.
fn attention_segments(counts: AttentionCounts) -> Vec<(String, AttentionGroup)> {
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
    segments
}

/// The flat input-request count, the badge AMF showed before the attention
/// layer existed.
fn pending_badge_text(pending_count: usize) -> Option<String> {
    (pending_count > 0).then(|| {
        format!(
            "  [{} input request{}]",
            pending_count,
            if pending_count == 1 { "" } else { "s" },
        )
    })
}

/// The header badge's plain text, so hit-testing can measure exactly what
/// [`draw`] renders instead of reimplementing the decision. `None` when there
/// is no badge.
///
/// The attention breakdown supersedes the flat count when any harness has told
/// us why it stopped. The old count stays as the fallback for pending inputs
/// the attention layer doesn't cover (diff reviews, change reasons) and for
/// harnesses that report no lifecycle events at all.
pub fn badge_text(attention: AttentionCounts, pending_count: usize) -> Option<String> {
    let segments = attention_segments(attention);
    if segments.is_empty() {
        return pending_badge_text(pending_count);
    }
    let body: Vec<String> = segments.into_iter().map(|(text, _)| text).collect();
    Some(format!("  [{}]", body.join(", ")))
}

/// The needs-attention breakdown, as coloured spans.
fn attention_spans(counts: AttentionCounts, theme: &Theme) -> Vec<Span<'static>> {
    let segments = attention_segments(counts);
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

    // Same precedence as `badge_text`, which hit-testing measures: the
    // attention breakdown when there is one, the flat count otherwise.
    let attention_spans = attention_spans(attention, theme);
    if !attention_spans.is_empty() {
        title_spans.extend(attention_spans);
    } else if let Some(text) = pending_badge_text(pending_count) {
        title_spans.push(Span::styled(
            text,
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ));
    }

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
