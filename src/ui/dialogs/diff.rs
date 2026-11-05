use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::path::Path;
use unicode_width::UnicodeWidthStr;

#[cfg(test)]
use crate::app::{DiffPickerState, ReviewDecision};

use crate::{
    app::{DiffScope, DiffViewerFocus, DiffViewerLayout, DiffViewerState},
    diff::{DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind, DiffLineLocation},
    highlight,
    theme::Theme,
};

use super::super::dashboard::centered_rect;

#[path = "diff_picker.rs"]
mod diff_picker;
pub use diff_picker::draw_diff_picker;
/// Base-selection guidance for a current-changes view without a usable base.
/// Either the diff fell back to `HEAD` or the selected base could not be read.
pub(super) const CHOOSE_BASE_HINT: &str = "Press b to choose a base ref.";

#[path = "diff_file_tree.rs"]
mod diff_file_tree;
pub(in crate::ui::dialogs) use diff_file_tree::draw_file_list;
#[cfg(test)]
use diff_file_tree::{changeset_has_test_changes, dir_row_summary, file_risk_marker};
#[path = "diff_text.rs"]
mod diff_text;
use diff_text::{
    IntraLineEmphasis, StyledChunk, chunks_to_spans, diff_chunks, diff_chunks_emphasized,
    file_highlights, highlighted_line, pad_chunks_to_width, plain_chunks, wrap_chunks,
};
#[path = "diff_overlays.rs"]
mod diff_overlays;
#[cfg(test)]
use diff_overlays::{REVIEW_HELP_SECTIONS, review_help_sections};
use diff_overlays::{
    draw_base_ref_prompt, draw_changeset_overview_modal, draw_interdiff_modal,
    draw_pr_submit_modal, draw_review_help_modal, draw_review_history_modal,
    draw_review_summary_modal, draw_search_prompt,
};
#[path = "diff_comments.rs"]
mod diff_comments;
#[cfg(test)]
use diff_comments::cursor_comment_text;
use diff_comments::{
    ReviewLineMarkers, cursor_comment, cursor_comment_peek_lines, cursor_comment_preview_rows,
    review_cursor_info,
};
#[path = "diff_body.rs"]
mod diff_body;
use diff_body::draw_body;
#[cfg(test)]
use diff_body::pr_review_notes_markdown;
#[path = "diff_header.rs"]
mod diff_header;
#[cfg(test)]
use diff_header::amf_split_count_label;
use diff_header::draw_header;
#[path = "diff_footer.rs"]
mod diff_footer;
#[cfg(test)]
use diff_footer::{diff_footer_lines, review_hint_lines, wrapped_line_height};
use diff_footer::{draw_footer, review_hint_height};

pub fn draw_diff_viewer(frame: &mut Frame, state: &mut DiffViewerState, theme: &Theme) {
    if state.questions.open {
        super::review_questions::draw(frame, &mut state.questions, theme);
        return;
    }
    let area = centered_rect(96, 90, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let border_color = if state.error.is_some() {
        theme.danger.to_color()
    } else {
        theme.primary.to_color()
    };

    let block = Block::default()
        .title(if matches!(&state.scope, DiffScope::PullRequest(_)) {
            " PR Review "
        } else if state.review {
            " Final Review "
        } else if matches!(&state.scope, DiffScope::Commit(_)) {
            " Commit Diff "
        } else {
            " Current Changes "
        })
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The review footer grows to host the multi-line feedback editor while it
    // is open; otherwise it is a two-row key hint.
    let footer_height = if state.review
        && (state.feedback_editing
            || state.editing_general
            || state.editing_line_comment
            || state.editing_file_comment
            || state.editing_suggestion)
    {
        inner.height.saturating_sub(10).clamp(4, 12)
    } else if state.review {
        // Grow to fit the hints themselves — the verdict row is dense enough to
        // wrap past two rows on an ordinary terminal — plus the line-comment
        // peek box when the cursor is parked on a line that carries a comment.
        review_hint_height(state, theme, inner) + cursor_comment_preview_rows(state)
    } else {
        2
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(footer_height),
        ])
        .split(inner);

    draw_header(frame, chunks[0], state, theme);
    draw_body(frame, chunks[1], state, theme);
    draw_footer(frame, chunks[2], state, theme);

    if state.editing_base_ref {
        draw_base_ref_prompt(frame, state, theme);
    }
    if state.editing_search {
        draw_search_prompt(frame, state, theme);
    }
    if state.changeset_overview_open {
        draw_changeset_overview_modal(frame, state, theme);
    }
    if state.interdiff_open {
        draw_interdiff_modal(frame, state, theme);
    }
    if state.summary_open {
        draw_review_summary_modal(frame, state, theme);
    }
    if state.review_history.is_some() {
        draw_review_history_modal(frame, state, theme);
    }
    // Drawn last so it sits on top of anything else the reviewer left open.
    if state.help_open {
        draw_review_help_modal(frame, state, theme);
    }
    // The destination picker and the companion-feature setup it can open sit on
    // top of everything (they capture every key while shown).
    if let Some(pick) = &state.destination_pick {
        super::draw_review_destination_pick(frame, pick, theme);
    }
    if let Some(setup) = &state.review_feature_setup {
        super::draw_review_feature_setup(frame, setup, theme);
    }
    // Hidden while its summary is being edited, so the editor is visible.
    if state.pr_submit.is_some() && !state.editing_general {
        draw_pr_submit_modal(frame, state, theme);
    }
}

