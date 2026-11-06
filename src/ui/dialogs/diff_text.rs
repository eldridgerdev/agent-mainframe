use std::path::Path;

use ratatui::{style::Style, text::Span};
use unicode_width::UnicodeWidthStr;

use crate::{diff::DiffFile, highlight, theme::Theme};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StyledChunk {
    pub(super) text: String,
    pub(super) style: Style,
}

pub(super) struct FileHighlights {
    pub(super) old: Option<highlight::HighlightedText>,
    pub(super) new: Option<highlight::HighlightedText>,
}

pub(super) fn diff_chunks(
    text: &str,
    row_style: Style,
    theme: &Theme,
    highlighted_line: Option<&highlight::HighlightedLine>,
) -> Vec<StyledChunk> {
    diff_chunks_emphasized(text, row_style, theme, highlighted_line, None)
}

/// As `diff_chunks`, but additionally brightens the byte ranges in `emphasis`
/// (offsets into the line's content, prefix excluded) — the tokens a
/// word-level diff found actually changed. Only the background is touched, so
/// syntax highlighting shows through unchanged.
pub(super) fn diff_chunks_emphasized(
    text: &str,
    row_style: Style,
    theme: &Theme,
    highlighted_line: Option<&highlight::HighlightedLine>,
    emphasis: Option<&IntraLineEmphasis>,
) -> Vec<StyledChunk> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut chars = text.chars();
    let first = chars.next().expect("diff chunk text should not be empty");
    let (prefix, content) = if matches!(first, '+' | '-' | ' ') {
        (Some(first), chars.as_str())
    } else {
        (None, text)
    };

    let mut chunks = Vec::new();
    if let Some(prefix) = prefix {
        chunks.push(StyledChunk {
            text: prefix.to_string(),
            style: row_style,
        });
    }

    if !content.is_empty() {
        let mut content_chunks = Vec::new();
        append_highlighted_content(
            &mut content_chunks,
            content,
            row_style,
            theme,
            highlighted_line,
        );
        if let Some(emphasis) = emphasis {
            content_chunks =
                apply_intra_line_emphasis(content_chunks, &emphasis.ranges, emphasis.style);
        }
        chunks.extend(content_chunks);
    }

    chunks
}

/// The changed byte ranges of one line plus the style to mark them with.
pub(super) struct IntraLineEmphasis {
    pub(super) ranges: Vec<std::ops::Range<usize>>,
    pub(super) style: Style,
}

/// Split `chunks` at every emphasis boundary and patch `style` onto the parts
/// that fall inside a changed range. `chunks` must cover the line's content
/// contiguously from offset 0, which is what `append_highlighted_content`
/// guarantees.
pub(super) fn apply_intra_line_emphasis(
    chunks: Vec<StyledChunk>,
    ranges: &[std::ops::Range<usize>],
    style: Style,
) -> Vec<StyledChunk> {
    if ranges.is_empty() {
        return chunks;
    }
    let mut rebuilt: Vec<StyledChunk> = Vec::with_capacity(chunks.len());
    let mut offset = 0usize;
    for chunk in chunks.iter() {
        let chunk_start = offset;
        offset += chunk.text.len();
        // Cut points inside this chunk where emphasis starts or stops.
        let mut cuts: Vec<usize> = vec![0, chunk.text.len()];
        for range in ranges {
            for edge in [range.start, range.end] {
                if edge > chunk_start && edge < offset {
                    cuts.push(edge - chunk_start);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let Some(text) = chunk.text.get(from..to) else {
                // A cut landed mid-character: keep the chunk whole rather than
                // slicing a UTF-8 boundary.
                continue;
            };
            let absolute = chunk_start + from;
            let emphasized = ranges
                .iter()
                .any(|range| absolute >= range.start && absolute < range.end);
            rebuilt.push(StyledChunk {
                text: text.to_string(),
                style: if emphasized {
                    chunk.style.patch(style)
                } else {
                    chunk.style
                },
            });
        }
    }
    rebuilt
}

pub(super) fn append_highlighted_content(
    chunks: &mut Vec<StyledChunk>,
    content: &str,
    row_style: Style,
    theme: &Theme,
    highlighted_line: Option<&highlight::HighlightedLine>,
) {
    let Some(highlighted_line) = highlighted_line else {
        chunks.push(StyledChunk {
            text: content.to_string(),
            style: row_style,
        });
        return;
    };

    if highlighted_line.spans.is_empty() {
        chunks.push(StyledChunk {
            text: content.to_string(),
            style: row_style,
        });
        return;
    }

    let mut rendered_any = false;
    let mut remaining = content;
    for span in &highlighted_line.spans {
        if remaining.is_empty() {
            break;
        }
        if span.text.is_empty() {
            continue;
        }

        let consumed = consume_shared_prefix(remaining, &span.text);
        if consumed.is_empty() {
            continue;
        }

        chunks.push(StyledChunk {
            text: consumed.to_string(),
            style: row_style.patch(highlight::style_for_class(span.class, theme)),
        });
        remaining = &remaining[consumed.len()..];
        rendered_any = true;
    }

    if !remaining.is_empty() {
        chunks.push(StyledChunk {
            text: remaining.to_string(),
            style: row_style,
        });
    } else if !rendered_any {
        chunks.push(StyledChunk {
            text: content.to_string(),
            style: row_style,
        });
    }
}

pub(super) fn consume_shared_prefix<'a>(content: &'a str, highlighted: &str) -> &'a str {
    let mut end = 0usize;
    for (left, right) in content.chars().zip(highlighted.chars()) {
        if left != right {
            break;
        }
        end += left.len_utf8();
    }
    &content[..end]
}

