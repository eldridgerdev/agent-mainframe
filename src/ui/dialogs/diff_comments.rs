use crate::{app::DiffViewerState, diff::DiffLineLocation};

pub(super) struct ReviewLineMarkers {
    pub(super) cursor: Option<DiffLineLocation>,
    pub(super) commented: std::collections::HashSet<DiffLineLocation>,
    /// Lines covered by an unaccepted AI draft comment — marked distinctly from
    /// `commented` so the reviewer can tell suggestions apart from kept comments.
    pub(super) draft: std::collections::HashSet<DiffLineLocation>,
    /// Subset of `commented` covered by a `Blocker`-severity comment, so the
    /// gutter can flag must-fix lines in a higher-contrast colour.
    pub(super) blocker: std::collections::HashSet<DiffLineLocation>,
    pub(super) selection: std::collections::HashSet<DiffLineLocation>,
    /// Lines matching the active diff search, so the gutter can mark every hit
    /// (the current match is already the cursor).
    pub(super) matched: std::collections::HashSet<DiffLineLocation>,
}

pub(super) fn review_cursor_info(state: &DiffViewerState) -> ReviewLineMarkers {
    use std::collections::HashSet;
    let empty = || ReviewLineMarkers {
        cursor: None,
        commented: HashSet::new(),
        draft: HashSet::new(),
        blocker: HashSet::new(),
        selection: HashSet::new(),
        matched: HashSet::new(),
    };
    if !state.review {
        return empty();
    }
    let Some(file) = state.files.get(state.selected_file) else {
        return empty();
    };
    let locs = file.addressable_lines();
    let cursor = state.comment_cursor.and_then(|idx| locs.get(idx).copied());
    let mut commented = HashSet::new();
    let mut draft = HashSet::new();
    let mut blocker = HashSet::new();
    if let Some(comments) = state.line_comments.get(&file.path) {
        for comment in comments {
            if let Some(range) = comment.covered_indices(&locs) {
                let covered: Vec<DiffLineLocation> =
                    range.filter_map(|idx| locs.get(idx).copied()).collect();
                if comment.draft {
                    draft.extend(covered);
                } else {
                    if comment.severity.is_blocker() {
                        blocker.extend(covered.iter().copied());
                    }
                    commented.extend(covered);
                }
            }
        }
    }
    // The active selection span (only meaningful while an anchor is set).
    let selection = match (state.comment_anchor, state.comment_cursor) {
        (Some(anchor), Some(cur)) => (anchor.min(cur)..=anchor.max(cur))
            .filter_map(|idx| locs.get(idx).copied())
            .collect(),
        _ => HashSet::new(),
    };
    // Every diff-search hit in the current file (empty when no search is active).
    let matched = state
        .search_matches
        .iter()
        .filter_map(|&idx| locs.get(idx).copied())
        .collect();
    ReviewLineMarkers {
        cursor,
        commented,
        draft,
        blocker,
        selection,
        matched,
    }
}

/// The review comment whose span covers the line the comment cursor is
/// currently on, if any. `None` outside review mode, when no cursor is active,
/// or when the cursored line carries no comment.
pub(super) fn cursor_comment(state: &DiffViewerState) -> Option<&crate::app::LineComment> {
    if !state.review {
        return None;
    }
    let cursor = state.comment_cursor?;
    let file = state.files.get(state.selected_file)?;
    let locs = file.addressable_lines();
    state.line_comments.get(&file.path)?.iter().find(|c| {
        c.covered_indices(&locs)
            .is_some_and(|range| range.contains(&cursor))
    })
}

/// The body text of [`cursor_comment`].
#[cfg(test)]
pub(super) fn cursor_comment_text(state: &DiffViewerState) -> Option<&str> {
    cursor_comment(state).map(|c| c.text.as_str())
}

/// The lines shown in the cursor-comment peek box: the prose (if any) followed
/// by a labelled preview of the suggested change (if any). Never empty.
pub(super) fn cursor_comment_peek_lines(comment: &crate::app::LineComment) -> Vec<String> {
    let mut lines: Vec<String> = comment.text.lines().map(|l| l.to_string()).collect();
    // Lead a kept comment's peek with its severity so a reviewer scrolling the
    // diff sees each annotation's priority. Drafts always default to the neutral
    // severity, so tagging them would be noise.
    if !comment.draft {
        lines.insert(0, format!("[{}]", comment.severity.label()));
    }
    if let Some(suggestion) = &comment.suggestion {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("suggested change:".to_string());
        lines.extend(suggestion.lines().map(|l| l.to_string()));
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Height (in rows, including the box border) the footer needs to peek the
/// comment on the cursored line; `0` when there is nothing to preview. The body
/// is capped so a long comment is glimpsed rather than fully scrolled — the
/// editor (Enter) still shows it in full.
pub(super) fn cursor_comment_preview_rows(state: &DiffViewerState) -> u16 {
    match cursor_comment(state) {
        Some(comment) => {
            let content = cursor_comment_peek_lines(comment).len().clamp(1, 6) as u16;
            content + 2
        }
        None => 0,
    }
}
