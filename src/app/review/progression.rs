use super::preparation::{FEEDBACK_TITLE, parse_review_history_rounds};
use crate::app::{
    App, AppMode, DiffViewerState, FileComment, FileFilter, ReviewDecision, ReviewHistoryState,
    SummaryItem,
};
use anyhow::Result;
impl App {
    /// Approve the file currently selected in the review viewer and advance.
    pub fn diff_review_approve_current(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            if let Some(path) = state.files.get(state.selected_file).map(|f| f.path.clone()) {
                state.push_verdict_undo(&path, Some(&ReviewDecision::Approve));
                state
                    .decisions
                    .insert(path.clone(), ReviewDecision::Approve);
                // An explicit verdict wins over (and sticks against) a
                // rejection auto-set by a line comment.
                state.auto_rejected.remove(&path);
            }
        }
        self.diff_review_advance();
        self.persist_review_progress();
    }

    /// Skip the current file (clear any prior verdict) and advance.
    pub fn diff_review_skip_current(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            if let Some(path) = state.files.get(state.selected_file).map(|f| f.path.clone()) {
                state.push_verdict_undo(&path, None);
                state.decisions.remove(&path);
                // An explicit skip clears a comment-implied rejection. A later
                // comment mutation on the file re-defaults it — a fresh signal
                // on a file with no verdict.
                state.auto_rejected.remove(&path);
            }
        }
        self.diff_review_advance();
        self.persist_review_progress();
    }

    /// Toggle the per-line comment cursor in the review viewer. When turning it
    /// on, place it on the first changed (added/removed) line of the current
    /// file, or the first line if the file is all context.
    pub fn diff_review_toggle_line_cursor(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            if state.comment_cursor.is_some() {
                state.comment_cursor = None;
                state.comment_anchor = None;
                return;
            }
            let locs = state
                .files
                .get(state.selected_file)
                .map(|f| f.addressable_lines())
                .unwrap_or_default();
            if locs.is_empty() {
                self.message = Some("No diff lines to comment on".to_string());
                return;
            }
            let first_change = locs
                .iter()
                .position(|l| l.old_line.is_none() || l.new_line.is_none())
                .unwrap_or(0);
            state.comment_cursor = Some(first_change);
            state.cursor_sync_to_view = true;
        }
    }

    /// Move the comment cursor by `delta` lines (negative = up), clamped to the
    /// current file's addressable lines.
    pub fn diff_review_cursor_move(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let Some(cur) = state.comment_cursor else {
                return;
            };
            let len = state
                .files
                .get(state.selected_file)
                .map(|f| f.addressable_lines().len())
                .unwrap_or(0);
            if len == 0 {
                state.comment_cursor = None;
                return;
            }
            let max = len - 1;
            let next = if delta < 0 {
                cur.saturating_sub((-delta) as usize)
            } else {
                cur.saturating_add(delta as usize).min(max)
            };
            state.comment_cursor = Some(next.min(max));
            state.cursor_sync_to_view = true;
        }
    }

    /// Jump the line cursor to the next (`delta > 0`) or previous hunk's first
    /// line, activating the cursor if it is off (next lands on the first hunk,
    /// prev on the last). "Previous" picks the largest hunk start strictly below
    /// the cursor, so from mid-hunk it first snaps back to the current hunk's
    /// start. The selection anchor is kept, so a `v` range can extend across
    /// hunks; the patch follows via the cursor sync.
    pub fn diff_review_jump_hunk(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let Some(file) = state.files.get(state.selected_file) else {
                return;
            };
            let starts = file.hunk_start_indices();
            let (Some(&first), Some(&last)) = (starts.first(), starts.last()) else {
                self.message = Some("No hunks to jump between".to_string());
                return;
            };
            let target = match state.comment_cursor {
                None => {
                    if delta > 0 {
                        first
                    } else {
                        last
                    }
                }
                Some(cur) if delta > 0 => {
                    match starts.iter().find(|&&idx| idx > cur) {
                        Some(&next) => next,
                        None => return, // already in the last hunk
                    }
                }
                Some(cur) => match starts.iter().rev().find(|&&idx| idx < cur) {
                    Some(&prev) => prev,
                    None => return, // already at the first hunk's start
                },
            };
            state.comment_cursor = Some(target);
            state.cursor_sync_to_view = true;
        }
    }

    /// Open the diff search prompt in the review viewer (`/`). Requires the
    /// current file to have addressable diff lines, and activates the line
    /// cursor if it is off so matches have a jump target. Keeps any existing
    /// query as the editable seed so `/`↵ repeats the last search.
    pub fn diff_review_start_search(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let has_lines = state
                .files
                .get(state.selected_file)
                .is_some_and(|f| !f.addressable_lines().is_empty());
            if !has_lines {
                self.message = Some("Nothing to search in this file".to_string());
                return;
            }
            if state.comment_cursor.is_none() {
                state.comment_cursor = Some(0);
                state.cursor_sync_to_view = true;
            }
            state.editing_search = true;
        }
    }

    /// Append a typed character to the search query and re-run the incremental
    /// jump. Control characters are ignored.
    pub fn diff_search_input(&mut self, c: char) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.editing_search || c.is_control() {
                return;
            }
            state.search_query.push(c);
        } else {
            return;
        }
        self.diff_search_recompute_and_jump();
    }

    /// Delete the last character of the search query and re-run the incremental
    /// jump.
    pub fn diff_search_backspace(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.editing_search {
                return;
            }
            state.search_query.pop();
        } else {
            return;
        }
        self.diff_search_recompute_and_jump();
    }

    /// Commit the search: close the prompt but keep the query and matches so
    /// `n`/`N` can cycle them. Reports the hit count (or a miss); a blank query
    /// just clears the search.
    pub fn diff_search_submit(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.editing_search = false;
            if state.search_query.trim().is_empty() {
                state.clear_search();
                return;
            }
            if state.search_matches.is_empty() {
                self.message = Some(format!("No matches for \"{}\"", state.search_query));
            } else {
                self.message = Some(format!(
                    "Match {}/{} for \"{}\" — n/N next/prev, Esc clear",
                    state.search_match_pos.map(|p| p + 1).unwrap_or(0),
                    state.search_matches.len(),
                    state.search_query
                ));
            }
        }
    }

    /// Cancel the search prompt (Esc while typing), discarding the query.
    pub fn diff_search_cancel(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.clear_search();
        }
    }

    /// Clear a committed search (Esc while it is active), restoring `n`/`N` to
    /// their file-navigation meaning. Leaves the line cursor where it is.
    pub fn diff_search_clear(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.clear_search();
        }
    }

    /// Whether a committed (non-editing) search is active, so match-navigation
    /// keys should shadow file navigation in the key layer.
    pub fn diff_search_active(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::DiffViewer(state)
                if state.review && !state.editing_search && !state.search_query.trim().is_empty()
        )
    }

    /// Move to the next (`delta > 0`) / previous (`delta < 0`) match, wrapping,
    /// and land the line cursor on it. Reports a miss when there are no matches.
    pub fn diff_search_next(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if state.search_matches.is_empty() {
                self.message = Some(format!("No matches for \"{}\"", state.search_query));
                return;
            }
            let len = state.search_matches.len();
            let cur = state.search_match_pos.unwrap_or(0) as isize;
            let next = (cur + delta).rem_euclid(len as isize) as usize;
            state.search_match_pos = Some(next);
            state.comment_cursor = Some(state.search_matches[next]);
            state.cursor_sync_to_view = true;
            self.message = Some(format!(
                "Match {}/{} for \"{}\"",
                next + 1,
                len,
                state.search_query
            ));
        }
    }

    /// Recompute matches for the current file + query and jump the cursor to the
    /// first match at or after its current position (wrapping to the first), so
    /// the view stays anchored as the reviewer types. Leaves the cursor put when
    /// there is no match.
    pub(super) fn diff_search_recompute_and_jump(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let query = state.search_query.clone();
            let matches = state
                .files
                .get(state.selected_file)
                .map(|f| compute_search_matches(f, &query))
                .unwrap_or_default();
            if matches.is_empty() {
                state.search_matches = matches;
                state.search_match_pos = None;
                return;
            }
            let from = state.comment_cursor.unwrap_or(0);
            let pos = matches.iter().position(|&idx| idx >= from).unwrap_or(0);
            state.comment_cursor = Some(matches[pos]);
            state.cursor_sync_to_view = true;
            state.search_matches = matches;
            state.search_match_pos = Some(pos);
        }
    }

    /// Close the changeset-overview modal. The cached overview (if any) is kept
    /// so reopening with `O` doesn't re-run the headless pass.
    pub fn close_changeset_overview(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.changeset_overview_open = false;
        }
    }

    /// Max scroll offset for the changeset-overview modal, in rendered
    /// (markdown-wrapped) visual lines. Mirrors `review_note_max_scroll`.
    pub(super) fn changeset_overview_max_scroll(state: &DiffViewerState) -> usize {
        state
            .changeset_overview_rendered_lines
            .saturating_sub(state.changeset_overview_view_height)
    }

    pub fn changeset_overview_scroll_down(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let max = Self::changeset_overview_max_scroll(state);
            state.changeset_overview_scroll = (state.changeset_overview_scroll + amount).min(max);
        }
    }

    pub fn changeset_overview_scroll_up(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.changeset_overview_scroll =
                state.changeset_overview_scroll.saturating_sub(amount);
        }
    }

    pub fn changeset_overview_scroll_top(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.changeset_overview_scroll = 0;
        }
    }

    pub fn changeset_overview_scroll_bottom(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.changeset_overview_scroll = Self::changeset_overview_max_scroll(state);
        }
    }

    /// Open the review-mode key-help overlay (`?`). Review-only: the plain diff
    /// viewer's key surface still fits in its footer. Always reopens at the top
    /// so `?` lands on the same first screen every time.
    pub fn open_review_help(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && state.review
        {
            state.help_open = true;
            state.help_scroll = 0;
        }
    }

    pub fn close_review_help(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.help_open = false;
        }
    }

    /// Max scroll offset for the help overlay, in the visual lines the renderer
    /// last reported. Mirrors `changeset_overview_max_scroll`.
    pub(super) fn review_help_max_scroll(state: &DiffViewerState) -> usize {
        state
            .help_rendered_lines
            .saturating_sub(state.help_view_height)
    }

    pub fn review_help_scroll_down(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let max = Self::review_help_max_scroll(state);
            state.help_scroll = (state.help_scroll + amount).min(max);
        }
    }

    pub fn review_help_scroll_up(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.help_scroll = state.help_scroll.saturating_sub(amount);
        }
    }

    pub fn review_help_scroll_top(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.help_scroll = 0;
        }
    }

    pub fn review_help_scroll_bottom(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.help_scroll = Self::review_help_max_scroll(state);
        }
    }

    /// Undo the most recent explicit verdict (`a` / `s` / a typed rejection),
    /// restoring the file's previous decision — including whether it was one
    /// the line-comment rule had set implicitly — and returning the selection
    /// to that file, since the verdict keys advance away from it. Comments,
    /// suggestions and general feedback are untouched: only verdicts are undone.
    pub fn diff_review_undo_verdict(&mut self) {
        // `(message, whether anything actually changed)` — "nothing to undo"
        // still reports, but must not rewrite the progress file.
        let outcome: Option<(String, bool)> = match &mut self.mode {
            AppMode::DiffViewer(state) if state.review => match state.verdict_undo.pop() {
                None => Some(("No verdict to undo".to_string(), false)),
                Some(entry) => {
                    match entry.previous.clone() {
                        Some(decision) => {
                            state.decisions.insert(entry.path.clone(), decision);
                        }
                        None => {
                            state.decisions.remove(&entry.path);
                        }
                    }
                    if entry.previous_auto_rejected {
                        state.auto_rejected.insert(entry.path.clone());
                    } else {
                        state.auto_rejected.remove(&entry.path);
                    }
                    let restored = match &entry.previous {
                        None => "no verdict",
                        Some(ReviewDecision::Approve) => "approved",
                        Some(ReviewDecision::Reject { .. }) => "needs revision",
                    };
                    match state.files.iter().position(|f| f.path == entry.path) {
                        Some(idx) => {
                            if state.selected_file != idx {
                                state.selected_file = idx;
                                state.on_file_changed();
                            } else {
                                state.reveal_selected_file();
                            }
                            // Restoring a verdict can push the file back out of the
                            // active filter; say so rather than leaving the reviewer
                            // wondering why the list didn't move.
                            let hidden = !state.visible_file_indices().contains(&idx);
                            let filter_note = if hidden {
                                format!(" (hidden by the {} filter)", state.file_filter.label())
                            } else {
                                String::new()
                            };
                            Some((
                                format!(
                                    "Undid verdict on {} — {restored}{filter_note}",
                                    entry.path
                                ),
                                true,
                            ))
                        }
                        // The file left the changeset since the verdict was set (a
                        // refresh, a base-ref change). The decision is still worth
                        // restoring; there is just nowhere to navigate to.
                        None => Some((
                            format!(
                                "Undid verdict on {} — {restored} (no longer in the diff)",
                                entry.path
                            ),
                            true,
                        )),
                    }
                }
            },
            _ => None,
        };
        if let Some((message, changed)) = outcome {
            self.message = Some(message);
            if changed {
                self.persist_review_progress();
            }
        }
    }

    pub(super) fn diff_review_advance(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            // Advance to the next *visible* file after the current one. Under a
            // filter (e.g. Undecided) the file just decided drops out of the
            // list, so this lands on the next item still needing attention.
            let next = state
                .visible_file_indices()
                .into_iter()
                .find(|&i| i > state.selected_file);
            if let Some(idx) = next {
                state.selected_file = idx;
                state.on_file_changed();
            }
        }
    }

    /// Toggle the full-height developer-notes panel in the review viewer.
    pub fn toggle_review_notes_expanded(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && state.review
        {
            state.notes_expanded = !state.notes_expanded;
            state.notes_scroll = 0;
        }
    }

    /// Max scroll offset for the current file's note, in rendered (markdown-
    /// wrapped) visual lines. Uses the line count and viewport height recorded
    /// by the renderer so a long soft-wrapped note scrolls fully to its visual
    /// bottom rather than clamping at the raw line count.
    pub(super) fn review_note_max_scroll(state: &DiffViewerState) -> usize {
        state
            .notes_rendered_lines
            .saturating_sub(state.notes_view_height)
    }

    pub fn review_notes_scroll_down(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let max = Self::review_note_max_scroll(state);
            state.notes_scroll = (state.notes_scroll + amount).min(max);
        }
    }

    pub fn review_notes_scroll_up(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.notes_scroll = state.notes_scroll.saturating_sub(amount);
        }
    }

    pub fn review_notes_scroll_top(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.notes_scroll = 0;
        }
    }

    pub fn review_notes_scroll_bottom(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.notes_scroll = Self::review_note_max_scroll(state);
        }
    }

    /// Number of files in the current review that have no verdict
    /// (neither approved, rejected, nor explicitly skipped via `s`).
    pub(super) fn diff_review_undecided_count(state: &DiffViewerState) -> usize {
        state
            .files
            .iter()
            .filter(|file| !state.decisions.contains_key(&file.path))
            .count()
    }

    /// Finish the review, but if some files still have no verdict, gate the
    /// finish behind a confirmation rather than ending silently. A second
    /// confirm (handled in the key layer) opens the pre-finish summary, same
    /// as when nothing is undecided.
    pub fn confirm_or_finish_review(&mut self) -> Result<()> {
        let undecided = match &self.mode {
            AppMode::DiffViewer(state) if state.review => Self::diff_review_undecided_count(state),
            _ => return self.finish_final_review(),
        };
        if undecided == 0 {
            self.open_review_summary();
            return Ok(());
        }
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.finish_confirm = true;
        }
        self.message = Some(format!(
            "{undecided} file(s) have no verdict — q/y to finish anyway, u to jump to the next, \
             Esc to keep reviewing"
        ));
        Ok(())
    }

    /// Open the pre-finish summary: every file's verdict, every open comment
    /// and suggestion, and the general feedback, in one navigable list — a
    /// last look before `q` from here actually writes the feedback file and
    /// dispatches it. Clears any pending undecided-files confirmation, since
    /// reaching the summary means that gate (if any) has already been passed.
    pub fn open_review_summary(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            state.finish_confirm = false;
            state.summary_selected = 0;
            state.summary_open = true;
        }
    }

    pub fn close_review_summary(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.summary_open = false;
        }
    }

    /// Open the read-only review-round timeline. Only the bounded live feedback
    /// log is read here; the archive is deliberately deferred until navigation
    /// reaches past the loaded tail so browsing history never puts old rounds
    /// back on the fixing agent's normal read path.
    pub fn open_review_history(&mut self) {
        let workdir = match &self.mode {
            AppMode::DiffViewer(state) if state.review => state.workdir.clone(),
            _ => return,
        };
        let live_path = workdir.join(".claude").join("final-review-feedback.md");
        let archive_path = workdir
            .join(".claude")
            .join("final-review-feedback-archive.md");

        let (rounds, error) = match std::fs::read_to_string(&live_path) {
            Ok(content) => (parse_review_history_rounds(&content, FEEDBACK_TITLE), None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
            Err(e) => (
                Vec::new(),
                Some(format!("Could not read review history: {e}")),
            ),
        };
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.review_history = Some(ReviewHistoryState {
                rounds,
                selected: 0,
                scroll: 0,
                rendered_lines: 0,
                view_height: 0,
                archive_available: archive_path.is_file(),
                archive_loaded: false,
                error,
            });
        }
    }

    pub fn close_review_history(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.review_history = None;
        }
    }

    /// Load archived review rounds newest-first. The archive itself is written
    /// oldest-first (older overflow chunks are appended), so reverse it before
    /// extending the live newest-first timeline.
    pub(super) fn load_review_history_archive(&mut self) {
        let workdir = match &self.mode {
            AppMode::DiffViewer(state)
                if state
                    .review_history
                    .as_ref()
                    .is_some_and(|h| h.archive_available && !h.archive_loaded) =>
            {
                state.workdir.clone()
            }
            _ => return,
        };
        let path = workdir
            .join(".claude")
            .join("final-review-feedback-archive.md");
        let result = std::fs::read_to_string(&path).map(|content| {
            let mut rounds =
                parse_review_history_rounds(&content, "# Final Review Feedback Archive\n\n");
            rounds.reverse();
            rounds
        });
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(history) = &mut state.review_history
        {
            history.archive_loaded = true;
            match result {
                Ok(rounds) => history.rounds.extend(rounds),
                Err(e) => {
                    history.error = Some(format!("Could not read archived review history: {e}"))
                }
            }
        }
    }

    /// Move left/right through `Current` and finished rounds. Crossing the
    /// loaded tail is the one operation that triggers the lazy archive read.
    pub fn review_history_move(&mut self, delta: isize) {
        let should_load = matches!(
            &self.mode,
            AppMode::DiffViewer(state)
                if state.review_history.as_ref().is_some_and(|h| {
                    delta > 0
                        && h.selected == h.rounds.len()
                        && h.archive_available
                        && !h.archive_loaded
                })
        );
        if should_load {
            self.load_review_history_archive();
        }
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(history) = &mut state.review_history
        {
            let max = history.rounds.len();
            let next = (history.selected as isize)
                .saturating_add(delta)
                .clamp(0, max as isize);
            if next as usize != history.selected {
                history.selected = next as usize;
                history.scroll = 0;
            }
        }
    }

    pub(super) fn review_history_max_scroll(state: &DiffViewerState) -> usize {
        state
            .review_history
            .as_ref()
            .map(|h| h.rendered_lines.saturating_sub(h.view_height))
            .unwrap_or(0)
    }

    pub fn review_history_scroll_down(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let max = Self::review_history_max_scroll(state);
            if let Some(history) = &mut state.review_history {
                history.scroll = history.scroll.saturating_add(amount).min(max);
            }
        }
    }

    pub fn review_history_scroll_up(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(history) = &mut state.review_history
        {
            history.scroll = history.scroll.saturating_sub(amount);
        }
    }

    pub fn review_history_scroll_top(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && let Some(history) = &mut state.review_history
        {
            history.scroll = 0;
        }
    }

    pub fn review_history_scroll_bottom(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let max = Self::review_history_max_scroll(state);
            if let Some(history) = &mut state.review_history {
                history.scroll = max;
            }
        }
    }

    /// Move the summary selection by `delta` rows, clamped to the list.
    /// `isize::MIN/2` / `isize::MAX/2` jump to the top/bottom, mirroring
    /// `diff_review_cursor_move`'s g/G handling.
    pub fn review_summary_move(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.summary_open {
                return;
            }
            let len = state.summary_items().len();
            if len == 0 {
                return;
            }
            let next = (state.summary_selected as isize)
                .saturating_add(delta)
                .clamp(0, len as isize - 1);
            state.summary_selected = next as usize;
        }
    }

    /// Jump back into the diff at the selected summary row and close the
    /// summary. Where there's exactly one unambiguous thing to edit — a line
    /// comment, a file comment, a rejection's feedback, or the general
    /// feedback — open that editor directly, pre-filled, so the reviewer lands
    /// ready to type rather than having to re-find and re-press the key.
    pub fn review_summary_jump_to_selected(&mut self) {
        let item = if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.summary_open {
                return;
            }
            let item = state.summary_items().get(state.summary_selected).copied();
            state.summary_open = false;
            item
        } else {
            return;
        };
        let Some(item) = item else {
            return;
        };
        match item {
            SummaryItem::General => self.diff_review_start_general_feedback(),
            SummaryItem::File { file_idx } => {
                let is_reject = if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.selected_file = file_idx;
                    state.on_file_changed();
                    matches!(
                        state
                            .files
                            .get(file_idx)
                            .and_then(|f| state.decisions.get(&f.path)),
                        Some(ReviewDecision::Reject { .. })
                    )
                } else {
                    false
                };
                if is_reject {
                    self.diff_review_start_feedback();
                }
            }
            SummaryItem::FileComment { file_idx } => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.selected_file = file_idx;
                    state.on_file_changed();
                }
                self.diff_review_start_file_comment();
            }
            SummaryItem::LineComment {
                file_idx,
                comment_idx,
            } => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.selected_file = file_idx;
                    state.on_file_changed();
                    if let Some(file) = state.files.get(file_idx) {
                        let path = file.path.clone();
                        let locs = file.addressable_lines();
                        if let Some(range) = state
                            .line_comments
                            .get(&path)
                            .and_then(|comments| comments.get(comment_idx))
                            .and_then(|comment| comment.covered_indices(&locs))
                        {
                            state.comment_anchor =
                                (*range.start() != *range.end()).then_some(*range.start());
                            state.comment_cursor = Some(*range.end());
                            state.cursor_sync_to_view = true;
                        }
                    }
                }
                self.diff_review_start_line_comment();
            }
        }
    }

    /// Leave the review viewer without finishing it: no feedback file, no PR
    /// post, no fix dispatch, and the progress/snapshot files are left
    /// exactly as they are. Decisions/comments/filters are already persisted
    /// incrementally by `persist_review_progress` on every mutation, so
    /// pausing only needs to return to the feature view.
    pub fn pause_final_review(&mut self) {
        if let AppMode::DiffViewer(state) = &self.mode
            && state.finish_check_child.is_some()
        {
            self.message = Some("Finish check still running — wait for it to finish".to_string());
            return;
        }
        self.close_diff_viewer();
        self.message = Some("Review paused — progress saved, press f to resume".to_string());
    }

    /// Move the selection to the next file with no verdict (wrapping), so a
    /// reviewer can sweep up everything they skipped past.
    pub fn diff_review_jump_next_undecided(&mut self) {
        let found = match &self.mode {
            AppMode::DiffViewer(state) if state.review => {
                let n = state.files.len();
                (1..=n).find_map(|offset| {
                    let idx = (state.selected_file + offset) % n;
                    (!state.decisions.contains_key(&state.files[idx].path)).then_some(idx)
                })
            }
            _ => return,
        };
        match found {
            Some(idx) => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.selected_file = idx;
                    state.on_file_changed();
                }
            }
            None => self.message = Some("All files have a verdict".to_string()),
        }
    }

    /// Cycle the review file-list filter (All → Undecided → Rejected → All). If
    /// the active selection is hidden by the new filter, snap it onto the first
    /// visible file at-or-after it so navigation stays anchored to the list.
    pub fn diff_review_cycle_file_filter(&mut self) {
        let msg = if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            state.file_filter = state.file_filter.next();
            if state.file_filter == FileFilter::FileComments
                && !state
                    .file_comments
                    .values()
                    .any(FileComment::is_open_thread)
            {
                state.file_filter = state.file_filter.next();
            }
            // `Unresolved` is only meaningful when at least one open thread
            // exists; `Changed` only when a prior review exists to compare
            // against. Otherwise skip straight past them.
            if state.file_filter == FileFilter::Unresolved && state.unresolved_thread_count() == 0 {
                state.file_filter = state.file_filter.next();
            }
            if state.file_filter == FileFilter::Changed && !state.has_prior_review {
                state.file_filter = state.file_filter.next();
            }
            let visible = state.visible_file_indices();
            if visible.is_empty() {
                format!("Filter: {} (no matching files)", state.file_filter.label())
            } else {
                if !visible.contains(&state.selected_file) {
                    let idx = visible
                        .iter()
                        .copied()
                        .find(|&i| i >= state.selected_file)
                        .unwrap_or_else(|| *visible.last().unwrap());
                    state.selected_file = idx;
                    state.on_file_changed();
                }
                format!(
                    "Filter: {} ({}/{} files)",
                    state.file_filter.label(),
                    visible.len(),
                    state.files.len()
                )
            }
        } else {
            return;
        };
        self.message = Some(msg);
    }

    /// Open the destination picker (`t`): choose where a finished review's
    /// "address the feedback" prompt is dispatched — the feature's existing
    /// agent pane, a fresh dedicated review session, another existing feature's
    /// session, or a brand-new companion feature. Replaces the old two-state
    /// toggle; see [`Self::review_destination_pick_confirm`].
    pub fn diff_review_toggle_fix_target(&mut self) {
        self.open_review_destination_picker();
    }
}

/// Indices into `file.addressable_lines()` whose text contains `query`
/// (case-insensitive substring), ascending. Empty for a blank query. Matches
/// against `addressable_line_texts()` (diff prefix stripped) so a query hits the
/// same content regardless of whether the line was added, removed or context.
pub(crate) fn compute_search_matches(file: &crate::diff::DiffFile, query: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    file.addressable_line_texts()
        .iter()
        .enumerate()
        .filter(|(_, text)| text.to_lowercase().contains(&needle))
        .map(|(idx, _)| idx)
        .collect()
}