pub fn draw_diff_viewer_loading(
    frame: &mut Frame,
    state: &DiffViewerState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(54, 28, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let branch = if state.branch.is_empty() {
        state.from_view.feature_name.as_str()
    } else {
        state.branch.as_str()
    };
    let (title, loading_label, detail) = match &state.scope {
        DiffScope::Commit(commit) => (
            " Commit Diff ",
            " Loading commit diff...",
            format!("{}  {}", commit.short_hash, commit.subject),
        ),
        DiffScope::CurrentChanges => (
            " Current Changes ",
            " Loading current changes...",
            format!("Comparing all changes for {branch}"),
        ),
        DiffScope::PullRequest(target) => (
            " PR Review ",
            " Loading pull request...",
            format!("#{}  {}", target.pr.number, target.pr.title),
        ),
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let loading = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            spinner,
            Span::styled(
                loading_label,
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            detail,
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });

    frame.render_widget(loading, inner);
}

pub(super) fn draw_patch(
    frame: &mut Frame,
    area: Rect,
    state: &mut DiffViewerState,
    theme: &Theme,
) {
    let border = if state.focus == DiffViewerFocus::Patch {
        theme.warning.to_color()
    } else {
        theme.primary.to_color()
    };
    let effective_layout = effective_layout(state);
    let ReviewLineMarkers {
        cursor: cursor_loc,
        commented,
        draft,
        blocker,
        selection,
        matched,
    } = review_cursor_info(state);

    // Keep the comment cursor visible by nudging the patch scroll. Unified only:
    // it's the layout with a per-line cursor and a stable row index.
    if state.cursor_sync_to_view
        && matches!(effective_layout, DiffViewerLayout::Unified)
        && cursor_loc.is_some()
    {
        let viewport = area.height.saturating_sub(2) as usize;
        let synced = state.files.get(state.selected_file).map(|file| {
            let rows = cached_patch_rows(PatchRowsRequest {
                file,
                width: area.width.saturating_sub(2),
                theme,
                layout: DiffViewerLayout::Unified,
                include_prologue: true,
                new_file_presentation: is_new_diff_file(file),
                commented: &commented,
                draft: &draft,
                blocker: &blocker,
                selection: &selection,
                matched: &matched,
            });
            let cursor_row = cursor_loc
                .and_then(|loc| rows.rows.get(&loc))
                .map(|span| span.start);
            (rows.lines.len(), cursor_row)
        });
        if let Some((total_lines, Some(row))) = synced {
            if row < state.patch_scroll {
                state.patch_scroll = row;
            } else if viewport > 0 && row >= state.patch_scroll + viewport {
                state.patch_scroll = row + 1 - viewport;
            }
            let max = total_lines.saturating_sub(viewport.max(1));
            state.patch_scroll = state.patch_scroll.min(max);
        }
        state.cursor_sync_to_view = false;
    }

    let file = state.files.get(state.selected_file);
    let title = file
        .map(|file| {
            if is_new_diff_file(file) {
                format!("New File: {}", file.path)
            } else {
                format!("Patch: {}", file.path)
            }
        })
        .unwrap_or_else(|| "Patch".to_string());

    draw_patch_panel(
        frame,
        area,
        file,
        PatchPanelOptions {
            layout: effective_layout,
            title,
            border_color: border,
            scroll: state.patch_scroll,
            include_prologue: true,
            new_file_presentation: file.map(is_new_diff_file).unwrap_or(false),
            cursor: cursor_loc,
            commented,
            draft,
            blocker,
            selection,
            matched,
        },
        theme,
    );
}

/// The diff-line markers a review viewer needs for the current file: the
/// cursored location, every location covered by a stored comment (so a
/// multi-line comment marks its whole span), and the locations in the
/// in-progress selection (anchor..cursor). All empty outside review mode or
/// when no cursor is active.
/// The patch panel's rows for one file, memoised across frames.
///
/// Building them highlights, word-diffs, and wraps the whole file, which is
/// far too slow to repeat on every frame while `j`/`k` is held. The review
/// cursor is deliberately not part of the cache key: it is painted over the
/// cached rows by [`paint_patch_cursor`], so moving it never forces a rebuild.
pub(crate) struct PatchPanelOptions {
    pub layout: DiffViewerLayout,
    pub title: String,
    pub border_color: Color,
    pub scroll: usize,
    pub include_prologue: bool,
    pub new_file_presentation: bool,
    /// Review line cursor location (unified view only); highlighted when set.
    pub cursor: Option<DiffLineLocation>,
    /// Diff-line locations that carry a kept (non-draft) review comment (unified
    /// view only).
    pub commented: std::collections::HashSet<DiffLineLocation>,
    /// Diff-line locations covered by an unaccepted AI draft comment (unified
    /// view only); marked distinctly from `commented`.
    pub draft: std::collections::HashSet<DiffLineLocation>,
    /// Subset of `commented` covered by a `Blocker`-severity comment (unified
    /// view only); its gutter marker reads in the danger colour.
    pub blocker: std::collections::HashSet<DiffLineLocation>,
    /// Diff-line locations in the in-progress multi-line selection (unified view
    /// only); the gutter is tinted across the span while the reviewer extends it.
    pub selection: std::collections::HashSet<DiffLineLocation>,
    /// Diff-line locations matching the active diff search (unified view only);
    /// each hit gets a gutter marker so matches are visible while scrolling.
    pub matched: std::collections::HashSet<DiffLineLocation>,
}

impl Default for PatchPanelOptions {
    fn default() -> Self {
        Self {
            layout: DiffViewerLayout::Unified,
            title: String::new(),
            border_color: Color::Reset,
            scroll: 0,
            include_prologue: true,
            new_file_presentation: false,
            cursor: None,
            commented: std::collections::HashSet::new(),
            draft: std::collections::HashSet::new(),
            blocker: std::collections::HashSet::new(),
            selection: std::collections::HashSet::new(),
            matched: std::collections::HashSet::new(),
        }
    }
}

pub(crate) fn draw_patch_panel(
    frame: &mut Frame,
    area: Rect,
    file: Option<&DiffFile>,
    options: PatchPanelOptions,
    theme: &Theme,
) {
    let layout_label = match options.layout {
        DiffViewerLayout::Unified => "unified",
        DiffViewerLayout::SideBySide => "side-by-side",
    };
    let title = if options.new_file_presentation {
        Span::styled(
            format!(" {} [{layout_label}] ", options.title),
            Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw(format!(" {} [{layout_label}] ", options.title))
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(options.border_color));

    let Some(file) = file else {
        frame.render_widget(Paragraph::new("No file selected").block(block), area);
        return;
    };
    let rows = cached_patch_rows(PatchRowsRequest {
        file,
        width: area.width.saturating_sub(2),
        theme,
        layout: options.layout.clone(),
        include_prologue: options.include_prologue,
        new_file_presentation: options.new_file_presentation,
        commented: &options.commented,
        draft: &options.draft,
        blocker: &options.blocker,
        selection: &options.selection,
        matched: &options.matched,
    });

    // Only the visible rows go to the widget: a `Paragraph` scrolled over the
    // whole file re-walks every row above the viewport on each frame.
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let start = options.scroll.min(rows.lines.len());
    let end = (start + inner.height as usize).min(rows.lines.len());
    let visible = Paragraph::new(rows.lines[start..end].to_vec());
    let visible = if matches!(options.layout, DiffViewerLayout::SideBySide) {
        visible
    } else {
        visible.wrap(Wrap { trim: false })
    };
    frame.render_widget(visible, inner);

    if let Some(cursor) = options.cursor {
        paint_patch_cursor(
            frame.buffer_mut(),
            inner,
            options.scroll,
            &rows,
            PatchCursor {
                location: cursor,
                has_comment: options.commented.contains(&cursor),
                draft: options.draft.contains(&cursor),
            },
            theme,
        );
    }
}

struct PatchRows {
    lines: Vec<Line<'static>>,
    /// Rows each addressable diff line occupies (unified layout only).
    rows: std::collections::HashMap<DiffLineLocation, std::ops::Range<usize>>,
    /// Width of one line-number column in the unified gutter.
    number_width: usize,
}

struct PatchRowsRequest<'a> {
    file: &'a DiffFile,
    width: u16,
    theme: &'a Theme,
    layout: DiffViewerLayout,
    include_prologue: bool,
    new_file_presentation: bool,
    commented: &'a std::collections::HashSet<DiffLineLocation>,
    draft: &'a std::collections::HashSet<DiffLineLocation>,
    blocker: &'a std::collections::HashSet<DiffLineLocation>,
    selection: &'a std::collections::HashSet<DiffLineLocation>,
    matched: &'a std::collections::HashSet<DiffLineLocation>,
}

impl PatchRowsRequest<'_> {
    /// Everything the rows are derived from. Hashing the hunks rather than
    /// `file.patch` matters: context expansion rewrites the hunks without
    /// touching the patch text.
    ///
    /// This re-hashes the file's text every frame — about 0.15ms for a 550KB
    /// diff, against a rebuild of several milliseconds. A revision stamped on
    /// `DiffFile` would be cheaper, but every path that rewrites hunks would
    /// have to remember to bump it, and one that forgot would draw stale rows.
    fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};

        fn hash_locations(
            set: &std::collections::HashSet<DiffLineLocation>,
            hasher: &mut impl Hasher,
        ) {
            // Order-independent, since set iteration order is not stable.
            let sum = set.iter().fold(0u64, |acc, loc| {
                let mut item = std::collections::hash_map::DefaultHasher::new();
                loc.hash(&mut item);
                acc.wrapping_add(item.finish())
            });
            set.len().hash(hasher);
            sum.hash(hasher);
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        highlight::cache_generation().hash(&mut hasher);
        // The same sides and paths `file_highlights` asks the service for.
        if let Some(source) = self.file.old_content.as_deref() {
            highlight::parser_state_for(self.file.old_path.as_deref().map(Path::new), source)
                .hash(&mut hasher);
        }
        if let Some(source) = self.file.new_content.as_deref() {
            highlight::parser_state_for(Some(Path::new(&self.file.path)), source).hash(&mut hasher);
        }
        format!("{:?}", self.theme).hash(&mut hasher);
        self.width.hash(&mut hasher);
        std::mem::discriminant(&self.layout).hash(&mut hasher);
        self.include_prologue.hash(&mut hasher);
        self.new_file_presentation.hash(&mut hasher);

        let file = self.file;
        file.path.hash(&mut hasher);
        file.old_path.hash(&mut hasher);
        std::mem::discriminant(&file.status).hash(&mut hasher);
        file.is_binary.hash(&mut hasher);
        file.patch.hash(&mut hasher);
        file.old_content.hash(&mut hasher);
        file.new_content.hash(&mut hasher);
        for hunk in &file.hunks {
            hunk.header.hash(&mut hasher);
            (
                hunk.old_start,
                hunk.old_lines,
                hunk.new_start,
                hunk.new_lines,
            )
                .hash(&mut hasher);
            for line in &hunk.lines {
                std::mem::discriminant(&line.kind).hash(&mut hasher);
                line.text.hash(&mut hasher);
            }
        }

        for set in [
            self.commented,
            self.draft,
            self.blocker,
            self.selection,
            self.matched,
        ] {
            hash_locations(set, &mut hasher);
        }
        hasher.finish()
    }

    fn build(&self) -> PatchRows {
        let mut rows = std::collections::HashMap::new();
        let lines = match self.layout {
            DiffViewerLayout::SideBySide => {
                side_by_side_lines(self.file, self.width, self.theme, self.include_prologue)
            }
            DiffViewerLayout::Unified => indexed_patch_lines(
                self.file,
                self.width,
                self.theme,
                self.include_prologue,
                self.new_file_presentation,
                None,
                self.commented,
                self.draft,
                self.blocker,
                self.selection,
                self.matched,
                &mut rows,
            ),
        };
        PatchRows {
            lines,
            rows,
            number_width: line_number_width(self.file),
        }
    }
}

thread_local! {
    /// One entry is enough: a frame draws at most one patch panel, and the
    /// review viewer's cursor-scroll sync asks for the same rows it renders.
    static PATCH_ROWS_CACHE: std::cell::RefCell<Option<(u64, std::rc::Rc<PatchRows>)>> =
        const { std::cell::RefCell::new(None) };
}

fn cached_patch_rows(request: PatchRowsRequest<'_>) -> std::rc::Rc<PatchRows> {
    let key = request.key();
    PATCH_ROWS_CACHE.with(|cache| {
        if let Some((cached_key, rows)) = cache.borrow().as_ref()
            && *cached_key == key
        {
            return rows.clone();
        }
        let rows = std::rc::Rc::new(request.build());
        *cache.borrow_mut() = Some((key, rows.clone()));
        rows
    })
}

struct PatchCursor {
    location: DiffLineLocation,
    has_comment: bool,
    draft: bool,
}

