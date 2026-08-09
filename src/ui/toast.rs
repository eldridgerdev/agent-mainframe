use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::app::toast::{Toast, ToastKind};
use crate::theme::Theme;

/// Ceiling on how tall one toast may grow. A message longer than this is
/// telling the user too much for a transient popup, and letting it grow without
/// bound would push the others off the screen.
const MAX_TOAST_LINES: usize = 5;

pub(crate) fn draw_toasts(frame: &mut Frame, toasts: &[Toast], theme: &Theme) {
    if toasts.is_empty() {
        return;
    }

    let area = frame.area();
    let toast_width = 50_u16.min(area.width.saturating_sub(4));
    let gap: u16 = 1;
    // Inside the border, minus the one column of left padding the text carries.
    let text_width = toast_width.saturating_sub(3) as usize;

    // Stack upwards from the bottom. Heights vary with the message, so the
    // cursor walks rather than multiplying by a fixed step.
    let mut bottom = area.bottom().saturating_sub(1);

    for toast in toasts {
        let lines = wrap_message(&toast.message, text_width, MAX_TOAST_LINES);
        let toast_height = lines.len() as u16 + 2;
        if bottom < area.top() + toast_height {
            break;
        }
        let y = bottom - toast_height;
        let x = area.right().saturating_sub(toast_width + 1);
        let toast_rect = Rect::new(x, y, toast_width, toast_height);

        let border_color = match toast.kind {
            ToastKind::Success => theme.success.to_color(),
            ToastKind::Info => theme.info.to_color(),
            ToastKind::Warning => theme.warning.to_color(),
            ToastKind::Error => theme.danger.to_color(),
        };

        let label = match toast.kind {
            ToastKind::Success => " ✓ ",
            ToastKind::Info => " i ",
            ToastKind::Warning => " ! ",
            ToastKind::Error => " ✕ ",
        };

        frame.render_widget(Clear, toast_rect);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(label, Style::default().fg(border_color)));

        let inner = block.inner(toast_rect);
        frame.render_widget(block, toast_rect);

        let mut style = Style::default().fg(theme.text.to_color());
        if toast.age_fraction() > 0.75 {
            style = style.add_modifier(Modifier::DIM);
        }

        let body: Vec<Line> = lines
            .into_iter()
            .map(|line| Line::from(Span::styled(format!(" {line}"), style)))
            .collect();
        frame.render_widget(Paragraph::new(body), inner);

        bottom = y.saturating_sub(gap);
    }
}

/// Word-wrap `message` to `width` display columns, at most `max_lines` lines.
///
/// Measured in display width rather than bytes so wide glyphs and the em dashes
/// AMF's own messages use don't overflow the box — and sliced on character
/// boundaries, never byte offsets, which a message carrying any non-ASCII would
/// otherwise panic on.
fn wrap_message(message: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return vec![String::new()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in message.split_whitespace() {
        // A word too long to ever fit is broken across lines rather than
        // overflowing; anything else keeps its word boundaries.
        if UnicodeWidthStr::width(word) > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            for chunk in split_to_width(word, width) {
                lines.push(chunk);
            }
            current = lines.pop().unwrap_or_default();
            continue;
        }

        let candidate_width = if current.is_empty() {
            UnicodeWidthStr::width(word)
        } else {
            UnicodeWidthStr::width(current.as_str()) + 1 + UnicodeWidthStr::width(word)
        };
        if candidate_width > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }

    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            // Make room for the ellipsis rather than adding to an already-full
            // line.
            while UnicodeWidthStr::width(last.as_str()) + 1 > width && !last.is_empty() {
                last.pop();
            }
            last.push('…');
        }
    }

    lines
}

/// Split one long word into `width`-wide chunks, on character boundaries.
fn split_to_width(word: &str, width: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    for ch in word.chars() {
        let ch_width = UnicodeWidthStr::width(ch.to_string().as_str());
        if UnicodeWidthStr::width(chunk.as_str()) + ch_width > width && !chunk.is_empty() {
            chunks.push(std::mem::take(&mut chunk));
        }
        chunk.push(ch);
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_message_stays_on_one_line() {
        assert_eq!(
            wrap_message("Started 'alpha'", 40, 5),
            vec!["Started 'alpha'"]
        );
    }

    #[test]
    fn a_long_message_wraps_on_word_boundaries_instead_of_truncating() {
        let message = "'cache-eviction' created but not started: 1 agent already running (limit 1). \
             Press c to start it.";
        let lines = wrap_message(message, 46, 5);

        assert!(lines.len() > 1, "expected a wrap, got {lines:?}");
        for line in &lines {
            assert!(
                UnicodeWidthStr::width(line.as_str()) <= 46,
                "line over width: {line:?}"
            );
        }
        // Nothing is lost: the whole message is readable across the lines.
        assert_eq!(
            lines.join(" "),
            message.split_whitespace().collect::<Vec<_>>().join(" ")
        );
        assert!(!lines.last().unwrap().ends_with('…'));
    }

    #[test]
    fn wrapping_never_splits_a_multibyte_character() {
        // The em dash and arrow are what a byte-sliced truncation used to
        // panic on.
        let message = "VS Code — search-ranking → closed 2 processes ✓";
        for width in 4..30 {
            let lines = wrap_message(message, width, 5);
            for line in lines {
                assert!(line.chars().count() > 0 || width == 0);
            }
        }
    }

    #[test]
    fn a_word_longer_than_the_box_is_broken_rather_than_overflowing() {
        let path = "/home/user/code/project/.worktrees/project-some-very-long-feature";
        let lines = wrap_message(path, 20, 5);
        for line in &lines {
            assert!(
                UnicodeWidthStr::width(line.as_str()) <= 20,
                "line over width: {line:?}"
            );
        }
    }

    #[test]
    fn an_over_long_message_is_capped_with_an_ellipsis() {
        let message = "word ".repeat(200);
        let lines = wrap_message(&message, 20, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines.last().unwrap().ends_with('…'));
        assert!(UnicodeWidthStr::width(lines.last().unwrap().as_str()) <= 20);
    }

    #[test]
    fn a_degenerate_width_does_not_panic() {
        assert_eq!(wrap_message("anything", 0, 5), vec![String::new()]);
        assert_eq!(wrap_message("anything", 10, 0), vec![String::new()]);
    }
}
