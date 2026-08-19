use std::ops::Range;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::theme::Theme;

/// The right-aligned hint painted at the end of the header row.
const HELP_HINT: &str = "? help ";

/// The columns [`HELP_HINT`] claims. The hint is drawn last and over the same
/// row as the title, so anything the title paints here is silently lost —
/// which is why the title is given its own narrower rect rather than the whole
/// row.
const HELP_HINT_WIDTH: u16 = HELP_HINT.len() as u16;

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
    Some(format!("  [{} | <leader i>]", body.join(", ")))
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
    spans.push(Span::styled(" | ", muted));
    spans.push(Span::styled(
        "<leader i>",
        Style::default()
            .fg(theme.warning.to_color())
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled("]", muted));
    spans
}

/// The header title's fixed leading text, in render order: name, version,
/// separator. Only the cwd that follows is negotiable when space runs short.
fn title_prefix_parts(version: &str) -> [String; 3] {
    [
        " Agent Mainframe ".to_string(),
        format!("v{version} "),
        "| ".to_string(),
    ]
}

/// The shortest cwd worth showing: an ellipsis plus enough of the tail to
/// recognise the directory. Below this the cwd is dropped rather than reduced
/// to punctuation.
const MIN_CWD_WIDTH: u16 = 6;

/// Split the header's inner row into `(title_width, hint_width)`.
///
/// The help hint is right-aligned and drawn last, so the title has to stop
/// short of it or whatever the title painted there is silently lost. On a row
/// too narrow to carry both, the hint is dropped entirely rather than eating
/// the whole title: a bare `? help ` says less than a clipped name does.
fn split_header_row(inner_width: u16) -> (u16, u16) {
    if inner_width > HELP_HINT_WIDTH {
        (inner_width - HELP_HINT_WIDTH, HELP_HINT_WIDTH)
    } else {
        (inner_width, 0)
    }
}

/// Keep the last `budget` columns of `cwd`, marking the cut with an ellipsis.
/// The tail is the informative end — the branch or worktree name — so that is
/// what survives.
fn shorten_cwd(cwd: &str, budget: u16) -> String {
    let chars: Vec<char> = cwd.chars().collect();
    if chars.len() as u16 <= budget {
        return cwd.to_string();
    }
    if budget < MIN_CWD_WIDTH {
        return String::new();
    }
    let keep = budget as usize - 1;
    std::iter::once('…')
        .chain(chars[chars.len() - keep..].iter().copied())
        .collect()
}

/// The header title laid out for a region `title_width` columns wide.
struct TitleLayout {
    /// Name, version, separator, cwd — the cwd already shortened to fit.
    prefix: [String; 4],
    /// Columns the badge occupies, relative to the title region's first
    /// column. `None` when there is no badge or no room left for one.
    badge: Option<Range<u16>>,
}

/// Lay out the header title so the badge survives a narrow terminal.
///
/// The cwd is the expendable part: it is shortened from the left until the
/// badge fits, because the badge is the only thing on this row that names work
/// waiting on the user *and* the key that reaches it. Only when the row cannot
/// hold even a shortened cwd plus the badge does the badge start losing
/// columns. [`draw`] renders this and hit-testing measures it, so the
/// clickable region is by construction the region that was drawn.
fn layout_title(
    title_width: u16,
    cwd: &str,
    version: &str,
    attention: AttentionCounts,
    pending_count: usize,
) -> TitleLayout {
    let [name, ver, sep] = title_prefix_parts(version);
    let fixed_head: u16 = [&name, &ver]
        .iter()
        .map(|part| part.chars().count() as u16)
        .sum();
    let fixed = fixed_head.saturating_add(sep.chars().count() as u16);
    // The badge is ASCII-only, so byte length is its rendered width.
    let badge = badge_text(attention, pending_count);
    let badge_width = badge.as_ref().map_or(0, |b| b.len() as u16);

    let cwd_budget = title_width
        .saturating_sub(fixed)
        .saturating_sub(badge_width);
    let shown_cwd = shorten_cwd(cwd, cwd_budget);
    // A separator with nothing after it just spends a column on punctuation.
    let sep = if shown_cwd.is_empty() {
        String::new()
    } else {
        sep
    };

    let badge_start = [&sep, &shown_cwd].iter().fold(fixed_head, |acc, part| {
        acc.saturating_add(part.chars().count() as u16)
    });
    let badge = badge.and_then(|_| {
        let end = badge_start.saturating_add(badge_width).min(title_width);
        (badge_start < end).then_some(badge_start..end)
    });

    TitleLayout {
        prefix: [name, ver, sep, shown_cwd],
        badge,
    }
}