/// Paints the review cursor onto rows rendered without it, reproducing what
/// `wrap_gutter_line` draws for a cursored line: a selection-tinted gutter,
/// high-contrast line numbers, the cursor marker, and bold content.
fn paint_patch_cursor(
    buf: &mut ratatui::buffer::Buffer,
    inner: Rect,
    scroll: usize,
    rows: &PatchRows,
    cursor: PatchCursor,
    theme: &Theme,
) {
    let Some(span) = rows.rows.get(&cursor.location) else {
        return;
    };
    let number_width = rows.number_width;
    let marker_col = number_width * 2 + 2;
    let gutter_width = marker_col + 2;
    let marker = if cursor.has_comment {
        "◆"
    } else if cursor.draft {
        "◈"
    } else {
        "▶"
    };
    let selection_bg = theme.selection.to_color();
    let text_fg = theme.text.to_color();
    let warning_fg = theme.warning.to_color();

    for row in span.clone() {
        let Some(offset) = row.checked_sub(scroll) else {
            continue;
        };
        if offset >= inner.height as usize {
            break;
        }
        let y = inner.y + offset as u16;
        let first = row == span.start;
        let line_width = rows.lines[row].width().min(inner.width as usize);
        for dx in 0..line_width {
            let cell = &mut buf[(inner.x + dx as u16, y)];
            if dx >= gutter_width {
                cell.set_style(Style::default().add_modifier(Modifier::BOLD));
                continue;
            }
            cell.set_bg(selection_bg);
            let is_number =
                dx < number_width || (number_width + 1..number_width * 2 + 1).contains(&dx);
            if is_number {
                cell.set_fg(text_fg);
            } else if dx >= marker_col {
                if first {
                    cell.set_fg(warning_fg);
                    if dx == marker_col {
                        cell.set_symbol(marker);
                    }
                } else {
                    cell.set_fg(text_fg);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn patch_lines(
    file: &DiffFile,
    width: u16,
    theme: &Theme,
    include_prologue: bool,
    new_file_presentation: bool,
    cursor: Option<DiffLineLocation>,
    commented: &std::collections::HashSet<DiffLineLocation>,
    draft: &std::collections::HashSet<DiffLineLocation>,
    blocker: &std::collections::HashSet<DiffLineLocation>,
    selection: &std::collections::HashSet<DiffLineLocation>,
    matched: &std::collections::HashSet<DiffLineLocation>,
    cursor_row: &mut Option<usize>,
) -> Vec<Line<'static>> {
    let mut rows = std::collections::HashMap::new();
    let lines = indexed_patch_lines(
        file,
        width,
        theme,
        include_prologue,
        new_file_presentation,
        cursor,
        commented,
        draft,
        blocker,
        selection,
        matched,
        &mut rows,
    );
    *cursor_row = cursor.and_then(|loc| rows.get(&loc)).map(|span| span.start);
    lines
}

/// [`patch_lines`], also recording which rows each addressable diff line
/// occupies, so a cached render can place the review cursor without a rebuild.
#[allow(clippy::too_many_arguments)]
fn indexed_patch_lines(
    file: &DiffFile,
    width: u16,
    theme: &Theme,
    include_prologue: bool,
    new_file_presentation: bool,
    cursor: Option<DiffLineLocation>,
    commented: &std::collections::HashSet<DiffLineLocation>,
    draft: &std::collections::HashSet<DiffLineLocation>,
    blocker: &std::collections::HashSet<DiffLineLocation>,
    selection: &std::collections::HashSet<DiffLineLocation>,
    matched: &std::collections::HashSet<DiffLineLocation>,
    rows: &mut std::collections::HashMap<DiffLineLocation, std::ops::Range<usize>>,
) -> Vec<Line<'static>> {
    let content_width = width as usize;
    if file.is_binary || file.hunks.is_empty() || content_width < 16 {
        return raw_patch_wrapped_lines(file, content_width, theme);
    }

    let number_width = line_number_width(file);
    let gutter_width = number_width * 2 + 4;
    if content_width <= gutter_width + 4 {
        return raw_patch_wrapped_lines(file, content_width, theme);
    }
    let text_width = content_width - gutter_width;
    let highlights = file_highlights(file);
    let added_style = if new_file_presentation {
        new_file_added_row_style(theme)
    } else {
        added_row_style(theme)
    };
    let hunk_style = if new_file_presentation {
        new_file_hunk_header_style(theme)
    } else {
        hunk_header_style(theme)
    };

    let annotation = |loc: DiffLineLocation| GutterAnnotation {
        cursor: cursor == Some(loc),
        has_comment: commented.contains(&loc),
        draft: draft.contains(&loc),
        is_blocker: blocker.contains(&loc),
        selected: selection.contains(&loc),
        search_match: matched.contains(&loc),
    };

    let mut lines = Vec::new();
    if include_prologue {
        for meta in patch_prologue(file) {
            lines.extend(wrap_gutter_line(
                None,
                None,
                plain_chunks(meta, meta_style(meta, theme)),
                meta_style(meta, theme),
                number_width,
                text_width,
                GutterAnnotation::default(),
                theme,
            ));
        }
    }

    for (idx, hunk) in file.hunks.iter().enumerate() {
        if idx > 0 {
            lines.push(Line::from(""));
        }
        lines.extend(wrap_gutter_line(
            None,
            None,
            plain_chunks(&format_hunk_header(hunk), hunk_style),
            hunk_style,
            number_width,
            text_width,
            GutterAnnotation::default(),
            theme,
        ));

        let mut old_line = hunk.old_start;
        let mut new_line = hunk.new_start;
        // Which tokens changed within each paired removed/added line.
        let intra = hunk_intra_line_ranges(hunk);
        for (line_idx, diff_line) in hunk.lines.iter().enumerate() {
            match diff_line.kind {
                DiffLineKind::Context => {
                    let loc = DiffLineLocation {
                        old_line: Some(old_line),
                        new_line: Some(new_line),
                    };
                    let ann = annotation(loc);
                    let start = lines.len();
                    lines.extend(wrap_gutter_line(
                        Some(old_line),
                        Some(new_line),
                        diff_chunks(
                            &diff_line.text,
                            context_row_style(theme),
                            theme,
                            highlighted_line(highlights.new.as_ref(), new_line)
                                .or_else(|| highlighted_line(highlights.old.as_ref(), old_line)),
                        ),
                        context_row_style(theme),
                        number_width,
                        text_width,
                        ann,
                        theme,
                    ));
                    rows.insert(loc, start..lines.len());
                    old_line += 1;
                    new_line += 1;
                }
                DiffLineKind::Removed => {
                    let loc = DiffLineLocation {
                        old_line: Some(old_line),
                        new_line: None,
                    };
                    let ann = annotation(loc);
                    let start = lines.len();
                    lines.extend(wrap_gutter_line(
                        Some(old_line),
                        None,
                        diff_chunks_emphasized(
                            &diff_line.text,
                            removed_row_style(theme),
                            theme,
                            highlighted_line(highlights.old.as_ref(), old_line),
                            intra[line_idx]
                                .clone()
                                .map(|ranges| IntraLineEmphasis {
                                    ranges,
                                    style: removed_emphasis_style(theme),
                                })
                                .as_ref(),
                        ),
                        removed_row_style(theme),
                        number_width,
                        text_width,
                        ann,
                        theme,
                    ));
                    rows.insert(loc, start..lines.len());
                    old_line += 1;
                }
                DiffLineKind::Added => {
                    let loc = DiffLineLocation {
                        old_line: None,
                        new_line: Some(new_line),
                    };
                    let ann = annotation(loc);
                    let start = lines.len();
                    lines.extend(wrap_gutter_line(
                        None,
                        Some(new_line),
                        diff_chunks_emphasized(
                            &diff_line.text,
                            added_style,
                            theme,
                            highlighted_line(highlights.new.as_ref(), new_line),
                            // A brand-new file has no counterpart lines, so its
                            // rows never carry emphasis.
                            intra[line_idx]
                                .clone()
                                .map(|ranges| IntraLineEmphasis {
                                    ranges,
                                    style: added_emphasis_style(theme),
                                })
                                .as_ref(),
                        ),
                        added_style,
                        number_width,
                        text_width,
                        ann,
                        theme,
                    ));
                    rows.insert(loc, start..lines.len());
                    new_line += 1;
                }
                DiffLineKind::NoNewlineMarker => {
                    lines.extend(wrap_gutter_line(
                        None,
                        None,
                        plain_chunks(&diff_line.text, meta_subtle_style(theme)),
                        meta_subtle_style(theme),
                        number_width,
                        text_width,
                        GutterAnnotation::default(),
                        theme,
                    ));
                }
            }
        }
    }

    lines
}

fn side_by_side_lines(
    file: &DiffFile,
    width: u16,
    theme: &Theme,
    include_prologue: bool,
) -> Vec<Line<'static>> {
    if file.is_binary || file.hunks.is_empty() || width < 24 {
        return patch_lines(
            file,
            width,
            theme,
            include_prologue,
            false,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
    }

    let inner_width = width as usize;
    let separator = " | ";
    let column_width = inner_width.saturating_sub(separator.len()) / 2;
    let number_width = line_number_width(file);
    let cell_prefix_width = number_width + 2;
    if column_width <= cell_prefix_width + 6 {
        return patch_lines(
            file,
            width,
            theme,
            include_prologue,
            false,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
    }
    let cell_text_width = column_width - cell_prefix_width;
    let highlights = file_highlights(file);

    let mut lines = vec![Line::from(vec![
        Span::styled(
            pad_cell("BASE", column_width),
            removed_row_style(theme).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            separator.to_string(),
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            pad_cell("CURRENT", column_width),
            added_row_style(theme).add_modifier(Modifier::BOLD),
        ),
    ])];

    if include_prologue {
        for meta in patch_prologue(file) {
            for chunk in wrap_text_to_width(meta, inner_width) {
                lines.push(Line::from(Span::styled(chunk, meta_style(meta, theme))));
            }
        }
    }

    for (idx, hunk) in file.hunks.iter().enumerate() {
        if idx > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            format_hunk_header(hunk),
            hunk_header_style(theme),
        )));

        let mut index = 0usize;
        let mut old_line = hunk.old_start;
        let mut new_line = hunk.new_start;
        while index < hunk.lines.len() {
            match hunk.lines[index].kind {
                DiffLineKind::Context => {
                    let text = trim_diff_prefix(&hunk.lines[index]).to_string();
                    lines.extend(side_by_side_rows(
                        Some(old_line),
                        Some(new_line),
                        format!(" {text}"),
                        format!(" {text}"),
                        highlighted_line(highlights.old.as_ref(), old_line),
                        highlighted_line(highlights.new.as_ref(), new_line),
                        context_row_style(theme),
                        context_row_style(theme),
                        number_width,
                        cell_text_width,
                        separator,
                        theme,
                    ));
                    index += 1;
                    old_line += 1;
                    new_line += 1;
                }
                DiffLineKind::Removed => {
                    let removed = collect_run(&hunk.lines, &mut index, DiffLineKind::Removed);
                    let added = collect_run(&hunk.lines, &mut index, DiffLineKind::Added);
                    let row_count = removed.len().max(added.len());
                    for row in 0..row_count {
                        let left = removed
                            .get(row)
                            .map(|line| format!("-{}", trim_diff_prefix(line)))
                            .unwrap_or_default();
                        let right = added
                            .get(row)
                            .map(|line| format!("+{}", trim_diff_prefix(line)))
                            .unwrap_or_default();
                        let left_number = removed.get(row).map(|_| old_line + row);
                        let right_number = added.get(row).map(|_| new_line + row);
                        lines.extend(side_by_side_rows(
                            left_number,
                            right_number,
                            left,
                            right,
                            left_number
                                .and_then(|line| highlighted_line(highlights.old.as_ref(), line)),
                            right_number
                                .and_then(|line| highlighted_line(highlights.new.as_ref(), line)),
                            removed_row_style(theme),
                            added_row_style(theme),
                            number_width,
                            cell_text_width,
                            separator,
                            theme,
                        ));
                    }
                    old_line += removed.len();
                    new_line += added.len();
                }
                DiffLineKind::Added => {
                    let added = collect_run(&hunk.lines, &mut index, DiffLineKind::Added);
                    for (row, line) in added.iter().enumerate() {
                        lines.extend(side_by_side_rows(
                            None,
                            Some(new_line + row),
                            String::new(),
                            format!("+{}", trim_diff_prefix(line)),
                            None,
                            highlighted_line(highlights.new.as_ref(), new_line + row),
                            neutral_side_style(theme),
                            added_row_style(theme),
                            number_width,
                            cell_text_width,
                            separator,
                            theme,
                        ));
                    }
                    new_line += added.len();
                }
                DiffLineKind::NoNewlineMarker => {
                    lines.push(Line::from(Span::styled(
                        hunk.lines[index].text.clone(),
                        meta_subtle_style(theme),
                    )));
                    index += 1;
                }
            }
        }
    }

    lines
}

fn format_hunk_header(hunk: &crate::diff::DiffHunk) -> String {
    format!(
        " Change: base {} -> current {} ",
        format_hunk_range(hunk.old_start, hunk.old_lines),
        format_hunk_range(hunk.new_start, hunk.new_lines)
    )
}

fn format_hunk_range(start: usize, len: usize) -> String {
    match len {
        0 => format!("{start}"),
        1 => format!("{start}"),
        _ => format!("{start}-{}", start + len - 1),
    }
}

fn effective_layout(state: &DiffViewerState) -> DiffViewerLayout {
    state
        .files
        .get(state.selected_file)
        .filter(|file| is_new_diff_file(file))
        .map(|_| DiffViewerLayout::Unified)
        .unwrap_or_else(|| state.layout.clone())
}

fn is_new_diff_file(file: &DiffFile) -> bool {
    matches!(
        file.status,
        DiffFileStatus::Added | DiffFileStatus::Untracked
    )
}

fn status_label(status: &DiffFileStatus) -> &'static str {
    match status {
        DiffFileStatus::Added => "A",
        DiffFileStatus::Modified => "M",
        DiffFileStatus::Deleted => "D",
        DiffFileStatus::Renamed => "R",
        DiffFileStatus::Copied => "C",
        DiffFileStatus::TypeChanged => "T",
        DiffFileStatus::Untracked => "U",
    }
}

