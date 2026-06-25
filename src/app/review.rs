use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::*;

/// The resumable parts of an in-flight final review, persisted to
/// `.claude/final-review-progress.json` so a long review can be paused
/// (or survive an AMF quit / crash) and picked up where it left off.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ReviewProgress {
    #[serde(default)]
    decisions: std::collections::HashMap<String, ReviewDecision>,
    #[serde(default)]
    line_comments: std::collections::HashMap<String, Vec<LineComment>>,
    #[serde(default)]
    general_feedback: String,
    #[serde(default)]
    selected_file: usize,
}

/// Path of the saved review-progress file for a feature workdir.
fn review_progress_path(workdir: &Path) -> PathBuf {
    workdir.join(".claude").join("final-review-progress.json")
}

/// Best-effort load of any saved review progress for `workdir`.
fn load_review_progress(workdir: &Path) -> Option<ReviewProgress> {
    let content = std::fs::read_to_string(review_progress_path(workdir)).ok()?;
    serde_json::from_str(&content).ok()
}

impl App {
    /// Open AMF's native diff viewer in final-review mode: walk every file
    /// changed since the base ref, approving / rejecting / skipping each, then
    /// write `.claude/final-review-feedback.md` for any rejected files. This
    /// replaces the old tmux-session + vimdiff-popup script, which could not
    /// work against AMF's bundled control-mode tmux server.
    pub fn trigger_final_review(&mut self) -> Result<()> {
        let view = match &self.mode {
            AppMode::Viewing(view) => view.clone(),
            _ => return Ok(()),
        };

        let workdir = self
            .store
            .projects
            .iter()
            .find(|p| p.name == view.project_name)
            .and_then(|p| p.features.iter().find(|f| f.name == view.feature_name))
            .map(|f| f.workdir.clone());

        let Some(workdir) = workdir else {
            self.message = Some("No active feature to review".to_string());
            return Ok(());
        };

        let mut state = DiffViewerState::new(view, workdir);
        state.layout = self.preferred_diff_viewer_layout();
        state.review = true;
        self.mode = AppMode::DiffViewerLoading(state);
        Ok(())
    }