/// The screen columns where clicking opens the needs-attention picker, or
/// `None` when there is no badge or no room to show any of it.
///
/// Measures [`layout_title`], the same layout [`draw`] renders, so a badge the
/// help hint covers or a narrow row squeezed out is not clickable either.
pub fn badge_hit_columns(
    area: Rect,
    cwd: &str,
    version: &str,
    attention: AttentionCounts,
    pending_count: usize,
) -> Option<Range<u16>> {
    let inner_x = area.x.saturating_add(1);
    let (title_width, _) = split_header_row(area.width.saturating_sub(2));
    let columns = layout_title(title_width, cwd, version, attention, pending_count).badge?;
    Some(inner_x.saturating_add(columns.start)..inner_x.saturating_add(columns.end))
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

    let (title_width, hint_width) = split_header_row(inner.width);

    let layout = layout_title(title_width, cwd, version, attention, pending_count);
    let [name, ver, sep, dir] = layout.prefix;
    let mut title_spans = vec![
        Span::styled(
            name,
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(ver, Style::default().fg(theme.text_muted.to_color())),
        Span::styled(sep, Style::default().fg(theme.text_muted.to_color())),
        Span::styled(dir, Style::default().fg(theme.text.to_color())),
    ];

    // Only when `layout_title` found room for it, so what is drawn and what
    // `badge_hit_columns` reports as clickable cannot disagree.
    if layout.badge.is_some() {
        title_spans.extend(badge_spans(attention, pending_count, theme));
    }

    if title_width > 0 {
        // Stops short of the help hint rather than being painted over by it.
        let title_area = Rect {
            width: title_width,
            ..inner
        };
        let title = Paragraph::new(Line::from(title_spans));
        frame.render_widget(title, title_area);
    }

    if hint_width > 0 {
        let hint_area = Rect {
            x: inner.x.saturating_add(title_width),
            y: inner.y,
            width: hint_width,
            height: 1,
        };
        let hint = Paragraph::new(Line::from(vec![
            Span::styled("?", Style::default().fg(theme.primary.to_color())),
            Span::styled(" help ", Style::default().fg(theme.text_muted.to_color())),
        ]));
        frame.render_widget(hint, hint_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    /// Read row `y` of the rendered buffer back as a string.
    fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn a_narrow_header_keeps_the_badge_whole_and_the_help_hint_intact() {
        let width = 90;
        let backend = TestBackend::new(width, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        // Long enough that the untruncated title plus badge would run past the
        // row's end and under the help hint.
        let cwd = "/home/dev/code/agent-mainframe/.worktrees/some-long-branch";

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    Rect::new(0, 0, width, 3),
                    cwd,
                    "0.37.0",
                    1,
                    (1, 0, 0),
                    &theme,
                )
            })
            .expect("draw");

        let rendered = row(&terminal, 1);
        // The hint is whole, not half-eaten by the title...
        assert!(
            rendered.ends_with("? help │"),
            "help hint was overwritten: {rendered:?}"
        );
        // ...the badge kept every column, including the key it names...
        assert!(
            rendered.contains("[1 question, 1 input request | <leader i>]"),
            "badge was clipped: {rendered:?}"
        );
        // ...and the path is what gave way, marked as cut rather than silently
        // truncated.
        assert!(
            rendered.contains('…') && !rendered.contains("/home/dev"),
            "expected an elided cwd: {rendered:?}"
        );
    }

    #[test]
    fn the_title_never_paints_into_the_help_hint_at_any_width() {
        let theme = Theme::default();
        let cwd = "/home/dev/code/agent-mainframe/.worktrees/some-long-branch";

        // From the narrowest row that carries a hint at all: below this
        // `split_header_row` drops it and gives the columns to the title.
        for width in 10..120u16 {
            let backend = TestBackend::new(width, 3);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    draw(
                        frame,
                        Rect::new(0, 0, width, 3),
                        cwd,
                        "0.37.0",
                        1,
                        (1, 0, 0),
                        &theme,
                    )
                })
                .expect("draw");

            let rendered = row(&terminal, 1);
            assert!(
                rendered.ends_with("? help │"),
                "width {width}: help hint was overwritten: {rendered:?}"
            );
        }
    }

    #[test]
    fn the_help_hint_gives_way_to_the_title_on_a_row_too_narrow_for_both() {
        // Nothing but the hint would fit, and `? help ` alone says less than a
        // clipped product name does.
        assert_eq!(split_header_row(HELP_HINT_WIDTH), (HELP_HINT_WIDTH, 0));
        assert_eq!(split_header_row(0), (0, 0));
        assert_eq!(
            split_header_row(HELP_HINT_WIDTH + 1),
            (1, HELP_HINT_WIDTH),
            "one spare column belongs to the title"
        );
    }
}
