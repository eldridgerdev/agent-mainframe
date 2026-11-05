use std::path::Path;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use super::{status_color, status_label};
use crate::{
    app::{AMF_GROUP_KEY, DiffViewerFocus, DiffViewerState, FileTreeRow, ReviewDecision},
    diff::DiffFile,
    theme::Theme,
};

/// A changed file whose diff crosses this many total added+removed lines is
/// flagged `L` (large) in the file list — big enough that a reviewer skimming
/// the list should expect it to take real time, small enough that it still
/// fires on plenty of ordinary changes worth flagging.
const LARGE_FILE_CHANGE_THRESHOLD: usize = 300;

/// True if any file in the changeset looks like a test file. Used as the
/// changeset-wide signal behind the file list's "no test coverage" marker:
/// there is no per-file test-mapping convention anywhere in this codebase (a
/// module's tests usually live in a sibling `tests.rs` or an inline
/// `#[cfg(test)]` block elsewhere), so this settles for "did the changeset
/// touch anything test-shaped at all".
pub(super) fn changeset_has_test_changes(files: &[DiffFile]) -> bool {
    files.iter().any(|f| looks_like_test_path(&f.path))
}

fn looks_like_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.ends_with("tests.rs")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.contains("test_")
        || lower.contains("_spec.")
}

/// Non-test source extensions the "no test coverage" marker considers —
/// config/docs/markdown/lockfiles etc. are excluded since "no tests touched
/// this README" isn't a useful signal.
fn looks_like_source_path(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "rb"
            | "java"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "cs"
            | "swift"
            | "kt"
    )
}

/// Build the `[L,N,T]`-style risk-marker span for a file's row (empty when no
/// flag applies): `L` large change, `N` no developer note / walkthrough yet,
/// `T` changeset has no test-looking file at all. Review mode only, mirroring
/// the `Δ` changed-since-last marker.
pub(super) fn file_risk_marker(
    file: &DiffFile,
    state: &DiffViewerState,
    changeset_has_tests: bool,
    theme: &Theme,
) -> Option<Span<'static>> {
    if !state.review || file.is_binary {
        return None;
    }
    let mut flags = Vec::new();
    if file.additions + file.deletions >= LARGE_FILE_CHANGE_THRESHOLD {
        flags.push("L");
    }
    // "No developer note" means nothing in a PR review, which has none.
    if !state.review_notes.contains_key(&file.path)
        && !state.generated_notes.contains_key(&file.path)
        && !state.is_pr_review()
    {
        flags.push("N");
    }
    if !changeset_has_tests
        && looks_like_source_path(&file.path)
        && !looks_like_test_path(&file.path)
    {
        flags.push("T");
    }
    if flags.is_empty() {
        return None;
    }
    Some(Span::styled(
        format!(" [{}]", flags.join(",")),
        Style::default().fg(theme.warning.to_color()),
    ))
}