    /// Write the current review's decisions, line comments, general feedback
    /// and file position to `.claude/final-review-progress.json`. A no-op when
    /// not in a final review. Called after each state-changing review action so
    /// progress is never lost — the only exit from the review viewer finishes
    /// it, but an AMF quit/crash mid-review would otherwise discard everything.
    pub fn persist_review_progress(&mut self) {
        let AppMode::DiffViewer(state) = &self.mode else {
            return;
        };
        if !state.review {
            return;
        }
        let progress = ReviewProgress {
            decisions: state.decisions.clone(),
            line_comments: state.line_comments.clone(),
            general_feedback: state.general_feedback.clone(),
            selected_file: state.selected_file,
        };
        let path = review_progress_path(&state.workdir);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&progress) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&path, json) {
                    self.log_warn(
                        "review",
                        format!("failed to persist review progress: {err}"),
                    );
                }
            }
            Err(err) => self.log_warn(
                "review",
                format!("failed to serialize review progress: {err}"),
            ),
        }
    }

    /// Remove any saved review progress for `workdir`. Called once a review
    /// finishes so the next review for the feature starts fresh.
    fn clear_review_progress(workdir: &Path) {
        let _ = std::fs::remove_file(review_progress_path(workdir));
    }

    /// On opening a fresh final review, restore any previously saved decisions /
    /// comments / general feedback for the feature. Skipped when the in-memory
    /// review state already holds verdicts (e.g. an in-review `r` refresh), so a
    /// reload never clobbers work in progress. Stale entries for paths no longer
    /// in the diff are dropped and the file position is clamped.
    pub fn restore_review_progress(&mut self) {
        let AppMode::DiffViewer(state) = &mut self.mode else {
            return;
        };
        if !state.review
            || !state.decisions.is_empty()
            || !state.line_comments.is_empty()
            || !state.general_feedback.is_empty()
        {
            return;
        }
        let Some(progress) = load_review_progress(&state.workdir) else {
            return;
        };
        let known: std::collections::HashSet<&str> =
            state.files.iter().map(|f| f.path.as_str()).collect();
        state.decisions = progress
            .decisions
            .into_iter()
            .filter(|(path, _)| known.contains(path.as_str()))
            .collect();
        state.line_comments = progress
            .line_comments
            .into_iter()
            .filter(|(path, _)| known.contains(path.as_str()))
            .collect();
        state.general_feedback = progress.general_feedback;
        if !state.files.is_empty() {
            state.selected_file = progress.selected_file.min(state.files.len() - 1);
        }
    }

    /// Approve the file currently selected in the review viewer and advance.
    pub fn diff_review_approve_current(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            if let Some(file) = state.files.get(state.selected_file) {
                state
                    .decisions
                    .insert(file.path.clone(), ReviewDecision::Approve);
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
            if let Some(file) = state.files.get(state.selected_file) {
                state.decisions.remove(&file.path);
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

    /// Open the comment editor for the cursored diff line, pre-filling any
    /// comment already attached to it.
    pub fn diff_review_start_line_comment(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let Some(cur) = state.comment_cursor else {
                return;
            };
            let Some(file) = state.files.get(state.selected_file) else {
                return;
            };
            let locs = file.addressable_lines();
            let Some(loc) = locs.get(cur).copied() else {
                return;
            };
            let path = file.path.clone();
            let existing = state
                .line_comments
                .get(&path)
                .and_then(|comments| comments.iter().find(|c| c.location == loc))
                .map(|c| c.text.clone())
                .unwrap_or_default();
            state.feedback_editor = crate::editor::TextEditor::new(existing);
            state.feedback_scroll = 0;
            state.feedback_sync_to_cursor = true;
            state.editing_line_comment = true;
            state.feedback_editing = false;
            state.editing_general = false;
        }
    }

    /// Store (or, when empty, delete) the typed comment for the cursored line.
    /// Does not advance the cursor, so a reviewer can comment several adjacent
    /// lines.
    pub fn diff_review_submit_line_comment(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.editing_line_comment {
                return;
            }
            let text = state.feedback_editor.text().trim().to_string();
            let anchor = state.comment_cursor.and_then(|cur| {
                state
                    .files
                    .get(state.selected_file)
                    .and_then(|file| {
                        file.addressable_lines()
                            .get(cur)
                            .copied()
                            .map(|loc| (file.path.clone(), loc))
                    })
            });
            if let Some((path, loc)) = anchor {
                let comments = state.line_comments.entry(path).or_default();
                comments.retain(|c| c.location != loc);
                if !text.is_empty() {
                    comments.push(LineComment {
                        location: loc,
                        text,
                    });
                    comments.sort_by_key(|c| {
                        c.location
                            .new_line
                            .or(c.location.old_line)
                            .unwrap_or(0)
                    });
                }
            }
            state.editing_line_comment = false;
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
        }
        self.persist_review_progress();
    }

    /// Generate a walkthrough for the current file when it has no developer
    /// note. Spawns a headless Claude explanation of the file's diff; the result
    /// is collected by `poll_review_walkthrough` and cached in `generated_notes`
    /// so the developer-notes panel is never empty.
    pub fn generate_review_walkthrough(&mut self) {
        let (workdir, path, prompt) = {
            let AppMode::DiffViewer(state) = &self.mode else {
                return;
            };
            if !state.review || state.walkthrough_child.is_some() {
                return;
            }
            let Some(file) = state.files.get(state.selected_file) else {
                return;
            };
            let path = file.path.clone();
            // A developer note or an already-generated walkthrough makes this a
            // no-op.
            if state.review_notes.contains_key(&path)
                || state.generated_notes.contains_key(&path)
            {
                return;
            }
            if file.is_binary {
                let path2 = path.clone();
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state
                        .generated_notes
                        .insert(path2, "Binary file — no walkthrough available.".to_string());
                }
                let _ = path;
                return;
            }
            (state.workdir.clone(), path, build_walkthrough_prompt(file))
        };

        match crate::claude::ClaudeLauncher::spawn_headless(&workdir, &prompt) {
            Ok(child) => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.walkthrough_child = Some(child);
                    state.walkthrough_file = Some(path);
                }
            }
            Err(err) => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state
                        .generated_notes
                        .insert(path, format!("Walkthrough unavailable: {err}"));
                }
            }
        }
    }

    /// Poll an in-flight walkthrough generation; on completion cache the output
    /// under the file it was generated for. Mirrors
    /// `poll_diff_review_explanation`.
    pub fn poll_review_walkthrough(&mut self) -> Result<()> {
        let finished = match &mut self.mode {
            AppMode::DiffViewer(state) => match state.walkthrough_child.as_mut() {
                Some(child) => child.try_wait()?,
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        let Some(status) = finished else {
            return Ok(());
        };

        let (child, path) = match &mut self.mode {
            AppMode::DiffViewer(state) => {
                (state.walkthrough_child.take(), state.walkthrough_file.take())
            }
            _ => (None, None),
        };
        let (Some(child), Some(path)) = (child, path) else {
            return Ok(());
        };

        let output = child.wait_with_output()?;
        let note = if status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if text.is_empty() {
                "Walkthrough was empty.".to_string()
            } else {
                text
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            format!("Walkthrough unavailable: {stderr}")
        };
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.generated_notes.insert(path, note);
        }
        Ok(())
    }

    /// Begin entering rejection feedback for the current file, pre-filling any
    /// feedback already recorded for it.
    pub fn diff_review_start_feedback(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let existing = state
                .files
                .get(state.selected_file)
                .and_then(|f| state.decisions.get(&f.path))
                .and_then(|d| match d {
                    ReviewDecision::Reject { feedback } => Some(feedback.clone()),
                    ReviewDecision::Approve => None,
                })
                .unwrap_or_default();
            state.feedback_editor = crate::editor::TextEditor::new(existing);
            state.feedback_scroll = 0;
            state.feedback_sync_to_cursor = true;
            state.feedback_editing = true;
        }
    }

    /// Begin entering general (non-file) review feedback, pre-filling any note
    /// already recorded.
    pub fn diff_review_start_general_feedback(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            state.feedback_editor =
                crate::editor::TextEditor::new(state.general_feedback.clone());
            state.feedback_scroll = 0;
            state.feedback_sync_to_cursor = true;
            state.editing_general = true;
            state.feedback_editing = false;
        }
    }

    /// Store the typed general feedback. Unlike per-file rejection this does not
    /// advance the file selection.
    pub fn diff_review_submit_general_feedback(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.editing_general {
                return;
            }
            state.general_feedback = state.feedback_editor.text().trim().to_string();
            state.editing_general = false;
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
        }
        self.persist_review_progress();
    }

    pub fn diff_review_cancel_feedback(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.feedback_editing = false;
            state.editing_general = false;
            state.editing_line_comment = false;
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
        }
    }

    /// Record the typed feedback as a rejection for the current file, then
    /// advance to the next file.
    pub fn diff_review_submit_feedback(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.feedback_editing {
                return;
            }
            let feedback = state.feedback_editor.text().trim().to_string();
            if let Some(file) = state.files.get(state.selected_file) {
                state
                    .decisions
                    .insert(file.path.clone(), ReviewDecision::Reject { feedback });
            }
            state.feedback_editing = false;
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
        }
        self.diff_review_advance();
        self.persist_review_progress();
    }

    fn diff_review_advance(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && state.selected_file + 1 < state.files.len()
        {
            state.selected_file += 1;
            state.patch_scroll = 0;
            state.notes_scroll = 0;
            if state.comment_cursor.is_some() {
                state.comment_cursor = Some(0);
                state.cursor_sync_to_view = true;
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
    fn review_note_max_scroll(state: &DiffViewerState) -> usize {
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
    fn diff_review_undecided_count(state: &DiffViewerState) -> usize {
        state
            .files
            .iter()
            .filter(|file| !state.decisions.contains_key(&file.path))
            .count()
    }

    /// Finish the review, but if some files still have no verdict, gate the
    /// finish behind a confirmation rather than ending silently. A second
    /// confirm (handled in the key layer) calls `finish_final_review` directly.
    pub fn confirm_or_finish_review(&mut self) -> Result<()> {
        let undecided = match &self.mode {
            AppMode::DiffViewer(state) if state.review => Self::diff_review_undecided_count(state),
            _ => return self.finish_final_review(),
        };
        if undecided == 0 {
            return self.finish_final_review();
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
                    state.patch_scroll = 0;
                    state.notes_scroll = 0;
                    if state.comment_cursor.is_some() {
                        state.comment_cursor = Some(0);
                        state.cursor_sync_to_view = true;
                    }
                }
            }
            None => self.message = Some("All files have a verdict".to_string()),
        }
    }

    /// Finish the review: write `.claude/final-review-feedback.md` for any
    /// rejected files and return to the feature view with a summary message.
    pub fn finish_final_review(&mut self) -> Result<()> {
        let (workdir, files, decisions, line_comments, general_feedback, from_view) =
            match std::mem::replace(&mut self.mode, AppMode::Normal) {
                AppMode::DiffViewer(state) => (
                    state.workdir,
                    state.files,
                    state.decisions,
                    state.line_comments,
                    state.general_feedback,
                    state.from_view,
                ),
                AppMode::DiffViewerLoading(state) => {
                    // Diff not loaded yet; nothing to summarize.
                    self.mode = AppMode::Viewing(state.from_view);
                    return Ok(());
                }
                other => {
                    self.mode = other;
                    return Ok(());
                }
            };

        // The review is over; drop any saved progress so the next review for
        // this feature starts clean.
        Self::clear_review_progress(&workdir);

        let total = files.len();
        let mut approved = 0usize;
        let mut rejected: Vec<(String, String)> = Vec::new();
        for file in &files {
            match decisions.get(&file.path) {
                Some(ReviewDecision::Approve) => approved += 1,
                Some(ReviewDecision::Reject { feedback }) => {
                    rejected.push((file.path.clone(), feedback.clone()));
                }
                None => {}
            }
        }
        let skipped = total.saturating_sub(approved).saturating_sub(rejected.len());
        let general_feedback = general_feedback.trim().to_string();

        // Line comments in file order (each file's comments are already sorted
        // by line). Empty-text comments never reach here (submit deletes them).
        let mut line_comment_sections: Vec<(String, Vec<LineComment>)> = Vec::new();
        let mut line_comment_count = 0usize;
        for file in &files {
            if let Some(comments) = line_comments.get(&file.path)
                && !comments.is_empty()
            {
                line_comment_count += comments.len();
                line_comment_sections.push((file.path.clone(), comments.clone()));
            }
        }

        // The feature's agent session/window, so we can prompt it to act on the
        // feedback. Resolved before touching self.tmux to avoid borrow overlap.
        let agent_target: Option<(String, String)> = self
            .store
            .projects
            .iter()
            .find(|p| p.name == from_view.project_name)
            .and_then(|p| p.features.iter().find(|f| f.name == from_view.feature_name))
            .and_then(|f| {
                f.sessions
                    .iter()
                    .find(|s| {
                        matches!(
                            s.kind,
                            crate::project::SessionKind::Claude
                                | crate::project::SessionKind::Opencode
                                | crate::project::SessionKind::Codex
                        )
                    })
                    .map(|s| (f.tmux_session.clone(), s.tmux_window.clone()))
            });

        if rejected.is_empty() && general_feedback.is_empty() && line_comment_sections.is_empty() {
            self.message = Some(if total == 0 {
                "Final review: no changes against the base branch".to_string()
            } else {
                format!(
                    "Final review complete: all {approved} reviewed file(s) approved{}",
                    if skipped > 0 {
                        format!(", {skipped} skipped")
                    } else {
                        String::new()
                    }
                )
            });
        } else {
            let path = workdir.join(".claude").join("final-review-feedback.md");
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            // Build this round as a self-contained section. Rounds are
            // prepended under a single title (see `compose_feedback_log`) so
            // every review is preserved as a trail rather than overwritten.
            let mut round = String::new();
            round.push_str(&format!(
                "## Review — {}\n\n",
                chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
            ));
            round.push_str(&format!(
                "**Files reviewed:** {total} | **Approved:** {approved} | \
                 **Needs work:** {} | **Skipped:** {skipped} | \
                 **Line comments:** {line_comment_count}\n\n",
                rejected.len()
            ));

            if !general_feedback.is_empty() {
                round.push_str("### General Feedback\n\n");
                round.push_str(&general_feedback);
                round.push_str("\n\n");
            }

            if !rejected.is_empty() {
                round.push_str("### Files Needing Revision\n\n");
                for (file, feedback) in &rejected {
                    round.push_str(&format!("#### {file}\n\n"));
                    if feedback.is_empty() {
                        round.push_str("(No feedback provided — needs revision)\n\n");
                    } else {
                        round.push_str(feedback);
                        round.push_str("\n\n");
                    }
                }
            }

            if !line_comment_sections.is_empty() {
                round.push_str("### Line Comments\n\n");
                for (file, comments) in &line_comment_sections {
                    for comment in comments {
                        let anchor = match (comment.location.new_line, comment.location.old_line) {
                            (Some(new_line), _) => format!("{file}:{new_line}"),
                            (None, Some(old_line)) => format!("{file}:{old_line} (base)"),
                            (None, None) => file.clone(),
                        };
                        round.push_str(&format!("#### {anchor}\n\n"));
                        round.push_str(&comment.text);
                        round.push_str("\n\n");
                    }
                }
            }

            let out = compose_feedback_log(std::fs::read_to_string(&path).ok().as_deref(), &round);

            self.message = Some(match std::fs::write(&path, out) {
                Ok(()) => {
                    let comment_note = if line_comment_count > 0 {
                        format!(", {line_comment_count} line comment(s)")
                    } else {
                        String::new()
                    };
                    let summary = format!(
                        "Final review: {approved} approved, {} need work, {skipped} skipped\
                         {comment_note} — feedback saved to .claude/final-review-feedback.md",
                        rejected.len()
                    );
                    match &agent_target {
                        Some((session, window)) => {
                            let prompt = "A reviewer left feedback on these changes in \
                                 .claude/final-review-feedback.md. Read that file and address \
                                 every item in the most recent review round (the first \
                                 \"## Review\" section); earlier sections are prior rounds kept \
                                 for history.";
                            let submit = self.config.final_review_submit_prompt;
                            let pasted = self.tmux.paste_text(session, window, prompt).and_then(
                                |()| {
                                    if submit {
                                        self.tmux.send_key_name(session, window, "Enter")
                                    } else {
                                        Ok(())
                                    }
                                },
                            );
                            match pasted {
                                Ok(()) if submit => format!("{summary} — sent to agent"),
                                Ok(()) => format!("{summary} — pasted to agent (not submitted)"),
                                Err(e) => format!("{summary} (couldn't prompt agent: {e})"),
                            }
                        }
                        None => summary,
                    }
                }
                Err(e) => format!("Final review: failed to write feedback file: {e}"),
            });
        }

        self.mode = AppMode::Viewing(from_view);
        Ok(())
    }
}

/// Document title that heads the feedback log. Each review round is prepended
/// directly under it.
const FEEDBACK_TITLE: &str = "# Final Review Feedback\n\n";

/// Prepend a freshly-built review round to the existing feedback log so every
/// round is preserved as a trail rather than overwritten. `existing` is the
/// prior file content (if any); `round` is the new round's body (starting at
/// its `## Review …` heading and ending with a blank line). The newest round
/// lands directly under the single title, with prior rounds following.
fn compose_feedback_log(existing: Option<&str>, round: &str) -> String {
    let prior = existing
        .map(|c| c.strip_prefix(FEEDBACK_TITLE).unwrap_or(c).trim_start())
        .filter(|p| !p.is_empty());
    let mut out = String::from(FEEDBACK_TITLE);
    out.push_str(round);
    if let Some(prior) = prior {
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(prior);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Build the prompt for an on-demand walkthrough of a file's diff. Large
/// patches are truncated to keep the headless request bounded.
fn build_walkthrough_prompt(file: &crate::diff::DiffFile) -> String {
    const MAX_PATCH: usize = 8000;
    let mut patch = if file.patch.trim().is_empty() {
        file.new_content.clone().unwrap_or_default()
    } else {
        file.patch.clone()
    };
    if patch.len() > MAX_PATCH {
        patch.truncate(MAX_PATCH);
        patch.push_str("\n… (diff truncated)");
    }
    format!(
        "You are helping a reviewer understand a code change during final \
         review. Concisely explain what this diff does and why it likely \
         matters. Answer in short markdown: a sentence or two of summary, then \
         a few bullet points for the notable changes. Do not restate the diff \
         line by line.\n\nFile: {}\n\n```diff\n{}\n```",
        file.path, patch
    )
}

/// Parse `.claude/review-notes.md` into a map of file path -> note body.
///
/// Review mode writes one section per changed file, headed either `## <path> —
/// <title>` (the documented format) or grouped under `### <path> — <title>`.
/// The path is the heading text up to the first ` — ` / ` - ` separator. A
/// section ends at the next heading or a `---` rule.
pub(crate) fn parse_review_notes(content: &str) -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;

    fn flush(current: &mut Option<(String, String)>, map: &mut HashMap<String, String>) {
        if let Some((path, body)) = current.take() {
            let body = body.trim().to_string();
            if !path.is_empty() && !body.is_empty() {
                map.insert(path, body);
            }
        }
    }

    let mut map: HashMap<String, String> = HashMap::new();
    let mut current: Option<(String, String)> = None;

    for line in content.lines() {
        if let Some(heading) = line
            .strip_prefix("### ")
            .or_else(|| line.strip_prefix("## "))
        {
            flush(&mut current, &mut map);
            let heading = heading.trim();
            let path = heading
                .split(" — ")
                .next()
                .unwrap_or(heading)
                .split(" - ")
                .next()
                .unwrap_or(heading)
                .trim()
                .to_string();
            current = Some((path, String::new()));
            continue;
        }

        if line.trim() == "---" {
            flush(&mut current, &mut map);
            continue;
        }

        if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }

    flush(&mut current, &mut map);
    map
}

#[cfg(test)]
mod tests {
    use super::{build_walkthrough_prompt, compose_feedback_log, parse_review_notes};

    #[test]
    fn first_round_writes_title_then_round() {
        let round = "## Review — 2026-06-25T00:00:00Z\n\nbody.\n\n";
        let out = compose_feedback_log(None, round);
        assert_eq!(out, "# Final Review Feedback\n\n## Review — 2026-06-25T00:00:00Z\n\nbody.\n\n");
    }

    #[test]
    fn later_round_is_prepended_above_prior_rounds() {
        let existing = "# Final Review Feedback\n\n## Review — 2026-06-24T00:00:00Z\n\nold.\n\n";
        let round = "## Review — 2026-06-25T00:00:00Z\n\nnew.\n\n";
        let out = compose_feedback_log(Some(existing), round);
        // Single title, newest round first, prior round retained after it.
        assert_eq!(out.matches("# Final Review Feedback").count(), 1);
        let new_at = out.find("new.").unwrap();
        let old_at = out.find("old.").unwrap();
        assert!(new_at < old_at, "newest round should come first");
        assert!(out.contains("## Review — 2026-06-24T00:00:00Z"));
    }

    #[test]
    fn tolerates_prior_file_without_title() {
        // A legacy / hand-edited file that doesn't start with the title is kept
        // verbatim below the new round rather than dropped.
        let existing = "## Review — 2026-06-24T00:00:00Z\n\nold.\n";
        let out = compose_feedback_log(Some(existing), "## Review — x\n\nnew.\n\n");
        assert!(out.starts_with("# Final Review Feedback\n\n## Review — x"));
        assert!(out.contains("old."));
    }


    #[test]
    fn walkthrough_prompt_includes_path_and_patch() {
        let file = crate::diff::DiffFile {
            old_path: Some("a.rs".into()),
            path: "a.rs".into(),
            status: crate::diff::DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: "@@ -1 +1 @@\n-old line\n+new line".into(),
            hunks: vec![],
        };
        let prompt = build_walkthrough_prompt(&file);
        assert!(prompt.contains("File: a.rs"));
        assert!(prompt.contains("+new line"));
        assert!(prompt.contains("```diff"));
    }

    #[test]
    fn walkthrough_prompt_truncates_huge_patches() {
        let file = crate::diff::DiffFile {
            old_path: Some("big.rs".into()),
            path: "big.rs".into(),
            status: crate::diff::DiffFileStatus::Modified,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: "+x\n".repeat(10_000),
            hunks: vec![],
        };
        let prompt = build_walkthrough_prompt(&file);
        assert!(prompt.contains("(diff truncated)"));
    }

    #[test]
    fn parses_documented_and_grouped_note_formats() {
        let content = "\
## src/app/state.rs — add fields

Added the review fields.
Second line.

---

### src/handlers/diff.rs — wire keys

Wired the keys.

## Overview heading not a path

ignored body
";
        let notes = parse_review_notes(content);
        assert_eq!(
            notes.get("src/app/state.rs").map(String::as_str),
            Some("Added the review fields.\nSecond line.")
        );
        assert_eq!(
            notes.get("src/handlers/diff.rs").map(String::as_str),
            Some("Wired the keys.")
        );
        // The non-path overview heading is stored under its text but never
        // matches a real file path.
        assert!(!notes.contains_key("src/app/review.rs"));
    }

    #[test]
    fn bare_path_heading_without_title_is_parsed() {
        let notes = parse_review_notes("## src/main.rs\n\nDid a thing.\n");
        assert_eq!(
            notes.get("src/main.rs").map(String::as_str),
            Some("Did a thing.")
        );
    }
}
