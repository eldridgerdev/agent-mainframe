use super::*;
use unicode_width::UnicodeWidthStr;

pub(in crate::ui::dialogs) fn draw_patch(
    frame: &mut Frame,
    area: Rect,
    state: &mut DiffViewerState,
    theme: &Theme,
) {
    draw_patch_with_gutter(frame, area, state, theme, false);
}

/// [`draw_patch`] with a choice of unified gutter. `compact_gutter` shows one
/// line-number column instead of old + new, for panes too narrow to spend two
/// columns of numbers on every wrapped row (the PR workspace's code pane).
pub(in crate::ui::dialogs) fn draw_patch_with_gutter(
    frame: &mut Frame,
    area: Rect,
    state: &mut DiffViewerState,
    theme: &Theme,
    compact_gutter: bool,
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
                compact_gutter,
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
            compact_gutter,
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
    /// One line-number column (new, or old for a removed line) instead of two
    /// (unified view only). For narrow panes, where every gutter column is
    /// taken from wrapped code.
    pub compact_gutter: bool,
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
            compact_gutter: false,
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
        compact_gutter: options.compact_gutter,
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
    /// The unified gutter has one number column rather than two.
    compact_gutter: bool,
}

struct PatchRowsRequest<'a> {
    file: &'a DiffFile,
    width: u16,
    theme: &'a Theme,
    layout: DiffViewerLayout,
    include_prologue: bool,
    new_file_presentation: bool,
    compact_gutter: bool,
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
        self.compact_gutter.hash(&mut hasher);

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
                self.compact_gutter,
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
            compact_gutter: self.compact_gutter,
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

/// Column of the unified gutter's marker glyph: after one number column and a
/// space when compact, after two of each otherwise. The gutter is this plus
/// the two-cell marker.
fn gutter_marker_col(number_width: usize, compact: bool) -> usize {
    if compact {
        number_width + 1
    } else {
        number_width * 2 + 2
    }
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
    let marker_col = gutter_marker_col(number_width, rows.compact_gutter);
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
            let is_number = dx < number_width
                || (!rows.compact_gutter && (number_width + 1..number_width * 2 + 1).contains(&dx));
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
pub(super) fn patch_lines(
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
        false,
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
pub(super) fn indexed_patch_lines(
    file: &DiffFile,
    width: u16,
    theme: &Theme,
    include_prologue: bool,
    new_file_presentation: bool,
    compact_gutter: bool,
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
    let columns = GutterColumns {
        number_width,
        compact: compact_gutter,
    };
    let gutter_width = gutter_marker_col(number_width, compact_gutter) + 2;
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
                columns,
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
            columns,
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
                        columns,
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
                        columns,
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
                        columns,
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
                        columns,
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

pub(in crate::ui::dialogs) fn effective_layout(state: &DiffViewerState) -> DiffViewerLayout {
    state
        .files
        .get(state.selected_file)
        .filter(|file| is_new_diff_file(file))
        .map(|_| DiffViewerLayout::Unified)
        .unwrap_or_else(|| state.layout.clone())
}

pub(in crate::ui::dialogs) fn is_new_diff_file(file: &DiffFile) -> bool {
    matches!(
        file.status,
        DiffFileStatus::Added | DiffFileStatus::Untracked
    )
}

pub(in crate::ui::dialogs) fn status_label(status: &DiffFileStatus) -> &'static str {
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

pub(in crate::ui::dialogs) fn status_color(
    status: &DiffFileStatus,
    theme: &Theme,
) -> ratatui::style::Color {
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

/// The unified gutter's number columns: two (old, new) of `number_width`
/// each, or one when `compact`, which shows the new line number and falls
/// back to the old one for a removed line. The row's `+`/`-` sign and colour
/// still say which side a compact number belongs to.
#[derive(Clone, Copy)]
struct GutterColumns {
    number_width: usize,
    compact: bool,
}

#[allow(clippy::too_many_arguments)]
fn wrap_gutter_line(
    old_number: Option<usize>,
    new_number: Option<usize>,
    chunks: Vec<StyledChunk>,
    style: Style,
    columns: GutterColumns,
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
    let number_width = columns.number_width;
    let label = |number: Option<usize>, index: usize| {
        if index == 0 {
            line_number_label(number, number_width)
        } else {
            blank_line_number_label(number_width)
        }
    };
    for (index, chunk_line) in wrapped.into_iter().enumerate() {
        // The marker glyph only shows on the first visual line of a logical
        // row; continuation lines keep the plain separator (tinted on cursor).
        let (sep_text, sep_fg) = if index == 0 {
            (marker, marker_fg)
        } else {
            ("│ ", number_fg)
        };
        let number_style = Style::default().fg(number_fg).bg(gutter_bg);
        let mut line = if columns.compact {
            vec![Span::styled(
                label(new_number.or(old_number), index),
                number_style,
            )]
        } else {
            vec![
                Span::styled(label(old_number, index), number_style),
                Span::styled(" ", Style::default().bg(gutter_bg)),
                Span::styled(label(new_number, index), number_style),
            ]
        };
        line.extend([
            Span::styled(" ", Style::default().bg(gutter_bg)),
            Span::styled(
                sep_text.to_string(),
                Style::default().fg(sep_fg).bg(gutter_bg),
            ),
        ]);
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

pub(super) fn added_row_style(theme: &Theme) -> Style {
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
pub(super) fn added_emphasis_style(theme: &Theme) -> Style {
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
pub(super) fn hunk_intra_line_ranges(hunk: &DiffHunk) -> Vec<Option<Vec<std::ops::Range<usize>>>> {
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

pub(super) fn new_file_added_row_style(theme: &Theme) -> Style {
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