fn status_color(status: &DiffFileStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        DiffFileStatus::Added | DiffFileStatus::Untracked => theme.success.to_color(),
        DiffFileStatus::Modified | DiffFileStatus::Renamed | DiffFileStatus::Copied => {
            theme.warning.to_color()
        }
        DiffFileStatus::Deleted => theme.danger.to_color(),
        DiffFileStatus::TypeChanged => theme.info.to_color(),
    }
}

fn collect_run(lines: &[DiffLine], index: &mut usize, kind: DiffLineKind) -> Vec<DiffLine> {
    let mut run = Vec::new();
    while *index < lines.len() && lines[*index].kind == kind {
        run.push(lines[*index].clone());
        *index += 1;
    }
    run
}

fn trim_diff_prefix(line: &DiffLine) -> &str {
    line.text
        .strip_prefix(['+', '-', ' '])
        .unwrap_or(line.text.as_str())
}

// Layout inputs for one paired diff row; they vary per call site, so a
// struct would be built and torn down for every row.
#[allow(clippy::too_many_arguments)]
fn side_by_side_rows(
    left_number: Option<usize>,
    right_number: Option<usize>,
    left: String,
    right: String,
    left_highlight: Option<&highlight::HighlightedLine>,
    right_highlight: Option<&highlight::HighlightedLine>,
    left_style: Style,
    right_style: Style,
    number_width: usize,
    text_width: usize,
    separator: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let base_bg = popup_base_bg(theme);
    let paired_change_row = left_number.is_some()
        && right_number.is_some()
        && left_style.bg != Some(base_bg)
        && right_style.bg != Some(base_bg);
    // A row showing a removal beside its replacement is exactly the pair a
    // word-level diff describes, so derive the emphasis here rather than
    // threading it through yet another parameter.
    let intra = if paired_change_row {
        crate::worddiff::word_diff_cached(line_content(&left), line_content(&right))
    } else {
        None
    };
    let left_wrapped = if left_number.is_none() && left.is_empty() {
        vec![plain_chunks(
            &hatch_fill(text_width, 0),
            hatched_side_style(right_style, theme),
        )]
    } else {
        wrap_chunks(
            &diff_chunks_emphasized(
                &left,
                left_style,
                theme,
                left_highlight,
                intra
                    .as_ref()
                    .map(|diff| IntraLineEmphasis {
                        ranges: diff.old.clone(),
                        style: removed_emphasis_style(theme),
                    })
                    .as_ref(),
            ),
            text_width,
            left_style,
        )
    };
    let right_wrapped = if right_number.is_none() && right.is_empty() {
        vec![plain_chunks(
            &hatch_fill(text_width, 0),
            hatched_side_style(left_style, theme),
        )]
    } else {
        wrap_chunks(
            &diff_chunks_emphasized(
                &right,
                right_style,
                theme,
                right_highlight,
                intra
                    .as_ref()
                    .map(|diff| IntraLineEmphasis {
                        ranges: diff.new.clone(),
                        style: added_emphasis_style(theme),
                    })
                    .as_ref(),
            ),
            text_width,
            right_style,
        )
    };
    let row_count = left_wrapped.len().max(right_wrapped.len());
    let mut rows = Vec::with_capacity(row_count);
    let left_missing = left_number.is_none() && left.is_empty();
    let right_missing = right_number.is_none() && right.is_empty();

    for row in 0..row_count {
        let left_has_content = left_wrapped
            .get(row)
            .map(|chunks| !chunks.is_empty())
            .unwrap_or(false);
        let right_has_content = right_wrapped
            .get(row)
            .map(|chunks| !chunks.is_empty())
            .unwrap_or(false);
        let left_prefix = if row == 0 {
            line_number_label(left_number, number_width)
        } else {
            blank_line_number_label(number_width)
        };
        let right_prefix = if row == 0 {
            line_number_label(right_number, number_width)
        } else {
            blank_line_number_label(number_width)
        };
        let left_cell_style = if left_missing {
            hatched_side_style(right_style, theme)
        } else if left_has_content {
            left_style
        } else {
            context_row_style(theme)
        };
        let right_cell_style = if right_missing {
            hatched_side_style(left_style, theme)
        } else if right_has_content {
            right_style
        } else {
            context_row_style(theme)
        };
        let left_bg = left_cell_style.bg.unwrap_or_else(|| popup_base_bg(theme));
        let right_bg = right_cell_style.bg.unwrap_or_else(|| popup_base_bg(theme));
        let left_cell = if left_missing {
            pad_chunks_to_width(
                plain_chunks(&hatch_fill(text_width, row), left_cell_style),
                text_width,
                left_cell_style,
            )
        } else {
            pad_chunks_to_width(
                left_wrapped.get(row).cloned().unwrap_or_default(),
                text_width,
                if paired_change_row {
                    Style::default().bg(base_bg)
                } else {
                    left_style
                },
            )
        };
        let right_cell = if right_missing {
            pad_chunks_to_width(
                plain_chunks(&hatch_fill(text_width, row), right_cell_style),
                text_width,
                right_cell_style,
            )
        } else {
            pad_chunks_to_width(
                right_wrapped.get(row).cloned().unwrap_or_default(),
                text_width,
                if paired_change_row {
                    Style::default().bg(base_bg)
                } else {
                    right_style
                },
            )
        };
        let mut line = vec![Span::styled(left_prefix, left_cell_style)];
        line.push(Span::styled(
            "│ ",
            Style::default()
                .fg(line_number_fg(left_cell_style, left_bg))
                .bg(left_bg),
        ));
        line.extend(chunks_to_spans(left_cell));
        line.push(Span::styled(
            separator.to_string(),
            Style::default().fg(theme.text_muted.to_color()).bg(
                if paired_change_row && (!left_has_content || !right_has_content) {
                    base_bg
                } else {
                    blend_color(left_bg, right_bg, 0.5)
                },
            ),
        ));
        line.push(Span::styled(right_prefix, right_cell_style));
        line.push(Span::styled(
            "│ ",
            Style::default()
                .fg(line_number_fg(right_cell_style, right_bg))
                .bg(right_bg),
        ));
        line.extend(chunks_to_spans(right_cell));
        rows.push(Line::from(line));
    }

    rows
}

fn raw_patch_wrapped_lines(file: &DiffFile, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for line in file.patch.lines() {
        for chunk in wrap_text_to_width(line, width) {
            lines.push(Line::from(Span::styled(chunk, meta_style(line, theme))));
        }
    }
    lines
}

/// Per-row marker state for the review line cursor. `cursor` is the line the
/// reviewer is positioned on; `has_comment` is a line that already carries a
/// comment; `selected` is a line inside the in-progress multi-line selection.
/// Default (all false) renders the ordinary `│ ` gutter.
#[derive(Clone, Copy, Default)]
struct GutterAnnotation {
    cursor: bool,
    has_comment: bool,
    /// Line carries an unaccepted AI draft comment (rendered as a hollow marker
    /// distinct from a kept comment's filled one).
    draft: bool,
    /// The kept comment on this line is `Blocker`-severity, so its marker reads
    /// in the higher-contrast danger colour.
    is_blocker: bool,
    selected: bool,
    /// The line matches the active diff search. Lowest-priority marker: shown
    /// only when the line isn't the cursor / commented / a draft (the current
    /// match is already the cursor's solid marker).
    search_match: bool,
}

#[allow(clippy::too_many_arguments)]
fn wrap_gutter_line(
    old_number: Option<usize>,
    new_number: Option<usize>,
    chunks: Vec<StyledChunk>,
    style: Style,
    number_width: usize,
    text_width: usize,
    annotation: GutterAnnotation,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let wrapped = wrap_chunks(&chunks, text_width, style);
    let mut lines = Vec::with_capacity(wrapped.len());
    let row_bg = style.bg.unwrap_or(Color::Black);
    // The cursor row and every line in the active selection get a
    // selection-tinted gutter; the cursor row also gets a high-contrast number
    // colour so it reads clearly against any diff-row background.
    let gutter_bg = if annotation.cursor || annotation.selected {
        theme.selection.to_color()
    } else {
        row_bg
    };
    let number_fg = if annotation.cursor {
        theme.text.to_color()
    } else {
        line_number_fg(style, row_bg)
    };
    // A kept comment takes priority over a draft marker when both somehow land on
    // a line. Drafts read as a hollow circle in the warning colour ("pending
    // adjudication") vs a kept comment's filled info-coloured dot.
    let (marker, marker_fg) = if annotation.cursor {
        if annotation.has_comment {
            ("◆ ", theme.warning.to_color())
        } else if annotation.draft {
            ("◈ ", theme.warning.to_color())
        } else {
            ("▶ ", theme.warning.to_color())
        }
    } else if annotation.has_comment {
        // A blocker's dot reads in danger so must-fix lines stand out.
        let color = if annotation.is_blocker {
            theme.danger.to_color()
        } else {
            theme.info.to_color()
        };
        ("● ", color)
    } else if annotation.draft {
        ("○ ", theme.warning.to_color())
    } else if annotation.search_match {
        // A hollow triangle echoes the cursor's solid ▶ so a match reads as
        // "another place to jump", in the primary colour to stay clear of the
        // comment / draft marker hues.
        ("▷ ", theme.primary.to_color())
    } else {
        ("│ ", line_number_fg(style, row_bg))
    };
    for (index, chunk_line) in wrapped.into_iter().enumerate() {
        let old_label = if index == 0 {
            line_number_label(old_number, number_width)
        } else {
            blank_line_number_label(number_width)
        };
        let new_label = if index == 0 {
            line_number_label(new_number, number_width)
        } else {
            blank_line_number_label(number_width)
        };
        // The marker glyph only shows on the first visual line of a logical
        // row; continuation lines keep the plain separator (tinted on cursor).
        let (sep_text, sep_fg) = if index == 0 {
            (marker, marker_fg)
        } else {
            ("│ ", number_fg)
        };
        let mut line = vec![
            Span::styled(old_label, Style::default().fg(number_fg).bg(gutter_bg)),
            Span::styled(" ", Style::default().bg(gutter_bg)),
            Span::styled(new_label, Style::default().fg(number_fg).bg(gutter_bg)),
            Span::styled(" ", Style::default().bg(gutter_bg)),
            Span::styled(
                sep_text.to_string(),
                Style::default().fg(sep_fg).bg(gutter_bg),
            ),
        ];
        if annotation.cursor {
            // Bold the content of the cursor row for extra emphasis without
            // disturbing syntax-highlight colours.
            line.extend(chunks_to_spans(chunk_line).into_iter().map(|span| {
                let style = span.style.add_modifier(Modifier::BOLD);
                Span::styled(span.content, style)
            }));
        } else {
            line.extend(chunks_to_spans(chunk_line));
        }
        lines.push(Line::from(line));
    }
    lines
}

