use anyhow::Result;

use super::*;

impl App {
    pub fn preferred_diff_viewer_layout(&self) -> DiffViewerLayout {
        self.config.diff_viewer_layout.clone()
    }

    pub fn toggled_diff_viewer_layout(&self, layout: &DiffViewerLayout) -> DiffViewerLayout {
        match layout {
            DiffViewerLayout::Unified => DiffViewerLayout::SideBySide,
            DiffViewerLayout::SideBySide => DiffViewerLayout::Unified,
        }
    }

    pub fn persist_diff_viewer_layout(&mut self, layout: DiffViewerLayout) {
        self.config.diff_viewer_layout = layout;
        self.save_config();
    }

    pub fn open_diff_viewer(&mut self) -> Result<()> {
        let Some((view, workdir)) = self.current_view_and_workdir() else {
            self.message = Some("No active feature diff available".to_string());
            return Ok(());
        };

        let (commits, error) = match crate::diff::list_diff_commits(&workdir) {
            Ok(commits) => (commits, None),
            Err(err) => (Vec::new(), Some(err.to_string())),
        };
        self.mode = AppMode::DiffPicker(DiffPickerState {
            from_view: view,
            workdir,
            commits,
            selected: 0,
            error,
        });
        Ok(())
    }

    pub fn close_diff_picker(&mut self) {
        let view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffPicker(state) => state.from_view,
            other => {
                self.mode = other;
                return;
            }
        };
        self.mode = AppMode::Viewing(view);
    }

    pub fn diff_picker_select_next(&mut self) {
        if let AppMode::DiffPicker(state) = &mut self.mode {
            let entry_count = state.commits.len() + 1;
            state.selected = (state.selected + 1) % entry_count;
        }
    }

    pub fn diff_picker_select_prev(&mut self) {
        if let AppMode::DiffPicker(state) = &mut self.mode {
            let entry_count = state.commits.len() + 1;
            state.selected = if state.selected == 0 {
                entry_count - 1
            } else {
                state.selected - 1
            };
        }
    }

    pub fn diff_picker_choose(&mut self) {
        let picker = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffPicker(state) => state,
            other => {
                self.mode = other;
                return;
            }
        };

        let scope = if picker.selected == 0 {
            DiffScope::CurrentChanges
        } else {
            match picker.commits.get(picker.selected - 1).cloned() {
                Some(commit) => DiffScope::Commit(commit),
                None => DiffScope::CurrentChanges,
            }
        };
        let mut state = DiffViewerState::new(picker.from_view, picker.workdir);
        state.scope = scope;
        state.layout = self.preferred_diff_viewer_layout();
        self.mode = AppMode::DiffViewerLoading(state);
    }

    pub fn close_diff_viewer(&mut self) {
        let view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffViewer(state) | AppMode::DiffViewerLoading(state) => state.from_view,
            other => {
                self.mode = other;
                return;
            }
        };
        self.mode = AppMode::Viewing(view);
    }

    pub fn refresh_diff_viewer(&mut self) {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffViewer(mut state) => {
                state.error = None;
                state
            }
            other => {
                self.mode = other;
                return;
            }
        };
        self.mode = AppMode::DiffViewerLoading(state);
    }

    pub fn complete_diff_viewer_loading(&mut self) {
        let mut state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffViewerLoading(state) => state,
            other => {
                self.mode = other;
                return;
            }
        };

        let selected_path = state
            .files
            .get(state.selected_file)
            .map(|file| file.path.clone());
        let selected_index = state.selected_file;
        let snapshot = match &state.scope {
            DiffScope::CurrentChanges => {
                crate::diff::load_snapshot(&state.workdir, state.override_base_ref.as_deref())
            }
            DiffScope::Commit(commit) => {
                crate::diff::load_commit_snapshot(&state.workdir, &commit.hash)
            }
        };
        match snapshot {
            Ok(snapshot) => {
                state.branch = snapshot.branch;
                state.base_ref = snapshot.base_ref;
                state.base_commit = snapshot.base_commit;
                state.error = None;
                state.files = snapshot.files;
                state.selected_file = selected_path
                    .and_then(|path| state.files.iter().position(|file| file.path == path))
                    .unwrap_or_else(|| selected_index.min(state.files.len().saturating_sub(1)));
                state.patch_scroll = 0;
                if state.review {
                    state.review_notes = crate::app::review::load_review_notes(&state.workdir);
                }
            }
            Err(err) => {
                state.branch.clear();
                state.base_ref.clear();
                state.base_commit.clear();
                state.files.clear();
                state.selected_file = 0;
                state.patch_scroll = 0;
                state.error = Some(err.to_string());
            }
        }
        let was_review = state.review;
        self.mode = AppMode::DiffViewer(state);
        if was_review {
            self.restore_review_progress();
            // The diff may have moved underneath existing comments (a refresh
            // after the agent edited code, or a base-ref change) — re-locate
            // them before anything else reads their anchors.
            self.reanchor_line_comments();
            self.apply_review_snapshot_diff();
            self.load_prior_agent_responses();
        }
    }

    pub fn diff_viewer_loading(&self) -> bool {
        matches!(self.mode, AppMode::DiffViewerLoading(_))
    }

    /// Open the base-ref prompt, pre-filling it with the active override (or the
    /// currently resolved base) so the reviewer can edit rather than retype.
    pub fn diff_viewer_start_base_ref_edit(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && matches!(&state.scope, DiffScope::CurrentChanges)
        {
            state.base_ref_input = state
                .override_base_ref
                .clone()
                .unwrap_or_else(|| state.base_ref.clone());
            state.editing_base_ref = true;
        }
    }

    pub fn diff_viewer_base_ref_input(&mut self, c: char) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && !c.is_control()
        {
            state.base_ref_input.push(c);
        }
    }

    pub fn diff_viewer_base_ref_backspace(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.base_ref_input.pop();
        }
    }

    pub fn diff_viewer_cancel_base_ref(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.editing_base_ref = false;
            state.base_ref_input.clear();
        }
    }

    /// Apply the typed base ref and reload the diff against it. A blank entry
    /// clears the override and reverts to auto-resolution.
    pub fn diff_viewer_submit_base_ref(&mut self) {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffViewer(mut state) => {
                let trimmed = state.base_ref_input.trim().to_string();
                state.override_base_ref = (!trimmed.is_empty()).then_some(trimmed);
                state.editing_base_ref = false;
                state.base_ref_input.clear();
                state.error = None;
                state
            }
            other => {
                self.mode = other;
                return;
            }
        };
        self.mode = AppMode::DiffViewerLoading(state);
    }

    pub fn diff_viewer_select_next_file(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let visible = state.visible_file_indices();
            if visible.is_empty() {
                return;
            }
            // Move to the next visible file. If the current selection is itself
            // hidden (e.g. just decided under a filter), fall to the first
            // visible file at-or-after it, then the first overall.
            let next = match visible.iter().position(|&i| i == state.selected_file) {
                Some(pos) if pos + 1 < visible.len() => Some(visible[pos + 1]),
                Some(_) => None,
                None => visible
                    .iter()
                    .copied()
                    .find(|&i| i > state.selected_file)
                    .or_else(|| visible.first().copied()),
            };
            if let Some(idx) = next {
                state.selected_file = idx;
                state.on_file_changed();
            }
        }
    }

    pub fn diff_viewer_select_prev_file(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let visible = state.visible_file_indices();
            if visible.is_empty() {
                return;
            }
            let prev = match visible.iter().position(|&i| i == state.selected_file) {
                Some(pos) if pos > 0 => Some(visible[pos - 1]),
                Some(_) => None,
                None => visible
                    .iter()
                    .rev()
                    .copied()
                    .find(|&i| i < state.selected_file)
                    .or_else(|| visible.last().copied()),
            };
            if let Some(idx) = prev {
                state.selected_file = idx;
                state.on_file_changed();
            }
        }
    }

    /// Move the file-list cursor by `delta` *rows* of the directory tree —
    /// unlike `diff_viewer_select_next_file`, this walks onto directory rows
    /// too (`j`/`k`). Landing on a file selects it; landing on a directory
    /// parks the cursor there and leaves the selected file (and the patch
    /// panel) untouched.
    pub fn diff_viewer_tree_move(&mut self, delta: isize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let rows = state.file_tree_rows();
            if rows.is_empty() {
                return;
            }
            let current = state.tree_cursor_row(&rows).unwrap_or(0) as isize;
            let target = (current + delta).clamp(0, rows.len() as isize - 1) as usize;
            if target == current as usize {
                return;
            }
            match &rows[target] {
                FileTreeRow::Dir { path, .. } => {
                    state.tree_cursor_dir = Some(path.clone());
                }
                FileTreeRow::File { index, .. } => {
                    state.selected_file = *index;
                    state.on_file_changed();
                }
            }
        }
    }

    /// Collapse / expand the directory under the file-list cursor (`z` or
    /// Enter). With the cursor on a file, folds that file's own directory and
    /// parks the cursor on it — so `z` always does something useful.
    pub fn diff_viewer_tree_toggle_collapsed(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let dir = match &state.tree_cursor_dir {
                Some(dir) => Some(dir.clone()),
                None => state
                    .files
                    .get(state.selected_file)
                    .and_then(|file| crate::app::ancestor_dirs(&file.path).pop()),
            };
            let Some(dir) = dir else {
                self.message = Some("No directory to collapse".to_string());
                return;
            };
            state.toggle_dir_collapsed(&dir);
            state.tree_cursor_dir = Some(dir);
        }
    }

    /// `h`: collapse the cursored directory, or — when it is already collapsed
    /// (or the cursor is on a file) — step out to the parent directory row.
    pub fn diff_viewer_tree_collapse_or_parent(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if let Some(dir) = state.tree_cursor_dir.clone() {
                if !state.collapsed_dirs.contains(&dir) {
                    state.collapsed_dirs.insert(dir);
                    return;
                }
                // Already folded: move up a level, if there is one.
                if let Some(pos) = dir.rfind('/') {
                    state.tree_cursor_dir = Some(dir[..pos].to_string());
                }
                return;
            }
            // On a file: step to its directory row rather than folding straight
            // away, mirroring how a file explorer's left arrow behaves.
            if let Some(parent) = state
                .files
                .get(state.selected_file)
                .and_then(|file| crate::app::ancestor_dirs(&file.path).pop())
            {
                state.tree_cursor_dir = Some(parent);
            }
        }
    }

    /// `l`: expand the cursored directory, or step into its first row when it
    /// is already open.
    pub fn diff_viewer_tree_expand(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let Some(dir) = state.tree_cursor_dir.clone() else {
                return;
            };
            if state.collapsed_dirs.remove(&dir) {
                return;
            }
            let rows = state.file_tree_rows();
            let Some(pos) = state.tree_cursor_row(&rows) else {
                return;
            };
            match rows.get(pos + 1) {
                Some(FileTreeRow::Dir { path, .. }) => state.tree_cursor_dir = Some(path.clone()),
                Some(FileTreeRow::File { index, .. }) => {
                    state.selected_file = *index;
                    state.on_file_changed();
                }
                None => {}
            }
        }
    }

    /// `Z`: fold the whole tree to its top-level directories, or unfold it
    /// completely when nothing is currently collapsed.
    pub fn diff_viewer_tree_toggle_all(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if state.collapsed_dirs.is_empty() {
                let dirs = state.tree_dirs();
                if dirs.is_empty() {
                    self.message = Some("No directories to collapse".to_string());
                    return;
                }
                state.collapsed_dirs.extend(dirs);
                // The selected file is now folded away; park the cursor on the
                // outermost directory holding it so the highlight stays put.
                state.tree_cursor_dir = state
                    .files
                    .get(state.selected_file)
                    .and_then(|file| crate::app::ancestor_dirs(&file.path).into_iter().next());
            } else {
                state.collapsed_dirs.clear();
            }
        }
    }

    pub fn diff_viewer_toggle_focus(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.focus = match state.focus {
                DiffViewerFocus::FileList => DiffViewerFocus::Patch,
                DiffViewerFocus::Patch => DiffViewerFocus::FileList,
            };
        }
    }

    pub fn diff_viewer_scroll_patch_up(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.patch_scroll = state.patch_scroll.saturating_sub(amount);
        }
    }

    pub fn diff_viewer_scroll_patch_down(&mut self, amount: usize) {
        let max_scroll = self.diff_viewer_patch_line_count().saturating_sub(1);
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.patch_scroll = (state.patch_scroll + amount).min(max_scroll);
        }
    }

    pub fn diff_viewer_scroll_patch_top(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.patch_scroll = 0;
        }
    }

    pub fn diff_viewer_scroll_patch_bottom(&mut self) {
        let max_scroll = self.diff_viewer_patch_line_count().saturating_sub(1);
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.patch_scroll = max_scroll;
        }
    }

    pub fn diff_review_scroll_patch_up(&mut self, amount: usize) {
        if let AppMode::DiffReviewPrompt(state) = &mut self.mode {
            state.patch_scroll = state.patch_scroll.saturating_sub(amount);
        }
    }

    pub fn diff_review_scroll_patch_down(&mut self, amount: usize) {
        let max_scroll = self.diff_review_patch_line_count().saturating_sub(1);
        if let AppMode::DiffReviewPrompt(state) = &mut self.mode {
            state.patch_scroll = (state.patch_scroll + amount).min(max_scroll);
        }
    }

    pub fn diff_review_scroll_patch_top(&mut self) {
        if let AppMode::DiffReviewPrompt(state) = &mut self.mode {
            state.patch_scroll = 0;
        }
    }

    pub fn diff_review_scroll_patch_bottom(&mut self) {
        let max_scroll = self.diff_review_patch_line_count().saturating_sub(1);
        if let AppMode::DiffReviewPrompt(state) = &mut self.mode {
            state.patch_scroll = max_scroll;
        }
    }

    pub fn diff_viewer_toggle_layout(&mut self) {
        if self.diff_viewer_selected_file_is_new() {
            self.message =
                Some("Side-by-side isn't available for a new/untracked file".to_string());
            return;
        }
        let next_layout = match &self.mode {
            AppMode::DiffViewer(state) => Some(self.toggled_diff_viewer_layout(&state.layout)),
            _ => None,
        };
        if let Some(layout) = next_layout {
            self.persist_diff_viewer_layout(layout.clone());
            if let AppMode::DiffViewer(state) = &mut self.mode {
                state.layout = layout;
                state.patch_scroll = 0;
            }
        }
    }

    pub fn diff_viewer_focus(&self) -> Option<DiffViewerFocus> {
        match &self.mode {
            AppMode::DiffViewer(state) => Some(state.focus.clone()),
            _ => None,
        }
    }

    pub fn diff_viewer_selected_file_is_new(&self) -> bool {
        match &self.mode {
            AppMode::DiffViewer(state) => state
                .files
                .get(state.selected_file)
                .map(is_new_diff_file)
                .unwrap_or(false),
            _ => false,
        }
    }

    pub fn diff_viewer_layout(&self) -> Option<DiffViewerLayout> {
        match &self.mode {
            AppMode::DiffViewer(state) => Some(if self.diff_viewer_selected_file_is_new() {
                DiffViewerLayout::Unified
            } else {
                state.layout.clone()
            }),
            _ => None,
        }
    }

    pub fn poll_diff_review_explanation(&mut self) -> Result<()> {
        let finished = match &mut self.mode {
            AppMode::DiffReviewPrompt(state) => match state.explanation_child.as_mut() {
                Some(child) => child.try_wait()?,
                None => return Ok(()),
            },
            _ => return Ok(()),
        };

        let Some(status) = finished else {
            return Ok(());
        };

        let child = match &mut self.mode {
            AppMode::DiffReviewPrompt(state) => state.explanation_child.take(),
            _ => None,
        };

        let Some(child) = child else {
            return Ok(());
        };

        let output = child.wait_with_output()?;
        let explanation = if status.success() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            format!("Explanation unavailable: {stderr}")
        };

        if let AppMode::DiffReviewPrompt(state) = &mut self.mode {
            state.explanation = Some(explanation);
        }

        Ok(())
    }

    fn current_view_and_workdir(&self) -> Option<(ViewState, std::path::PathBuf)> {
        let view = match &self.mode {
            AppMode::Viewing(view) => view.clone(),
            _ => return None,
        };

        let workdir = self
            .store
            .projects
            .iter()
            .find(|project| project.name == view.project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.name == view.feature_name)
            })
            .map(|feature| feature.workdir.clone())?;

        Some((view, workdir))
    }

    fn diff_viewer_patch_line_count(&self) -> usize {
        match &self.mode {
            AppMode::DiffViewer(state) => state
                .files
                .get(state.selected_file)
                .map(|file| {
                    diff_patch_line_count(
                        file,
                        self.diff_viewer_layout()
                            .unwrap_or_else(|| state.layout.clone()),
                    )
                })
                .unwrap_or(0),
            _ => 0,
        }
    }

    fn diff_review_patch_line_count(&self) -> usize {
        match &self.mode {
            AppMode::DiffReviewPrompt(state) => state
                .diff_file
                .as_ref()
                .map(|file| {
                    let layout = if is_new_diff_file(file) {
                        DiffViewerLayout::Unified
                    } else {
                        state.layout.clone()
                    };
                    diff_patch_line_count(file, layout)
                })
                .unwrap_or(0),
            _ => 0,
        }
    }
}

