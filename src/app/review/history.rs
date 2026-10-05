//! Read-only review rounds shared by the terminal and desktop interfaces.
use super::preparation::{FEEDBACK_TITLE, parse_review_history_rounds};
use crate::app::{App, AppMode, DiffViewerState, ReviewDecision, ReviewHistoryState};

impl App {
    /// Open the read-only review-round timeline. Only the bounded live feedback
    /// log is read here; the archive is deferred until explicitly requested or
    /// TUI navigation reaches past the loaded tail. Browsing never puts old rounds
    /// back on the fixing agent's normal read path.
    pub fn open_review_history(&mut self) {
        if self.refuse_in_pr_review("review history belongs to a local feature's review rounds") {
            return;
        }
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
    pub(crate) fn load_review_history_archive(&mut self) {
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
    /// loaded tail triggers the lazy archive read in the TUI.
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
}

/// Compose the live `Current` history body from review state. Unlike finished
/// rounds this includes drafts and resolved threads, because it is a faithful
/// read-only projection of what the reviewer can return to and edit.
pub(crate) fn current_review_history_markdown(state: &DiffViewerState) -> String {
    let mut approved = 0usize;
    let mut rejected = 0usize;
    for file in &state.files {
        match state.decisions.get(&file.path) {
            Some(ReviewDecision::Approve) => approved += 1,
            Some(ReviewDecision::Reject { .. }) => rejected += 1,
            None => {}
        }
    }
    let undecided = state
        .files
        .len()
        .saturating_sub(approved)
        .saturating_sub(rejected);
    let mut out = format!(
        "## Current Review\n\n**Files:** {} | **Approved:** {approved} | **Needs work:** \
         {rejected} | **No verdict:** {undecided} | **Open threads:** {}\n\n",
        state.files.len(),
        state.unresolved_thread_count()
    );
    if state.finish_check_child.is_some() {
        out.push_str("**Check:** running…\n\n");
    }
    if !state.general_feedback.trim().is_empty() {
        out.push_str("### General Feedback\n\n");
        out.push_str(state.general_feedback.trim());
        out.push_str("\n\n");
    }
    for file in &state.files {
        let verdict = match state.decisions.get(&file.path) {
            Some(ReviewDecision::Approve) => "approved".to_string(),
            Some(ReviewDecision::Reject { severity, .. }) => {
                format!("needs work [{}]", severity.label())
            }
            None => "no verdict".to_string(),
        };
        out.push_str(&format!("### {} — {verdict}\n\n", file.path));
        if let Some(ReviewDecision::Reject { feedback, .. }) = state.decisions.get(&file.path)
            && !feedback.trim().is_empty()
        {
            out.push_str(feedback.trim());
            out.push_str("\n\n");
        }
        if let Some(comment) = state.file_comments.get(&file.path) {
            let status = if comment.resolved { "resolved" } else { "open" };
            out.push_str(&format!(
                "**File comment [{} · {status}]:** {}\n\n",
                comment.severity.label(),
                comment.text.trim()
            ));
        }
        if let Some(comments) = state.line_comments.get(&file.path) {
            for comment in comments {
                let start = comment.start.and_then(|loc| loc.new_line.or(loc.old_line));
                let end = comment.location.new_line.or(comment.location.old_line);
                let anchor = match (start, end) {
                    (Some(start), Some(end)) if start != end => format!("L{start}-{end}"),
                    (_, Some(end)) => format!("L{end}"),
                    _ => "anchor lost".to_string(),
                };
                let status = if comment.draft {
                    "AI draft"
                } else if comment.resolved {
                    "resolved"
                } else if comment.carried {
                    "open · carried"
                } else {
                    "open"
                };
                out.push_str(&format!(
                    "#### {anchor} — [{} · {status}]\n\n",
                    comment.severity.label()
                ));
                if !comment.text.trim().is_empty() {
                    out.push_str(comment.text.trim());
                    out.push_str("\n\n");
                }
                if let Some(suggestion) = &comment.suggestion {
                    out.push_str("```suggestion\n");
                    out.push_str(suggestion);
                    out.push_str("\n```\n\n");
                }
            }
        }
        if let Some(responses) = state.prior_agent_responses.get(&file.path) {
            for response in responses {
                out.push_str(&format!(
                    "**Agent reply to {}:** {}\n\n",
                    response.anchor,
                    response.response.trim()
                ));
            }
        }
    }
    out
}