fn line_number_width(file: &DiffFile) -> usize {
    let mut max_line = 1usize;
    for hunk in &file.hunks {
        max_line = max_line.max(hunk.old_start.saturating_add(hunk.old_lines));
        max_line = max_line.max(hunk.new_start.saturating_add(hunk.new_lines));
    }
    max_line.to_string().len().max(1)
}

fn patch_prologue(file: &DiffFile) -> Vec<&str> {
    if is_new_diff_file(file) {
        return vec!["NEW FILE added in this branch"];
    }
    file.patch
        .lines()
        .take_while(|line| !line.starts_with("@@ "))
        .collect()
}

fn meta_style(line: &str, theme: &Theme) -> Style {
    if line.starts_with("NEW FILE ") || line.starts_with("New file ") {
        new_file_hunk_header_style(theme)
    } else if line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("copy from ")
        || line.starts_with("copy to ")
    {
        meta_subtle_style(theme)
    } else if line.starts_with("@@ ") {
        hunk_header_style(theme)
    } else if line.starts_with('+') && !line.starts_with("+++") {
        added_row_style(theme)
    } else if line.starts_with('-') && !line.starts_with("---") {
        removed_row_style(theme)
    } else if line.starts_with("+++ ") || line.starts_with("--- ") {
        meta_subtle_style(theme)
    } else {
        context_row_style(theme)
    }
}

fn line_number_label(number: Option<usize>, width: usize) -> String {
    match number {
        Some(number) => format!("{number:>width$}", width = width),
        None => blank_line_number_label(width),
    }
}

fn blank_line_number_label(width: usize) -> String {
    " ".repeat(width)
}

fn popup_base_bg(theme: &Theme) -> Color {
    theme.background.to_color()
}

fn context_row_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.text.to_color())
        .bg(popup_base_bg(theme))
}

fn neutral_side_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.text_muted.to_color())
        .bg(blend_color(
            popup_base_bg(theme),
            theme.header_background.to_color(),
            0.42,
        ))
}

fn added_row_style(theme: &Theme) -> Style {
    Style::default().fg(theme.text.to_color()).bg(blend_color(
        popup_base_bg(theme),
        theme.success.to_color(),
        0.28,
    ))
}

/// Background for the tokens a word-level diff found changed: the row's own
/// hue, blended harder so the changed part reads as "more added" / "more
/// removed" rather than as a different kind of thing. Only the background is
/// set, so the syntax-highlight foreground survives.
fn added_emphasis_style(theme: &Theme) -> Style {
    Style::default().bg(blend_color(
        popup_base_bg(theme),
        theme.success.to_color(),
        0.62,
    ))
}

fn removed_emphasis_style(theme: &Theme) -> Style {
    Style::default().bg(blend_color(
        popup_base_bg(theme),
        theme.danger.to_color(),
        0.58,
    ))
}

/// Word-level emphasis for every line of a hunk, indexed the same as
/// `hunk.lines`. The pairing rule lives in `worddiff` so the GUI shares it.
fn hunk_intra_line_ranges(hunk: &DiffHunk) -> Vec<Option<Vec<std::ops::Range<usize>>>> {
    crate::worddiff::hunk_intra_line_ranges(&hunk.lines)
}

/// A diff line's text with its leading `+`/`-`/space prefix removed.
fn line_content(text: &str) -> &str {
    crate::worddiff::line_content(text)
}

fn removed_row_style(theme: &Theme) -> Style {
    Style::default().fg(theme.text.to_color()).bg(blend_color(
        popup_base_bg(theme),
        theme.danger.to_color(),
        0.26,
    ))
}

fn hunk_header_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.info.to_color())
        .bg(blend_color(
            popup_base_bg(theme),
            theme.info.to_color(),
            0.12,
        ))
        .add_modifier(Modifier::BOLD)
}

fn new_file_added_row_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.success.to_color())
        .bg(popup_base_bg(theme))
}

fn new_file_hunk_header_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.success.to_color())
        .bg(blend_color(
            popup_base_bg(theme),
            theme.success.to_color(),
            0.18,
        ))
        .add_modifier(Modifier::BOLD)
}

fn meta_subtle_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.text_muted.to_color())
        .bg(blend_color(
            popup_base_bg(theme),
            theme.primary.to_color(),
            0.08,
        ))
}

fn line_number_fg(style: Style, row_bg: Color) -> Color {
    style.fg.unwrap_or(blend_color(row_bg, Color::White, 0.55))
}

fn hatched_side_style(reference: Style, theme: &Theme) -> Style {
    let row_bg = reference.bg.unwrap_or_else(|| popup_base_bg(theme));
    Style::default()
        .fg(blend_color(row_bg, Color::White, 0.42))
        .bg(row_bg)
}

fn hatch_fill(width: usize, row: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let _ = row;
    " ".repeat(width)
}

fn blend_color(base: Color, overlay: Color, alpha: f32) -> Color {
    let alpha = alpha.clamp(0.0, 1.0);
    let (br, bg, bb) = color_to_rgb(base);
    let (or, og, ob) = color_to_rgb(overlay);
    Color::Rgb(
        ((br as f32 * (1.0 - alpha)) + (or as f32 * alpha)).round() as u8,
        ((bg as f32 * (1.0 - alpha)) + (og as f32 * alpha)).round() as u8,
        ((bb as f32 * (1.0 - alpha)) + (ob as f32 * alpha)).round() as u8,
    )
}

fn color_to_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (204, 204, 204),
        Color::DarkGray => (118, 118, 118),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::White => (242, 242, 242),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => (i, i, i),
        Color::Reset => (48, 52, 70),
    }
}

fn pad_cell(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]));
        if used + ch_width > width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }

    if used < width {
        out.push_str(&" ".repeat(width - used));
    }

    out
}