/// A collapsed directory's row summarises what it is hiding, so folding a tree
/// never hides the fact that something under it still needs attention: how many
/// files, how many still undecided, and whether any changed since the last
/// review round. Every count comes from `visible` — the files the active filter
/// shows — so the badges always describe the same set as the row's `(n)`, which
/// the tree also counts per filter.
pub(super) fn dir_row_summary(
    dir: &str,
    files: usize,
    visible: &[usize],
    state: &DiffViewerState,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let prefix = format!("{dir}/");
    let amf_group = dir == AMF_GROUP_KEY;
    let under: Vec<&DiffFile> = visible
        .iter()
        .filter(|&&idx| !amf_group || state.is_amf_file(idx))
        .filter_map(|&idx| state.files.get(idx))
        .filter(|file| amf_group || file.path.starts_with(&prefix))
        .collect();
    let mut spans = vec![Span::styled(
        format!("  ({files})"),
        Style::default().fg(theme.text_muted.to_color()),
    )];
    if !state.review {
        return spans;
    }
    let undecided = under
        .iter()
        .filter(|file| !state.decisions.contains_key(&file.path))
        .count();
    if undecided > 0 {
        spans.push(Span::styled(
            format!(" ·{undecided}"),
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }
    let rejected = under
        .iter()
        .filter(|file| {
            matches!(
                state.decisions.get(&file.path),
                Some(ReviewDecision::Reject { .. })
            )
        })
        .count();
    if rejected > 0 {
        spans.push(Span::styled(
            format!(" ✗{rejected}"),
            Style::default().fg(theme.danger.to_color()),
        ));
    }
    if under
        .iter()
        .any(|file| state.changed_since_last.contains(&file.path))
    {
        spans.push(Span::styled(
            " Δ",
            Style::default().fg(theme.warning.to_color()),
        ));
    }
    spans
}

/// The row holding AMF's own files (`DiffViewerState::amf_file_count`).
/// Folding them by default is only acceptable because nothing about it is
/// silent: this row carries the count, the header says how many are AMF's,
/// and the footer names the key that shows them.
fn amf_group_row(
    collapsed: bool,
    files: usize,
    visible: &[usize],
    state: &DiffViewerState,
    theme: &Theme,
) -> Line<'static> {
    let muted = Style::default().fg(theme.text_muted.to_color());
    let mut spans = vec![
        Span::styled(if collapsed { "▸ " } else { "▾ " }, muted),
        Span::styled(
            format!("AMF files ({files})"),
            Style::default()
                .fg(theme.info.to_color())
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if collapsed && state.review {
        // Verdict badges only; the `(n)` is already part of the label.
        spans.extend(
            dir_row_summary(AMF_GROUP_KEY, files, visible, state, theme)
                .into_iter()
                .skip(1),
        );
    }
    // The header says what these are and the footer names `.`; the file list
    // is narrow, so the row keeps to what fits beside the count.
    if collapsed {
        spans.push(Span::styled(" · not yours", muted));
    }
    Line::from(spans)
}

pub(in crate::ui::dialogs) fn draw_file_list(
    frame: &mut Frame,
    area: Rect,
    state: &DiffViewerState,
    theme: &Theme,
) {
    let visible = state.visible_file_indices();
    let rows = state.file_tree_rows();
    let changeset_has_tests = changeset_has_test_changes(&state.files);
    let mut items: Vec<ListItem<'static>> = rows
        .iter()
        .map(|row| {
            let (idx, depth) = match row {
                FileTreeRow::Dir {
                    path,
                    label,
                    depth,
                    collapsed,
                    files,
                } => {
                    if path == AMF_GROUP_KEY {
                        return ListItem::new(amf_group_row(
                            *collapsed, *files, &visible, state, theme,
                        ));
                    }
                    let mut spans = vec![
                        Span::raw(" ".repeat(*depth)),
                        Span::styled(
                            if *collapsed { "▸ " } else { "▾ " },
                            Style::default().fg(theme.text_muted.to_color()),
                        ),
                        Span::styled(
                            format!("{label}/"),
                            Style::default()
                                .fg(theme.info.to_color())
                                .add_modifier(Modifier::BOLD),
                        ),
                    ];
                    if *collapsed {
                        spans.extend(dir_row_summary(path, *files, &visible, state, theme));
                    }
                    return ListItem::new(Line::from(spans));
                }
                FileTreeRow::File { index, depth, .. } => (*index, *depth),
            };
            let file = &state.files[idx];
            let status_style = Style::default()
                .fg(status_color(&file.status, theme))
                .add_modifier(Modifier::BOLD);
            let mut spans = Vec::new();
            if depth > 0 {
                spans.push(Span::raw(" ".repeat(depth)));
            }
            if state.review {
                let (symbol, color) = match state.decisions.get(&file.path) {
                    Some(ReviewDecision::Approve) => ("✓", theme.success.to_color()),
                    Some(ReviewDecision::Reject { .. }) => ("✗", theme.danger.to_color()),
                    None => ("·", theme.text_muted.to_color()),
                };
                spans.push(Span::styled(
                    format!("{symbol} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::styled(
                format!("{} ", status_label(&file.status)),
                status_style,
            ));
            // Flag files that changed since the last finished review round so
            // they stand out when re-reviewing.
            if state.review && state.changed_since_last.contains(&file.path) {
                spans.push(Span::styled(
                    "Δ ",
                    Style::default()
                        .fg(theme.warning.to_color())
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if state.review
                && let Some(comment) = state.file_comments.get(&file.path)
            {
                let (marker, color) = if comment.resolved {
                    ("◇ ", theme.text_muted.to_color())
                } else {
                    ("◆ ", theme.info.to_color())
                };
                spans.push(Span::styled(
                    marker,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
            // Only the basename: the directories are the rows above it.
            spans.push(Span::styled(
                match row {
                    FileTreeRow::File { name, .. } => name.clone(),
                    FileTreeRow::Dir { .. } => file.path.clone(),
                },
                Style::default().fg(theme.text.to_color()),
            ));
            spans.push(Span::styled(
                format!("  +{} -{}", file.additions, file.deletions),
                Style::default().fg(theme.text_muted.to_color()),
            ));
            if let Some(marker) = file_risk_marker(file, state, changeset_has_tests, theme) {
                spans.push(marker);
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    // A filter that matches nothing shows a muted placeholder rather than an
    // empty box.
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            format!(" (no {} files) ", state.file_filter.label()),
            Style::default().fg(theme.text_muted.to_color()),
        ))));
    }

    let border = if state.focus == DiffViewerFocus::FileList {
        theme.warning.to_color()
    } else {
        theme.primary.to_color()
    };
    let title = if state.review && state.file_filter != crate::app::FileFilter::All {
        format!(
            " Files ({}/{}) · {} ",
            visible.len(),
            state.files.len(),
            state.file_filter.label()
        )
    } else {
        format!(" Files ({}) ", state.files.len())
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.shortcut_background.to_color())
                .fg(theme.shortcut_text.to_color())
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">");

    let mut list_state = ListState::default();
    // Highlight maps onto the rendered tree rows: the cursored directory when
    // the cursor is parked on one, else the selected file's row. None when the
    // selection is hidden by the active filter.
    list_state.select(state.tree_cursor_row(&rows));
    frame.render_stateful_widget(list, area, &mut list_state);
}
