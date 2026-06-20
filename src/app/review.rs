use anyhow::Result;

use super::*;

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
            state.feedback_input = existing;
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
            state.feedback_input = state.general_feedback.clone();
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
            state.general_feedback = state.feedback_input.trim().to_string();
            state.editing_general = false;
            state.feedback_input.clear();
        }
    }

    pub fn diff_review_cancel_feedback(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.feedback_editing = false;
            state.editing_general = false;
            state.feedback_input.clear();
        }
    }

    pub fn diff_review_push_feedback_char(&mut self, c: char) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && (state.feedback_editing || state.editing_general)
            && state.feedback_input.len() < 2000
        {
            state.feedback_input.push(c);
        }
    }

    pub fn diff_review_pop_feedback_char(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && (state.feedback_editing || state.editing_general)
        {
            state.feedback_input.pop();
        }
    }

    /// Record the typed feedback as a rejection for the current file, then
    /// advance to the next file.
    pub fn diff_review_submit_feedback(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.feedback_editing {
                return;
            }
            let feedback = state.feedback_input.trim().to_string();
            if let Some(file) = state.files.get(state.selected_file) {
                state
                    .decisions
                    .insert(file.path.clone(), ReviewDecision::Reject { feedback });
            }
            state.feedback_editing = false;
            state.feedback_input.clear();
        }
        self.diff_review_advance();
    }

    fn diff_review_advance(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode
            && state.selected_file + 1 < state.files.len()
        {
            state.selected_file += 1;
            state.patch_scroll = 0;
            state.notes_scroll = 0;
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

    /// Max scroll offset for the current file's note (in raw lines).
    fn review_note_max_scroll(state: &DiffViewerState) -> usize {
        state
            .files
            .get(state.selected_file)
            .and_then(|f| state.review_notes.get(&f.path))
            .map(|note| note.lines().count().saturating_sub(1))
            .unwrap_or(0)
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

    /// Finish the review: write `.claude/final-review-feedback.md` for any
    /// rejected files and return to the feature view with a summary message.
    pub fn finish_final_review(&mut self) -> Result<()> {
        let (workdir, files, decisions, general_feedback, from_view) =
            match std::mem::replace(&mut self.mode, AppMode::Normal) {
                AppMode::DiffViewer(state) => (
                    state.workdir,
                    state.files,
                    state.decisions,
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

        if rejected.is_empty() && general_feedback.is_empty() {
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

            let mut out = String::new();
            out.push_str("# Final Review Feedback\n\n");
            out.push_str(&format!(
                "Reviewed: {}\n\n",
                chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
            ));
            out.push_str(&format!(
                "**Files reviewed:** {total} | **Approved:** {approved} | \
                 **Needs work:** {} | **Skipped:** {skipped}\n\n",
                rejected.len()
            ));

            if !general_feedback.is_empty() {
                out.push_str("## General Feedback\n\n");
                out.push_str(&general_feedback);
                out.push_str("\n\n");
            }

            if !rejected.is_empty() {
                out.push_str("## Files Needing Revision\n\n");
                for (file, feedback) in &rejected {
                    out.push_str(&format!("### {file}\n\n"));
                    if feedback.is_empty() {
                        out.push_str("(No feedback provided — needs revision)\n\n");
                    } else {
                        out.push_str(feedback);
                        out.push_str("\n\n");
                    }
                }
            }

            self.message = Some(match std::fs::write(&path, out) {
                Ok(()) => {
                    let summary = format!(
                        "Final review: {approved} approved, {} need work, {skipped} skipped \
                         — feedback saved to .claude/final-review-feedback.md",
                        rejected.len()
                    );
                    match &agent_target {
                        Some((session, window)) => {
                            let prompt = "A reviewer left feedback on these changes in \
                                 .claude/final-review-feedback.md. Please read that file and \
                                 address every item in it.";
                            let sent = self
                                .tmux
                                .paste_text(session, window, prompt)
                                .and_then(|()| self.tmux.send_key_name(session, window, "Enter"));
                            match sent {
                                Ok(()) => format!("{summary} — sent to agent"),
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
    use super::parse_review_notes;

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