fn wrap_text_to_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut out = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;

    for ch in text.chars() {
        let mut buf = [0; 4];
        let ch_str = ch.encode_utf8(&mut buf);
        let ch_width = UnicodeWidthStr::width(ch_str).max(1);

        if used + ch_width > width && !current.is_empty() {
            out.push(current);
            current = String::new();
            used = 0;
        }

        current.push(ch);
        used += ch_width;

        if used >= width {
            out.push(current);
            current = String::new();
            used = 0;
        }
    }

    if !current.is_empty() {
        out.push(current);
    }

    if out.is_empty() {
        out.push(String::new());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn changed_pair_hunk(old: &str, new: &str) -> DiffHunk {
        DiffHunk {
            header: "@@ -1 +1 @@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: format!("-{old}"),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: format!("+{new}"),
                },
            ],
        }
    }

    #[test]
    fn intra_line_ranges_pair_a_removal_with_its_replacement() {
        let hunk = changed_pair_hunk("let x = foo(1);", "let x = foo(2);");
        let ranges = hunk_intra_line_ranges(&hunk);

        // Index 0 is the removal, index 1 its paired addition.
        let old = ranges[0].as_ref().expect("removal has emphasis");
        let new = ranges[1].as_ref().expect("addition has emphasis");
        assert_eq!(&"let x = foo(1);"[old[0].clone()], "1");
        assert_eq!(&"let x = foo(2);"[new[0].clone()], "2");
    }

    #[test]
    fn intra_line_ranges_skip_unpaired_and_context_lines() {
        // Two removals, one addition: only the first pair has a counterpart.
        let hunk = DiffHunk {
            header: "@@ -1,3 +1,2 @@".into(),
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 2,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " unchanged".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: "-value = 1;".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: "-dropped();".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+value = 2;".into(),
                },
            ],
        };
        let ranges = hunk_intra_line_ranges(&hunk);

        assert!(ranges[0].is_none(), "context lines are never emphasised");
        assert!(ranges[1].is_some(), "first removal pairs with the addition");
        assert!(ranges[2].is_none(), "second removal has no counterpart");
        assert!(ranges[3].is_some());
    }

    #[test]
    fn emphasis_splits_chunks_without_losing_or_reordering_text() {
        let theme = Theme::default();
        let row = added_row_style(&theme);
        let emphasis = IntraLineEmphasis {
            // Two disjoint runs inside "abcdef": "bc" and "e".
            ranges: Vec::from([1..3, 4..5]),
            style: added_emphasis_style(&theme),
        };
        let chunks = diff_chunks_emphasized("+abcdef", row, &theme, None, Some(&emphasis));

        let text: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(text, "+abcdef", "no text may be lost or reordered");

        // Only the emphasised slices carry the brighter background; the rest
        // keeps the plain row background.
        let emphasised: String = chunks
            .iter()
            .filter(|c| c.style.bg == added_emphasis_style(&theme).bg)
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(emphasised, "bce");
    }

    #[test]
    fn emphasis_offsets_are_measured_past_the_diff_prefix() {
        let theme = Theme::default();
        // Offsets are into the *content*, not the raw line — starting a range
        // at 0 must highlight `a`, never the `+` prefix.
        let emphasis = IntraLineEmphasis {
            ranges: Vec::from([0..3, 5..6]),
            style: added_emphasis_style(&theme),
        };
        let chunks = diff_chunks_emphasized(
            "+abcdef",
            added_row_style(&theme),
            &theme,
            None,
            Some(&emphasis),
        );

        let emphasised: String = chunks
            .iter()
            .filter(|c| c.style.bg == added_emphasis_style(&theme).bg)
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(emphasised, "abcf");
    }

    #[test]
    fn the_amf_split_count_reads_cleanly_for_singular_plural_and_none() {
        assert_eq!(
            amf_split_count_label(1, 4),
            "5 files · 1 is yours · 4 generated by AMF"
        );
        assert_eq!(
            amf_split_count_label(3, 1),
            "4 files · 3 are yours · 1 generated by AMF"
        );
        assert_eq!(
            amf_split_count_label(0, 2),
            "2 files · none are yours · 2 generated by AMF"
        );
        assert_eq!(
            amf_split_count_label(0, 1),
            "1 file · none are yours · 1 generated by AMF"
        );
    }

    #[test]
    fn diff_footer_shows_the_ignore_whitespace_state() {
        let theme = Theme::default();
        let shown = diff_footer_lines(
            "files", "unified", false, None, true, None, false, false, 200, &theme,
        );
        assert!(line_text(&shown[1]).contains("ws: shown"));

        let ignored = diff_footer_lines(
            "files", "unified", false, None, true, None, true, false, 200, &theme,
        );
        assert!(line_text(&ignored[1]).contains("ws: ignored"));
    }

    /// The second row used to wrap at 120 columns with every hint present,
    /// pushing `b base ref` onto a third row the two-row footer never draws.
    #[test]
    fn diff_footer_rows_fit_whole_hints_at_80_and_120_columns() {
        use unicode_width::UnicodeWidthStr;
        let theme = Theme::default();
        for width in [80u16, 120] {
            let lines = diff_footer_lines(
                "files",
                "side-by-side",
                false,
                Some((
                    highlight::HighlightLanguage::Tsx,
                    highlight::HighlightInstallState::Available,
                )),
                true,
                Some(crate::diff::DIFF_DEFAULT_CONTEXT + 7),
                true,
                true,
                width,
                &theme,
            );
            for line in &lines {
                let text = line_text(line);
                assert!(
                    UnicodeWidthStr::width(text.as_str()) <= usize::from(width),
                    "{width}: {text:?} overflows"
                );
            }
            let first = line_text(&lines[0]);
            assert!(first.contains(". AMF files"), "{width}: {first}");
            assert!(first.contains("Esc close"), "{width}: {first}");
            let second = line_text(&lines[1]);
            assert!(second.contains("j/k move"), "{width}: {second}");
            assert!(second.contains("r refresh"), "{width}: {second}");
            assert!(second.contains("b base ref"), "{width}: {second}");
            // Whatever is dropped goes whole: no hint is left without its label.
            assert!(!second.trim_end().ends_with('b'), "{width}: {second}");
        }
        let wide = diff_footer_lines(
            "files",
            "unified",
            false,
            None,
            true,
            Some(3),
            false,
            false,
            200,
            &theme,
        );
        assert!(line_text(&wide[1]).contains("g/G top/bottom"));
    }

    #[test]
    fn diff_footer_prioritizes_syntax_install_hint() {
        let theme = Theme::default();
        let lines = diff_footer_lines(
            "files",
            "unified",
            true,
            Some((
                highlight::HighlightLanguage::Tsx,
                highlight::HighlightInstallState::Available,
            )),
            true,
            None,
            false,
            false,
            200,
            &theme,
        );

        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).contains("install tsx parser"));
        assert!(line_text(&lines[0]).contains("Esc close"));
        assert!(line_text(&lines[1]).contains("layout:unified (new file)"));
    }

    #[test]
    fn file_comment_editor_expands_footer_and_renders_edit_box() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.editing_file_comment = true;
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("File Comment — a.rs"));
        assert!(rendered.contains("Write feedback for the agent. Markdown is fine."));
    }

    #[test]
    fn review_footer_reports_session_vim_state() {
        let (mut state, _) = single_added_line_review_state();
        let theme = Theme::default();

        let plain = review_hint_lines(&state, &theme);
        assert!(line_text(&plain[1]).contains("editor vim: off"));

        state.toggle_feedback_vim();
        let vim = review_hint_lines(&state, &theme);
        assert!(line_text(&vim[1]).contains("editor vim: on"));
    }

    #[test]
    fn active_review_editor_reports_normal_mode_and_session_toggle() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.editing_file_comment = true;
        state.toggle_feedback_vim();
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("[Vim Normal]"));
        assert!(rendered.contains("session vim off"));
    }

    #[test]
    fn review_help_documents_session_vim_submit_and_cancel_controls() {
        let comments = REVIEW_HELP_SECTIONS
            .iter()
            .find(|(title, _)| *title == "Comments")
            .expect("Comments help section")
            .1;

        assert!(comments.iter().any(|(key, text)| {
            *key == "Ctrl+T" && text.contains("every editor in this review session")
        }));
        assert!(
            comments
                .iter()
                .any(|(key, text)| *key == "Tab" && text.contains("either keymap"))
        );
        assert!(
            comments
                .iter()
                .any(|(key, text)| *key == "Ctrl+Q" && text.contains("Cancel"))
        );
        assert!(
            comments
                .iter()
                .any(|(_, text)| text.contains("enabling Vim enters Normal mode"))
        );
    }

    #[test]
    fn new_file_patch_lines_preserve_syntax_coloring_for_indented_javascript() {
        if highlight::HighlightLanguage::JavaScript.install_state()
            != highlight::HighlightInstallState::Installed
        {
            return;
        }

        crate::highlight::reload_runtime_state();

        let theme = Theme::default();
        let file = DiffFile {
            old_path: None,
            path: "syntax-test.js".to_string(),
            status: DiffFileStatus::Added,
            additions: 3,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some("const palette = {\n  primary: \"#0f172a\",\n};\n".to_string()),
            patch: "\
diff --git a/syntax-test.js b/syntax-test.js
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/syntax-test.js
@@ -0,0 +1,3 @@
+const palette = {
+  primary: \"#0f172a\",
+};
"
            .to_string(),
            hunks: vec![crate::diff::DiffHunk {
                header: "@@ -0,0 +1,3 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 3,
                lines: vec![
                    crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: "+const palette = {".to_string(),
                    },
                    crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: "+  primary: \"#0f172a\",".to_string(),
                    },
                    crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: "+};".to_string(),
                    },
                ],
            }],
        };

        let lines = patch_lines(
            &file,
            100,
            &theme,
            false,
            true,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
        let indented_code_line = &lines[2];
        let default_added_fg = new_file_added_row_style(&theme).fg;
        let has_syntax_colored_token = indented_code_line.spans.iter().any(|span| {
            !span.content.trim().is_empty()
                && span.content.contains("primary")
                && span.style.fg != default_added_fg
        });

        assert!(
            has_syntax_colored_token,
            "expected indented JavaScript property to keep syntax coloring in new-file diff rows"
        );
    }

    #[test]
    fn new_file_patch_lines_preserve_syntax_coloring_for_typescript() {
        if highlight::HighlightLanguage::TypeScript.install_state()
            != highlight::HighlightInstallState::Installed
        {
            return;
        }

        crate::highlight::reload_runtime_state();

        let theme = Theme::default();
        let file = DiffFile {
            old_path: None,
            path: "docs/syntax-tests/syntax-test-highlight.ts".to_string(),
            status: DiffFileStatus::Added,
            additions: 74,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some(
                include_str!("../../../docs/syntax-tests/syntax-test-highlight.ts").to_string(),
            ),
            patch: "\
diff --git a/docs/syntax-tests/syntax-test-highlight.ts b/docs/syntax-tests/syntax-test-highlight.ts
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/docs/syntax-tests/syntax-test-highlight.ts
@@ -0,0 +1,74 @@
"
            .to_string(),
            hunks: vec![crate::diff::DiffHunk {
                header: "@@ -0,0 +1,74 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 74,
                lines: include_str!("../../../docs/syntax-tests/syntax-test-highlight.ts")
                    .lines()
                    .map(|line| crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: format!("+{line}"),
                    })
                    .collect(),
            }],
        };

        let lines = patch_lines(
            &file,
            120,
            &theme,
            false,
            true,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
        let has_syntax_colored_token =
            lines.iter().flat_map(|line| line.spans.iter()).any(|span| {
                !span.content.trim().is_empty()
                    && span.content.contains("JsonPrimitive")
                    && span.style.fg != new_file_added_row_style(&theme).fg
            });

        assert!(
            has_syntax_colored_token,
            "expected TypeScript diff rows to keep syntax coloring in new-file diff rows"
        );
    }

    #[test]
    fn pr_review_notes_list_comments_on_files_that_left_the_pr() {
        let (mut state, _) = single_added_line_review_state();
        // Nothing changed, nothing outdated, nothing detached: nothing to say.
        assert_eq!(pr_review_notes_markdown(&state, "a.rs"), None);
        state.pr_detached_line_comments.insert(
            "gone.rs".to_string(),
            vec![
                serde_json::from_value(serde_json::json!({
                    "location": {"old_line": null, "new_line": 4},
                    "text": "about removed code"
                }))
                .unwrap(),
            ],
        );
        state.pr_detached_file_comments.insert(
            "gone.rs".to_string(),
            serde_json::from_value(serde_json::json!({"text": "whole-file note"})).unwrap(),
        );

        let notes = pr_review_notes_markdown(&state, "a.rs").unwrap();

        assert!(
            notes.contains("Comments on files no longer in this PR"),
            "{notes}"
        );
        assert!(notes.contains("gone.rs (was L4)"), "{notes}");
        assert!(notes.contains("about removed code"), "{notes}");
        assert!(notes.contains("gone.rs** (whole file)"), "{notes}");
        assert!(!notes.contains("Outdated comments"), "{notes}");
    }

    fn single_added_line_review_state() -> (DiffViewerState, DiffLineLocation) {
        let mut state = DiffViewerState::new(
            crate::app::ViewState::new(
                "proj".into(),
                "feat".into(),
                "sess".into(),
                "claude".into(),
                "Claude".into(),
                crate::project::SessionKind::Claude,
                crate::project::VibeMode::Vibeless,
                true,
            ),
            std::path::PathBuf::from("/tmp"),
        );
        state.review = true;
        let file = DiffFile {
            old_path: None,
            path: "a.rs".to_string(),
            status: DiffFileStatus::Added,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some("x\n".to_string()),
            patch: String::new(),
            hunks: vec![crate::diff::DiffHunk {
                header: "@@ -0,0 +1,1 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 1,
                lines: vec![crate::diff::DiffLine {
                    kind: crate::diff::DiffLineKind::Added,
                    text: "+x".to_string(),
                }],
            }],
        };
        let loc = file.addressable_lines()[0];
        state.files = vec![file];
        state.selected_file = 0;
        (state, loc)
    }

    fn render_viewer(state: &mut DiffViewerState) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(160, 36)).unwrap();
        terminal
            .draw(|frame| draw_diff_viewer(frame, state, &Theme::default()))
            .unwrap();
        normalized_screen(&terminal)
    }

    #[test]
    fn head_fallback_is_labelled_and_the_body_offers_b() {
        for review in [false, true] {
            let (mut state, _) = single_added_line_review_state();
            state.review = review;
            state.branch = "main".into();
            state.base_ref = "HEAD".into();
            state.base_commit = "0123456789abcdef".into();
            state.no_base_branch = true;
            let screen = render_viewer(&mut state);
            assert!(
                screen.contains("vs HEAD (no base branch found)"),
                "review={review}: {screen}"
            );
            assert!(
                screen.contains("Press b to choose a base ref."),
                "review={review}"
            );
            assert!(!screen.contains("(unknown branch)"));
        }

        // Nothing to show yet: the empty state carries the same hint.
        let (mut state, _) = single_added_line_review_state();
        state.review = false;
        state.files.clear();
        state.no_base_branch = true;
        let screen = render_viewer(&mut state);
        assert!(screen.contains("No uncommitted changes"), "{screen}");
        assert!(screen.contains("Press b to choose a base ref."));
    }

    #[test]
    fn a_resolved_base_shows_no_fallback_notice() {
        let (mut state, _) = single_added_line_review_state();
        state.branch = "feature".into();
        state.base_ref = "main".into();
        let screen = render_viewer(&mut state);
        assert!(screen.contains("vs main"), "{screen}");
        assert!(!screen.contains("no base branch found"));
    }

    #[test]
    fn a_failed_current_changes_load_offers_b_in_the_body() {
        let (mut state, _) = single_added_line_review_state();
        state.review = false;
        state.files.clear();
        state.error = Some("Could not determine a base branch for 'x'".into());
        let screen = render_viewer(&mut state);
        assert!(
            screen.contains("Could not load current changes"),
            "{screen}"
        );
        assert!(screen.contains("Press b to choose a base ref. Press r to retry."));
    }

    #[test]
    fn scope_picker_explains_missing_commits_in_one_line() {
        use ratatui::{Terminal, backend::TestBackend};
        let (viewer, _) = single_added_line_review_state();
        let mut picker = DiffPickerState {
            from_view: viewer.from_view.clone(),
            workdir: "/tmp".into(),
            commits: Vec::new(),
            selected: 0,
            error: None,
            no_base_branch: true,
            base_unresolved: false,
        };
        let render = |picker: &DiffPickerState| {
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
            terminal
                .draw(|frame| draw_diff_picker(frame, picker, &Theme::default()))
                .unwrap();
            normalized_screen(&terminal)
        };
        let screen = render(&picker);
        assert!(screen.contains("No base branch found"), "{screen}");
        assert!(screen.contains("vs HEAD"));
        assert!(!screen.contains("0 commit(s)"));

        picker.no_base_branch = false;
        picker.error = Some("Detached HEAD: commits are listed only on a named branch.".into());
        let screen = render(&picker);
        assert!(screen.contains("Detached HEAD"), "{screen}");
        assert!(!screen.contains("Could not list feature commits"));
        assert!(!screen.contains("0 commit(s)"));
    }

    #[test]
    fn review_history_modal_renders_current_state_and_lazy_archive_tail() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state
            .decisions
            .insert("a.rs".to_string(), ReviewDecision::Approve);
        state.review_history = Some(crate::app::ReviewHistoryState {
            rounds: vec![crate::app::ReviewHistoryRound {
                title: "Review — r1".to_string(),
                markdown: "## Review — r1\n\n**Approved:** 1\n".to_string(),
                carried_unresolved: 0,
            }],
            selected: 0,
            scroll: 0,
            rendered_lines: 0,
            view_height: 0,
            archive_available: true,
            archive_loaded: false,
            error: None,
            archive_error: None,
        });
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Review Timeline"));
        assert!(rendered.contains("Current"));
        assert!(rendered.contains("Last review"));
        assert!(rendered.contains("Older"));
        assert!(rendered.contains("Current Review"));
        assert!(rendered.contains("Approved: 1"));
        assert!(rendered.contains("press Enter to return to editing"));
    }

    #[test]
    fn help_modal_renders_grouped_keys_and_records_its_scroll_extent() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.help_open = true;

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Final Review"));
        assert!(rendered.contains("Verdicts"));
        assert!(rendered.contains("Approve the current file"));
        assert!(rendered.contains("Line cursor"));

        // The overlay is taller than its viewport, and the renderer has to
        // report both so `G` can clamp to the real bottom.
        assert!(state.help_rendered_lines > state.help_view_height);
        assert!(state.help_view_height > 0);
    }

    /// Structural guard on the help table: a section that renders as a bare
    /// heading, or a continuation line with nothing above it to continue, is a
    /// rendering bug that only shows up on screen.
    #[test]
    fn help_sections_are_non_empty_and_uniquely_titled() {
        // Both variants: the PR review's filtering must not leave a bare
        // heading or an orphaned continuation line behind.
        for pr_review in [false, true] {
            let mut titles = std::collections::HashSet::new();
            for (title, binds) in review_help_sections(pr_review) {
                assert!(titles.insert(title), "duplicate help section: {title}");
                assert!(!binds.is_empty(), "empty help section: {title}");
                // A blank key column is a continuation line, so it must follow
                // a real binding rather than lead a section.
                assert!(
                    !binds[0].0.is_empty(),
                    "section {title} starts with a continuation line"
                );
                for (_, desc) in &binds {
                    assert!(!desc.is_empty(), "empty help description in {title}");
                }
            }
        }
    }

    #[test]
    fn pr_review_help_leads_with_submitting_and_omits_refused_keys() {
        let sections = review_help_sections(true);
        assert_eq!(sections[0].0, "Submitting (PR review)");
        let keys: Vec<&str> = sections
            .iter()
            .flat_map(|(_, binds)| binds.iter().map(|(key, _)| *key))
            .collect();
        for refused in ["x", "E", "b", "w", "A", "O", "I", "H", "t", "X"] {
            assert!(!keys.contains(&refused), "help still lists {refused}");
        }
        let descriptions: String = sections
            .iter()
            .flat_map(|(_, binds)| binds.iter().map(|(_, desc)| *desc))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!descriptions.contains("worktree"), "{descriptions}");
        assert!(!descriptions.contains("dispatches fixes"), "{descriptions}");
        // The Final Review overlay is unchanged.
        assert_eq!(
            review_help_sections(false).len(),
            REVIEW_HELP_SECTIONS.len()
        );
    }

    #[test]
    fn review_footer_always_advertises_the_help_key() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();

        // Both footer shapes — the cursor-off hints and the cursor-mode hints —
        // point at `?`, since it's the only way to see the keys they omit. The
        // narrow width is the case that matters: the dense key row wraps into
        // both footer rows there, so a hint on the wrong line is clipped away.
        for width in [200u16, 90] {
            for cursor in [None, Some(0)] {
                state.comment_cursor = cursor;
                let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
                terminal
                    .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
                    .unwrap();
                let rendered = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>();
                assert!(
                    rendered.contains("? keys"),
                    "footer missing the help hint (width: {width}, cursor: {cursor:?})"
                );
            }
        }
    }

    /// The whole screen as one whitespace-normalized string. Rows are joined and
    /// the panel border is blanked out, so a hint the wrapper split across two
    /// rows still reads as the phrase it is — which a raw cell-by-cell
    /// concatenation (trailing padding and border glyphs and all) does not.
    fn normalized_screen(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| match buffer[(x, y)].symbol() {
                        "│" | "┃" | "║" => " ",
                        symbol => symbol,
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The round-level keys live on the footer's *second* row, and the first row
    /// is dense enough to wrap into two rows on its own at an ordinary terminal
    /// width — which used to silently swallow the second row whole. The footer
    /// now grows to fit and renders each row into its own area, so the width has
    /// to stop mattering.
    #[test]
    fn review_footer_second_row_survives_every_terminal_width() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, loc) = single_added_line_review_state();
        // A realistic mid-review footer: every conditional hint on the first row
        // is showing, which is exactly when it wraps.
        state.general_feedback = "looks close".to_string();
        state.verdict_undo.push(crate::app::VerdictUndo {
            path: "a.rs".to_string(),
            previous: None,
            previous_auto_rejected: false,
        });
        state.changed_since_last.insert("a.rs".to_string());

        for width in [200u16, 160, 120, 100, 80] {
            for cursor in [None, Some(0)] {
                state.comment_cursor = cursor;
                let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
                terminal
                    .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
                    .unwrap();
                let rendered = normalized_screen(&terminal);

                // The first row's pointer to the full key list, and hints that
                // only ever appear on the second row.
                assert!(
                    rendered.contains("? keys"),
                    "footer dropped the help hint (width: {width}, cursor: {cursor:?})"
                );
                if cursor.is_none() {
                    for hint in ["b base ref", "F filter", "target: live", "Esc pause"] {
                        assert!(
                            rendered.contains(hint),
                            "footer dropped {hint:?} (width: {width})"
                        );
                    }
                } else {
                    for hint in ["c/Esc exit cursor", "R resolve/reopen"] {
                        assert!(
                            rendered.contains(hint),
                            "cursor footer dropped {hint:?} (width: {width})"
                        );
                    }
                }
            }
        }

        // The peek box must not squeeze the hints out either: with the cursor on
        // a commented line the footer hosts both.
        state.comment_cursor = Some(0);
        state.line_comments.insert(
            "a.rs".to_string(),
            vec![crate::app::LineComment {
                location: loc,
                start: None,
                text: "needs a guard".to_string(),
                draft: false,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
        for width in [200u16, 120, 80] {
            let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
            terminal
                .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
                .unwrap();
            let rendered = normalized_screen(&terminal);
            assert!(
                rendered.contains("needs a guard"),
                "peek box lost its body (width: {width})"
            );
            assert!(
                rendered.contains("c/Esc exit cursor"),
                "peek box crowded out the hint rows (width: {width})"
            );
        }
    }

    /// The wrapper breaks on word boundaries, so the height has to be measured
    /// the same way — a `ceil(total / width)` estimate undercounts and puts the
    /// footer right back to clipping a row.
    #[test]
    fn wrapped_line_height_counts_word_wrapping_not_raw_length() {
        let line = Line::from("aaaa bbbb cccc");
        assert_eq!(wrapped_line_height(&line, 14), 1);
        // "aaaa bbbb" fits in 10; "cccc" moves down whole rather than splitting
        // at the 10th column the way a raw division would assume.
        assert_eq!(wrapped_line_height(&line, 10), 2);
        assert_eq!(wrapped_line_height(&line, 5), 3);
        // A word wider than the row is hard-broken instead of overflowing.
        assert_eq!(wrapped_line_height(&Line::from("aaaaaaaaa"), 3), 3);
        // Degenerate widths must still report a drawable row.
        assert_eq!(wrapped_line_height(&Line::from(""), 20), 1);
        assert_eq!(wrapped_line_height(&line, 0), 1);
    }

    #[test]
    fn cursor_comment_preview_surfaces_only_when_cursor_lands_on_a_comment() {
        let (mut state, loc) = single_added_line_review_state();

        // No cursor → nothing to peek.
        assert!(cursor_comment_text(&state).is_none());
        assert_eq!(cursor_comment_preview_rows(&state), 0);

        // Cursor on an un-commented line → still nothing.
        state.comment_cursor = Some(0);
        assert!(cursor_comment_text(&state).is_none());
        assert_eq!(cursor_comment_preview_rows(&state), 0);

        // A comment on the cursored line surfaces, sized to its body + border.
        state.line_comments.insert(
            "a.rs".to_string(),
            vec![crate::app::LineComment {
                location: loc,
                start: None,
                text: "needs a guard\nfor None".to_string(),
                draft: false,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
        assert_eq!(cursor_comment_text(&state), Some("needs a guard\nfor None"));
        // 2 body lines + the severity header + 2 border rows.
        assert_eq!(cursor_comment_preview_rows(&state), 5);
    }

    #[test]
    fn cursor_comment_preview_caps_long_bodies() {
        let (mut state, loc) = single_added_line_review_state();
        state.comment_cursor = Some(0);
        let body = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.line_comments.insert(
            "a.rs".to_string(),
            vec![crate::app::LineComment {
                location: loc,
                start: None,
                text: body,
                draft: false,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
        // 20 body lines (+ severity header) clamp to 6 visible + 2 border rows.
        assert_eq!(cursor_comment_preview_rows(&state), 8);
    }

    #[test]
    fn editor_hint_hides_for_a_deleted_file_in_both_footers() {
        use ratatui::{Terminal, backend::TestBackend};

        fn footer_text(state: &mut DiffViewerState) -> String {
            let backend = TestBackend::new(200, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw_diff_viewer(frame, state, &Theme::default()))
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        }

        let (mut state, _) = single_added_line_review_state();
        // A live file advertises `E` whether or not the line cursor is up.
        assert!(footer_text(&mut state).contains("$EDITOR"));
        state.comment_cursor = Some(0);
        assert!(footer_text(&mut state).contains("$EDITOR"));

        // A deletion keeps its removed lines addressable, so the cursor can
        // still sit on one — but there is nothing on disk for `E` to open.
        state.files[0].status = DiffFileStatus::Deleted;
        state.files[0].hunks[0].lines[0] = crate::diff::DiffLine {
            kind: crate::diff::DiffLineKind::Removed,
            text: "-x".to_string(),
        };
        assert!(!state.files[0].addressable_lines().is_empty());
        assert!(!footer_text(&mut state).contains("$EDITOR"));
        state.comment_cursor = None;
        assert!(!footer_text(&mut state).contains("$EDITOR"));

        // Same for a binary blob, which has nothing an editor can show.
        state.files[0].status = DiffFileStatus::Modified;
        state.files[0].is_binary = true;
        assert!(!footer_text(&mut state).contains("$EDITOR"));
        state.comment_cursor = Some(0);
        assert!(!footer_text(&mut state).contains("$EDITOR"));
    }

    #[test]
    fn file_risk_marker_flags_large_no_note_and_no_tests() {
        let (mut state, _) = single_added_line_review_state();
        let theme = Theme::default();
        // Make the single existing file large and give it no note, so it picks
        // up all three flags (the changeset also has no test-looking file).
        state.files[0].additions = 500;
        let changeset_has_tests = changeset_has_test_changes(&state.files);
        assert!(!changeset_has_tests);
        let marker = file_risk_marker(&state.files[0], &state, changeset_has_tests, &theme)
            .expect("expected a risk marker");
        assert_eq!(marker.content.as_ref(), " [L,N,T]");

        // A small file with a developer note, in a changeset that now includes
        // a test file, gets no marker at all.
        state.files[0].additions = 1;
        state
            .review_notes
            .insert("a.rs".to_string(), "reasoning".to_string());
        state.files.push(DiffFile {
            old_path: None,
            path: "tests/a_test.rs".to_string(),
            status: DiffFileStatus::Added,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        });
        let changeset_has_tests = changeset_has_test_changes(&state.files);
        assert!(changeset_has_tests);
        assert!(file_risk_marker(&state.files[0], &state, changeset_has_tests, &theme).is_none());
    }
    #[test]
    fn file_list_renders_a_directory_tree_and_folds_it() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.files = ["src/app/mod.rs", "src/app/state.rs", "src/ui/diff.rs"]
            .iter()
            .map(|path| DiffFile {
                old_path: Some((*path).to_string()),
                path: (*path).to_string(),
                status: DiffFileStatus::Modified,
                additions: 2,
                deletions: 1,
                is_binary: false,
                old_content: None,
                new_content: None,
                patch: String::new(),
                hunks: vec![],
            })
            .collect();
        state.selected_file = 0;

        // Only the file-list column: the patch panel beside it still titles
        // itself with the full path, which would mask what the rows show.
        const WIDTH: u16 = 120;
        const FILE_LIST_COLS: usize = 34;
        let render = |state: &mut DiffViewerState| {
            let backend = TestBackend::new(WIDTH, 30);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw_diff_viewer(frame, state, &Theme::default()))
                .unwrap();
            let cells: Vec<String> = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol().to_string())
                .collect();
            cells
                .chunks(WIDTH as usize)
                .map(|row| row[..FILE_LIST_COLS].concat())
                .collect::<Vec<String>>()
                .join("\n")
        };

        let rendered = render(&mut state);
        // Directory headers replace the repeated path prefixes, and file rows
        // show only their basename.
        assert!(
            rendered.contains("│ ▾ src/"),
            "top-level directories should only reserve the selection gutter: {rendered}"
        );
        assert!(
            rendered.contains("│>  · M mod.rs"),
            "file marks should follow nesting without extra leading padding: {rendered}"
        );
        assert!(
            rendered.contains("  ▾ app/"),
            "expected a nested, indented app directory row: {rendered}"
        );
        assert!(rendered.contains("mod.rs"));
        assert!(
            !rendered.contains("src/app/mod.rs"),
            "file rows should drop the prefix their directory rows already show"
        );

        // Folding hides the files beneath and summarises what it swallowed.
        state.toggle_dir_collapsed("src/app");
        let folded = render(&mut state);
        assert!(!folded.contains("mod.rs"));
        assert!(folded.contains("▸ app/"));
        assert!(
            folded.contains("(2)"),
            "collapsed row should count its files"
        );
    }

    #[test]
    fn collapsed_dir_summary_only_counts_files_the_filter_shows() {
        let (mut state, _) = single_added_line_review_state();
        state.files = ["src/app/mod.rs", "src/app/state.rs", "src/app/ui.rs"]
            .iter()
            .map(|path| DiffFile {
                old_path: Some((*path).to_string()),
                path: (*path).to_string(),
                status: DiffFileStatus::Modified,
                additions: 2,
                deletions: 1,
                is_binary: false,
                old_content: None,
                new_content: None,
                patch: String::new(),
                hunks: vec![],
            })
            .collect();
        state.selected_file = 0;
        // Only mod.rs stays undecided; the other two are decided and one of them
        // also changed since the last round.
        state
            .decisions
            .insert("src/app/state.rs".to_string(), ReviewDecision::Approve);
        state.decisions.insert(
            "src/app/ui.rs".to_string(),
            ReviewDecision::Reject {
                feedback: "no".to_string(),
                severity: crate::app::Severity::default(),
            },
        );
        state
            .changed_since_last
            .insert("src/app/state.rs".to_string());
        state.file_filter = crate::app::FileFilter::Undecided;
        state.toggle_dir_collapsed("src/app");

        let rows = state.file_tree_rows();
        let files = rows
            .iter()
            .find_map(|row| match row {
                crate::app::FileTreeRow::Dir { path, files, .. } if path == "src/app" => {
                    Some(*files)
                }
                _ => None,
            })
            .expect("the collapsed src/app row");
        let visible = state.visible_file_indices();
        let summary: String =
            dir_row_summary("src/app", files, &visible, &state, &Theme::default())
                .iter()
                .map(|span| span.content.as_ref())
                .collect();

        // The badges describe the one file the row is actually hiding — not the
        // decisions and Δ of the two the filter dropped.
        assert_eq!(summary.trim(), "(1) ·1", "summary was {summary:?}");
    }

    /// The patch panel renders from a cursor-free cache and paints the cursor
    /// on afterwards; the result must match rendering every row with the
    /// cursor built in, including a cursor line that wraps and one scrolled
    /// partly out of view.
    #[test]
    fn cached_patch_panel_matches_a_full_render_with_the_cursor_built_in() {
        use ratatui::{Terminal, backend::TestBackend};
        use std::collections::HashSet;

        let long = "x".repeat(70);
        let file = DiffFile {
            old_path: None,
            path: "notes.txt".into(),
            status: DiffFileStatus::Modified,
            additions: 2,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: "diff --git a/notes.txt b/notes.txt\n".into(),
            hunks: vec![DiffHunk {
                header: "@@ -1,3 +1,4 @@".into(),
                old_start: 1,
                old_lines: 3,
                new_start: 1,
                new_lines: 4,
                lines: vec![
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: " alpha".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Removed,
                        text: "-beta".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: format!("+{long}"),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "+gamma".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: " delta".into(),
                    },
                ],
            }],
        };
        let theme = Theme::default();
        let wrapped = DiffLineLocation {
            old_line: None,
            new_line: Some(2),
        };
        let removed = DiffLineLocation {
            old_line: Some(2),
            new_line: None,
        };
        let context = DiffLineLocation {
            old_line: Some(3),
            new_line: Some(4),
        };
        let set = |locs: &[DiffLineLocation]| locs.iter().copied().collect::<HashSet<_>>();

        let cases = [
            (wrapped, set(&[]), set(&[]), set(&[context])),
            (wrapped, set(&[wrapped]), set(&[]), set(&[])),
            (removed, set(&[]), set(&[removed]), set(&[])),
            (context, set(&[removed]), set(&[wrapped]), set(&[context])),
        ];
        for (cursor, commented, draft, matched) in cases {
            for scroll in 0..6 {
                let render = |cached: bool| {
                    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
                    terminal
                        .draw(|frame| {
                            let area = frame.area();
                            if cached {
                                draw_patch_panel(
                                    frame,
                                    area,
                                    Some(&file),
                                    PatchPanelOptions {
                                        title: "Patch".into(),
                                        scroll,
                                        cursor: Some(cursor),
                                        commented: commented.clone(),
                                        draft: draft.clone(),
                                        matched: matched.clone(),
                                        ..Default::default()
                                    },
                                    &theme,
                                );
                            } else {
                                let lines = patch_lines(
                                    &file,
                                    area.width - 2,
                                    &theme,
                                    true,
                                    false,
                                    Some(cursor),
                                    &commented,
                                    &draft,
                                    &HashSet::new(),
                                    &HashSet::new(),
                                    &matched,
                                    &mut None,
                                );
                                let block = Block::default()
                                    .title(" Patch [unified] ")
                                    .borders(Borders::ALL)
                                    .border_style(Style::default().fg(Color::Reset));
                                frame.render_widget(
                                    Paragraph::new(lines)
                                        .block(block)
                                        .scroll((scroll as u16, 0))
                                        .wrap(Wrap { trim: false }),
                                    area,
                                );
                            }
                        })
                        .unwrap();
                    terminal.backend().buffer().clone()
                };
                assert_eq!(
                    render(true),
                    render(false),
                    "cursor {cursor:?} at scroll {scroll}"
                );
            }
        }
    }
}
