use anyhow::Result;

use super::*;

impl App {
    pub fn open_diff_viewer(&mut self) -> Result<()> {
        let Some((view, workdir)) = self.current_view_and_workdir() else {
            self.message = Some("No active feature diff available".to_string());
            return Ok(());
        };

        let mut state = DiffViewerState::new(view, workdir);
        state.layout = self.config.diff_viewer_layout.clone();
        self.populate_diff_viewer_state(&mut state);
        self.mode = AppMode::DiffViewer(state);
        Ok(())
    }

    pub fn close_diff_viewer(&mut self) {
        let view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffViewer(state) => state.from_view,
            other => {
                self.mode = other;
                return;
            }
        };
        self.mode = AppMode::Viewing(view);
    }

    pub fn refresh_diff_viewer(&mut self) {
        let Some((workdir, selected_path, selected_index)) = (match &self.mode {
            AppMode::DiffViewer(state) => Some((
                state.workdir.clone(),
                state
                    .files
                    .get(state.selected_file)
                    .map(|file| file.path.clone()),
                state.selected_file,
            )),
            _ => None,
        }) else {
            return;
        };

        let snapshot = crate::diff::load_snapshot(&workdir);
        if let AppMode::DiffViewer(state) = &mut self.mode {
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
        }
    }

    pub fn diff_viewer_select_next_file(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && state.selected_file + 1 < state.files.len()
        {
            state.selected_file += 1;
            state.patch_scroll = 0;
        }
    }

    pub fn diff_viewer_select_prev_file(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && state.selected_file > 0
        {
            state.selected_file -= 1;
            state.patch_scroll = 0;
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

    pub fn diff_viewer_toggle_layout(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.layout = match state.layout {
                DiffViewerLayout::Unified => DiffViewerLayout::SideBySide,
                DiffViewerLayout::SideBySide => DiffViewerLayout::Unified,
            };
            self.config.diff_viewer_layout = state.layout.clone();
            state.patch_scroll = 0;
        }
        self.save_config();
    }

    pub fn diff_viewer_focus(&self) -> Option<DiffViewerFocus> {
        match &self.mode {
            AppMode::DiffViewer(state) => Some(state.focus.clone()),
            _ => None,
        }
    }

    pub fn diff_viewer_layout(&self) -> Option<DiffViewerLayout> {
        match &self.mode {
            AppMode::DiffViewer(state) => Some(state.layout.clone()),
            _ => None,
        }
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

    fn populate_diff_viewer_state(&self, state: &mut DiffViewerState) {
        match crate::diff::load_snapshot(&state.workdir) {
            Ok(snapshot) => {
                state.branch = snapshot.branch;
                state.base_ref = snapshot.base_ref;
                state.base_commit = snapshot.base_commit;
                state.files = snapshot.files;
                state.selected_file = 0;
                state.patch_scroll = 0;
                state.error = None;
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
    }

    fn diff_viewer_patch_line_count(&self) -> usize {
        match &self.mode {
            AppMode::DiffViewer(state) => state
                .files
                .get(state.selected_file)
                .map(|file| match state.layout {
                    DiffViewerLayout::Unified => file.patch.lines().count(),
                    DiffViewerLayout::SideBySide => side_by_side_line_count(file),
                })
                .unwrap_or(0),
            _ => 0,
        }
    }
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