pub(super) fn file_highlights(file: &DiffFile) -> FileHighlights {
    let old = file.old_content.as_deref().map(|source| {
        highlight::highlight_source(highlight::HighlightRequest {
            path: file.old_path.as_deref().map(Path::new),
            language_hint: None,
            source,
        })
    });
    let new = file.new_content.as_deref().map(|source| {
        highlight::highlight_source(highlight::HighlightRequest {
            path: Some(Path::new(&file.path)),
            language_hint: None,
            source,
        })
    });

    FileHighlights { old, new }
}

pub(super) fn highlighted_line(
    highlighted: Option<&highlight::HighlightedText>,
    line_number: usize,
) -> Option<&highlight::HighlightedLine> {
    line_number
        .checked_sub(1)
        .and_then(|index| highlighted.and_then(|text| text.lines.get(index)))
}

pub(super) fn plain_chunks(text: &str, style: Style) -> Vec<StyledChunk> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![StyledChunk {
            text: text.to_string(),
            style,
        }]
    }
}

pub(super) fn wrap_chunks(
    chunks: &[StyledChunk],
    width: usize,
    fallback_style: Style,
) -> Vec<Vec<StyledChunk>> {
    if width == 0 {
        return vec![Vec::new()];
    }
    if chunks.is_empty() {
        return vec![Vec::new()];
    }

    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut used = 0usize;

    for chunk in chunks {
        for ch in chunk.text.chars() {
            let ch_width = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4])).max(1);
            if used + ch_width > width && !current.is_empty() {
                lines.push(current);
                current = Vec::new();
                used = 0;
            }

            push_chunk_char(&mut current, chunk.style, ch);
            used += ch_width;

            if used >= width {
                lines.push(current);
                current = Vec::new();
                used = 0;
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(plain_chunks("", fallback_style));
    }

    lines
}

pub(super) fn push_chunk_char(chunks: &mut Vec<StyledChunk>, style: Style, ch: char) {
    if let Some(last) = chunks.last_mut()
        && last.style == style
    {
        last.text.push(ch);
        return;
    }
    chunks.push(StyledChunk {
        text: ch.to_string(),
        style,
    });
}

pub(super) fn chunks_to_spans(chunks: Vec<StyledChunk>) -> Vec<Span<'static>> {
    chunks
        .into_iter()
        .map(|chunk| Span::styled(chunk.text, chunk.style))
        .collect()
}

pub(super) fn pad_chunks_to_width(
    mut chunks: Vec<StyledChunk>,
    width: usize,
    pad_style: Style,
) -> Vec<StyledChunk> {
    let used = chunks_width(&chunks);
    if used < width {
        chunks.push(StyledChunk {
            text: " ".repeat(width - used),
            style: pad_style,
        });
    }
    chunks
}

pub(super) fn chunks_width(chunks: &[StyledChunk]) -> usize {
    chunks
        .iter()
        .map(|chunk| UnicodeWidthStr::width(chunk.text.as_str()))
        .sum()
}