fn is_new_diff_file(file: &crate::diff::DiffFile) -> bool {
    matches!(
        file.status,
        crate::diff::DiffFileStatus::Added | crate::diff::DiffFileStatus::Untracked
    )
}

fn side_by_side_line_count(file: &crate::diff::DiffFile) -> usize {
    if file.is_binary || file.hunks.is_empty() {
        return file.patch.lines().count();
    }

    let mut count = 1usize;
    for hunk in &file.hunks {
        count += 1;
        let mut index = 0usize;
        while index < hunk.lines.len() {
            match hunk.lines[index].kind {
                crate::diff::DiffLineKind::Context => {
                    count += 1;
                    index += 1;
                }
                crate::diff::DiffLineKind::Removed => {
                    let removed =
                        consume_kind(hunk, &mut index, crate::diff::DiffLineKind::Removed);
                    let added = consume_kind(hunk, &mut index, crate::diff::DiffLineKind::Added);
                    count += removed.max(added);
                }
                crate::diff::DiffLineKind::Added => {
                    count += consume_kind(hunk, &mut index, crate::diff::DiffLineKind::Added);
                }
                crate::diff::DiffLineKind::NoNewlineMarker => {
                    count += 1;
                    index += 1;
                }
            }
        }
    }
    count
}

fn diff_patch_line_count(file: &crate::diff::DiffFile, layout: DiffViewerLayout) -> usize {
    match layout {
        DiffViewerLayout::Unified => file.patch.lines().count(),
        DiffViewerLayout::SideBySide => side_by_side_line_count(file),
    }
}

fn consume_kind(
    hunk: &crate::diff::DiffHunk,
    index: &mut usize,
    kind: crate::diff::DiffLineKind,
) -> usize {
    let mut count = 0usize;
    while *index < hunk.lines.len() && hunk.lines[*index].kind == kind {
        *index += 1;
        count += 1;
    }
    count
}
