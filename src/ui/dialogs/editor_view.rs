//! Shared rendering helpers for `TextEditor`-backed dialogs (steering
//! prompt, compose input): cursor overlay, wrap-aware line counting,
//! and scroll synchronization.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::editor::TextEditor;
use crate::theme::Theme;

/// Editor text as styled lines with a block cursor inserted. When the
/// buffer is empty, shows the cursor followed by a muted placeholder.
pub(crate) fn editor_lines(
    editor: &TextEditor,
    theme: &Theme,
    placeholder: &str,
) -> Vec<Line<'static>> {
    if editor.text().is_empty() {
        return vec![
            Line::from(Span::styled(
                "\u{2588}",
                Style::default().fg(theme.primary.to_color()),
            )),
            Line::from(Span::styled(
                placeholder.to_string(),
                Style::default().fg(theme.text_muted.to_color()),
            )),
        ];
    }

    let mut lines = editor
        .text()
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let (cursor_row, cursor_col) = editor.cursor_row_col();
    while lines.len() <= cursor_row {
        lines.push(String::new());
    }
    if let Some(line) = lines.get_mut(cursor_row) {
        let insert_at = char_col_to_byte_idx(line, cursor_col);
        line.insert(insert_at, '\u{2588}');
    }

    lines
        .into_iter()
        .map(|line| {
            Line::from(Span::styled(
                line,
                Style::default().fg(theme.text.to_color()),
            ))
        })
        .collect()
}

pub(crate) fn char_col_to_byte_idx(text: &str, char_col: usize) -> usize {
    text.char_indices()
        .nth(char_col)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

pub(crate) fn count_wrapped_editor_lines(lines: &[Line<'static>], width: usize) -> usize {
    if width == 0 {
        return 0;
    }

    lines
        .iter()
        .map(|line| {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            UnicodeWidthStr::width(text.as_str()).max(1).div_ceil(width)
        })
        .sum()
}

pub(crate) fn editor_cursor_visual_row(editor: &TextEditor, width: usize) -> usize {
    if width == 0 {
        return 0;
    }

    let mut lines = editor
        .text()
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let (cursor_row, cursor_col) = editor.cursor_row_col();
    while lines.len() <= cursor_row {
        lines.push(String::new());
    }

    let wrapped_before_cursor = lines
        .iter()
        .take(cursor_row)
        .map(|line| UnicodeWidthStr::width(line.as_str()).max(1).div_ceil(width))
        .sum::<usize>();
    let current_line = lines
        .get(cursor_row)
        .map(String::as_str)
        .unwrap_or_default();
    let cursor_byte = char_col_to_byte_idx(current_line, cursor_col);
    let cursor_prefix = &current_line[..cursor_byte];
    wrapped_before_cursor + UnicodeWidthStr::width(cursor_prefix).div_euclid(width)
}

/// Keep the cursor visible when requested, then clamp the scroll
/// offset to the wrapped content height.
pub(crate) fn sync_editor_scroll(
    editor: &TextEditor,
    scroll_offset: &mut usize,
    sync_to_cursor: &mut bool,
    visible_lines: usize,
    wrap_width: usize,
    total_visual_lines: usize,
) {
    if *sync_to_cursor && visible_lines > 0 && wrap_width > 0 {
        let cursor_row = editor_cursor_visual_row(editor, wrap_width);
        if cursor_row < *scroll_offset {
            *scroll_offset = cursor_row;
        } else if cursor_row >= scroll_offset.saturating_add(visible_lines) {
            *scroll_offset = cursor_row + 1 - visible_lines;
        }
        *sync_to_cursor = false;
    }

    let max_scroll = total_visual_lines.saturating_sub(visible_lines);
    *scroll_offset = (*scroll_offset).min(max_scroll);
}
