use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::*;
use crate::app::pr_review::FixTarget;

/// Label (and de-facto identity) of the dedicated final-review agent session.
/// Found-or-created by this label so re-running the review reuses the same
/// window. Kept distinct from PR review's "PR Review" session so the two
/// dedicated targets never collide on one feature.
pub(crate) const FINAL_REVIEW_SESSION_LABEL: &str = "Final Review";

/// The prompt dispatched to the agent after a review finishes, asking it to act
/// on the feedback file's most recent round.
const REVIEW_FEEDBACK_PROMPT: &str = "A reviewer left feedback on these changes in \
     .claude/final-review-feedback.md. Read that file and address every item in the most recent \
     review round (the first \"## Review\" section); earlier sections are prior rounds kept for \
     history.";

/// The resumable parts of an in-flight final review, persisted to
/// `.claude/final-review-progress.json` so a long review can be paused
/// (or survive an AMF quit / crash) and picked up where it left off.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ReviewProgress {
    #[serde(default)]
    decisions: std::collections::HashMap<String, ReviewDecision>,
    /// Which of `decisions`' rejections were auto-set by a line comment rather
    /// than explicitly. Defaulted so older progress files load unchanged (their
    /// rejections simply read as explicit — the conservative direction).
    #[serde(default)]
    auto_rejected: std::collections::HashSet<String>,
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

/// A fingerprint of the diff as it stood at the last *finished* review round,
/// keyed by file path. Persisted to `.claude/final-review-snapshot.json` (kept
/// across rounds, unlike the progress file) so the next review can flag which
/// files changed since the reviewer last looked — the re-review loop.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ReviewSnapshot {
    #[serde(default)]
    reviewed_at: String,
    /// file path -> diff fingerprint at the time of the last finished review.
    #[serde(default)]
    files: std::collections::HashMap<String, String>,
    /// file path -> the verdict that file carried when the round finished, so a
    /// re-review can restore approvals for files that have not changed since.
    /// Defaulted so snapshots written before this field load unchanged (no
    /// carried verdicts, everything simply re-checked).
    #[serde(default)]
    decisions: std::collections::HashMap<String, ReviewDecision>,
}

/// Path of the saved review-snapshot file for a feature workdir.
fn review_snapshot_path(workdir: &Path) -> PathBuf {
    workdir.join(".claude").join("final-review-snapshot.json")
}

/// Best-effort load of the last review snapshot for `workdir`.
fn load_review_snapshot(workdir: &Path) -> Option<ReviewSnapshot> {
    let content = std::fs::read_to_string(review_snapshot_path(workdir)).ok()?;
    serde_json::from_str(&content).ok()
}

/// A stable-enough fingerprint of a file's diff. Hashes the patch plus the
/// status / line counts so a content change reads as "changed". This is a local
/// cache only: `DefaultHasher` is not guaranteed stable across toolchain
/// versions, so after an upgrade everything reads as changed — the safe default
/// (the reviewer simply re-checks).
fn file_fingerprint(file: &crate::diff::DiffFile) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    file.patch.hash(&mut hasher);
    file.additions.hash(&mut hasher);
    file.deletions.hash(&mut hasher);
    file.is_binary.hash(&mut hasher);
    format!("{:?}", file.status).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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
            auto_rejected: state.auto_rejected.clone(),
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

    /// Record a fingerprint of the just-reviewed diff to
    /// `.claude/final-review-snapshot.json` so the next review can flag which
    /// files changed since (the re-review loop). Best-effort: a write failure is
    /// logged, not surfaced — it only degrades change detection on the next
    /// round. Unlike the progress file, this is *not* cleared on finish.
    fn save_review_snapshot(
        &mut self,
        workdir: &Path,
        files: &[crate::diff::DiffFile],
        decisions: &std::collections::HashMap<String, ReviewDecision>,
    ) {
        let snapshot = ReviewSnapshot {
            reviewed_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            files: files
                .iter()
                .map(|f| (f.path.clone(), file_fingerprint(f)))
                .collect(),
            // Only persist verdicts for files still in the diff, keyed the same
            // as the fingerprints above.
            decisions: files
                .iter()
                .filter_map(|f| decisions.get(&f.path).map(|d| (f.path.clone(), d.clone())))
                .collect(),
        };
        let path = review_snapshot_path(workdir);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&snapshot) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&path, json) {
                    self.log_warn("review", format!("failed to save review snapshot: {err}"));
                }
            }
            Err(err) => self.log_warn(
                "review",
                format!("failed to serialize review snapshot: {err}"),
            ),
        }
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
        state.auto_rejected = progress
            .auto_rejected
            .into_iter()
            .filter(|path| known.contains(path.as_str()))
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

    /// Compare the current diff against the last finished review's snapshot and
    /// flag the files that changed since — the re-review loop. Called after the
    /// diff loads (initial open and every refresh), so the `Changed` filter and
    /// the file-list marker always reflect the live diff.
    ///
    /// On a *pristine* open of a re-review — no decisions / feedback yet, default
    /// filter — where some but not all files changed, it auto-applies the
    /// `Changed` filter and snaps the selection onto the first changed file so
    /// the reviewer immediately sees only what needs re-checking. It never
    /// overrides a filter the reviewer is already working under (e.g. after a
    /// refresh mid-review).
    pub fn apply_review_snapshot_diff(&mut self) {
        let AppMode::DiffViewer(state) = &mut self.mode else {
            return;
        };
        if !state.review {
            return;
        }
        let Some(snapshot) = load_review_snapshot(&state.workdir) else {
            state.has_prior_review = false;
            state.changed_since_last.clear();
            return;
        };
        state.has_prior_review = true;
        state.changed_since_last = state
            .files
            .iter()
            .filter(|file| {
                snapshot
                    .files
                    .get(&file.path)
                    .map(|fp| *fp != file_fingerprint(file))
                    .unwrap_or(true)
            })
            .map(|file| file.path.clone())
            .collect();

        let total = state.files.len();
        let changed = state.changed_since_last.len();
        // Only steer a fresh review: leave an in-progress / refreshed review's
        // filter and selection untouched. Computed before any carry-over below
        // so restored approvals don't make the open read as non-pristine.
        let pristine = state.decisions.is_empty()
            && state.line_comments.is_empty()
            && state.general_feedback.is_empty()
            && state.file_filter == FileFilter::All;
        if !pristine {
            return;
        }
        // Fresh open of a re-review: carry over approvals for files that have not
        // changed since the last finished round, so the reviewer resumes from the
        // saved approved state and only re-checks the changed / rejected files.
        // Changed files were dropped from `changed_since_last`'s complement, so
        // their stale approval is deliberately not restored.
        let carry: Vec<String> = state
            .files
            .iter()
            .filter(|f| !state.changed_since_last.contains(&f.path))
            .filter(|f| {
                matches!(
                    snapshot.decisions.get(&f.path),
                    Some(ReviewDecision::Approve)
                )
            })
            .map(|f| f.path.clone())
            .collect();
        for path in carry {
            state.decisions.insert(path, ReviewDecision::Approve);
        }
        let when = if snapshot.reviewed_at.is_empty() {
            String::new()
        } else {
            format!(" (last reviewed {})", snapshot.reviewed_at)
        };
        if changed == 0 {
            self.message = Some(format!(
                "Re-review: no files changed since the last review{when}"
            ));
        } else if changed == total {
            self.message = Some(format!(
                "Re-review: all {total} file(s) changed since the last review{when}"
            ));
        } else {
            // Narrow to the changed files and land on the first of them.
            state.file_filter = FileFilter::Changed;
            if let Some(idx) = state.visible_file_indices().into_iter().next() {
                state.selected_file = idx;
                state.on_file_changed();
            }
            self.message = Some(format!(
                "Re-review: {changed}/{total} file(s) changed since the last \
                 review{when} — showing changed only (F to cycle filter)"
            ));
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
                // An explicit verdict wins over (and sticks against) a
                // rejection auto-set by a line comment.
                state.auto_rejected.remove(&file.path);
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
                // An explicit skip clears a comment-implied rejection. A later
                // comment mutation on the file re-defaults it — a fresh signal
                // on a file with no verdict.
                state.auto_rejected.remove(&file.path);
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

    /// Toggle the multi-line selection anchor at the current cursor. With the
    /// anchor set, moving the cursor extends a span that the next comment covers;
    /// toggling again drops back to a single-line comment. No-op without an
    /// active line cursor.
    pub fn diff_review_toggle_range_anchor(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let Some(cur) = state.comment_cursor else {
                return;
            };
            state.comment_anchor = if state.comment_anchor.is_some() {
                None
            } else {
                Some(cur)
            };
        }
    }

    /// Open the comment editor for the selected diff line(s), pre-filling any
    /// comment already covering the cursor. When the cursor lands on an existing
    /// (possibly multi-line) comment with no active selection, the anchor/cursor
    /// snap onto that comment's span so a re-submit edits the whole range.
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
            if locs.get(cur).is_none() {
                return;
            }
            let path = file.path.clone();
            // An existing comment covering the cursor is edited in place: prefill
            // its text and, when no fresh selection is active, snap the
            // anchor/cursor onto its span so the edit preserves the range.
            let existing = state.line_comments.get(&path).and_then(|comments| {
                comments.iter().find_map(|c| {
                    c.covered_indices(&locs)
                        .filter(|range| range.contains(&cur))
                        .map(|range| (c.text.clone(), range))
                })
            });
            let text = if let Some((text, range)) = existing {
                if state.comment_anchor.is_none() {
                    state.comment_anchor = Some(*range.start());
                    state.comment_cursor = Some(*range.end());
                }
                text
            } else {
                String::new()
            };
            state.feedback_editor = crate::editor::TextEditor::new(text);
            state.feedback_scroll = 0;
            state.feedback_sync_to_cursor = true;
            state.editing_line_comment = true;
            state.feedback_editing = false;
            state.editing_general = false;
        }
    }

    /// Store (or, when empty, delete) the typed comment for the selected line
    /// span (anchor..cursor, or the single cursor line). Replaces any existing
    /// comments overlapping the span and clears the selection anchor. Does not
    /// advance the cursor, so a reviewer can keep annotating nearby lines.
    pub fn diff_review_submit_line_comment(&mut self) {
        let mut commented_path = None;
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.editing_line_comment {
                return;
            }
            let text = state.feedback_editor.text().trim().to_string();
            let span = state.comment_cursor.and_then(|cur| {
                state.files.get(state.selected_file).and_then(|file| {
                    let locs = file.addressable_lines();
                    let lo = state.comment_anchor.unwrap_or(cur).min(cur);
                    let hi = state.comment_anchor.unwrap_or(cur).max(cur);
                    let start = locs.get(lo).copied()?;
                    let end = locs.get(hi).copied()?;
                    Some((file.path.clone(), lo, hi, start, end))
                })
            });
            if let Some((path, lo, hi, start, end)) = span {
                let locs = state
                    .files
                    .get(state.selected_file)
                    .map(|f| f.addressable_lines())
                    .unwrap_or_default();
                commented_path = Some(path.clone());
                let comments = state.line_comments.entry(path).or_default();
                // Carry over a suggested change already attached to the span so
                // editing the prose doesn't drop it.
                let suggestion = comments
                    .iter()
                    .find(|c| {
                        c.covered_indices(&locs)
                            .map(|r| !(*r.end() < lo || *r.start() > hi))
                            .unwrap_or(false)
                    })
                    .and_then(|c| c.suggestion.clone());
                // Drop any existing comment whose span overlaps the new one.
                comments.retain(|c| {
                    c.covered_indices(&locs)
                        .map(|r| *r.end() < lo || *r.start() > hi)
                        .unwrap_or(true)
                });
                // Keep the comment when it has prose or a carried-over suggestion;
                // an empty prose with no suggestion means "delete".
                if !text.is_empty() || suggestion.is_some() {
                    comments.push(LineComment {
                        location: end,
                        start: (lo != hi).then_some(start),
                        text,
                        // A comment the human just wrote (or edited) is never a
                        // draft, even if it replaced an AI draft on this span.
                        draft: false,
                        suggestion,
                    });
                    comments.sort_by_key(|c| {
                        let loc = c.start.unwrap_or(c.location);
                        loc.new_line.or(loc.old_line).unwrap_or(0)
                    });
                }
            }
            state.editing_line_comment = false;
            state.comment_anchor = None;
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
        }
        if let Some(path) = commented_path {
            self.diff_review_sync_auto_reject(&path);
        }
        self.persist_review_progress();
    }

    /// Reconcile a file's implicit "needs revision" verdict with its kept
    /// (non-draft) line comments: a commented file is a file that needs work, so
    /// the first kept comment defaults an undecided file to `Reject` with empty
    /// feedback (the comments carry the specifics), and removing the last kept
    /// comment clears a rejection that was auto-set this way. An explicit
    /// verdict — approve/skip/reject, or a carried-over approval — is never
    /// overridden. Called after every comment mutation, before the progress
    /// persist, so what's on disk always reflects the synced verdict.
    fn diff_review_sync_auto_reject(&mut self, path: &str) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let has_kept = state
                .line_comments
                .get(path)
                .is_some_and(|comments| comments.iter().any(|c| !c.draft));
            if has_kept {
                if !state.decisions.contains_key(path) {
                    state.decisions.insert(
                        path.to_string(),
                        ReviewDecision::Reject {
                            feedback: String::new(),
                        },
                    );
                    state.auto_rejected.insert(path.to_string());
                }
            } else if state.auto_rejected.remove(path) {
                state.decisions.remove(path);
            }
        }
    }

    /// Open the suggested-change editor for the cursored line(s). Pre-fills the
    /// editor with any suggestion already attached to the span, else the current
    /// text of the covered lines (so the reviewer edits a real replacement). Like
    /// the comment editor, snaps the anchor/cursor onto an existing comment's
    /// span when the cursor lands on it with no active selection, so the
    /// suggestion attaches to the whole comment.
    pub fn diff_review_start_suggestion(&mut self) {
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
            if locs.get(cur).is_none() {
                return;
            }
            let texts = file.addressable_line_texts();
            let path = file.path.clone();
            // An existing comment covering the cursor: reuse its span (when no
            // fresh selection is active) and its suggestion (when it has one).
            let existing = state.line_comments.get(&path).and_then(|comments| {
                comments.iter().find_map(|c| {
                    c.covered_indices(&locs)
                        .filter(|range| range.contains(&cur))
                        .map(|range| (c.suggestion.clone(), range))
                })
            });
            let prefill = match existing {
                Some((suggestion, range)) => {
                    if state.comment_anchor.is_none() {
                        state.comment_anchor = Some(*range.start());
                        state.comment_cursor = Some(*range.end());
                    }
                    // Existing suggestion: edit it. Otherwise seed from the span's
                    // current code.
                    suggestion.unwrap_or_else(|| span_current_text(&texts, &range))
                }
                None => {
                    let lo = state.comment_anchor.unwrap_or(cur).min(cur);
                    let hi = state.comment_anchor.unwrap_or(cur).max(cur);
                    span_current_text(&texts, &(lo..=hi))
                }
            };
            state.feedback_editor = crate::editor::TextEditor::new(prefill);
            state.feedback_scroll = 0;
            state.feedback_sync_to_cursor = true;
            state.editing_suggestion = true;
            state.editing_line_comment = false;
            state.feedback_editing = false;
            state.editing_general = false;
        }
    }

    /// Store the typed suggested change on the comment covering the selected line
    /// span (creating a comment with empty prose when none exists). An empty
    /// suggestion clears it — deleting the comment when it also has no prose.
    pub fn diff_review_submit_suggestion(&mut self) {
        let mut commented_path = None;
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.editing_suggestion {
                return;
            }
            // Trailing-newline trimmed but interior whitespace preserved: a
            // suggestion is verbatim code.
            let text = state.feedback_editor.text().trim_end().to_string();
            let suggestion = (!text.is_empty()).then_some(text);
            let span = state.comment_cursor.and_then(|cur| {
                state.files.get(state.selected_file).and_then(|file| {
                    let locs = file.addressable_lines();
                    let lo = state.comment_anchor.unwrap_or(cur).min(cur);
                    let hi = state.comment_anchor.unwrap_or(cur).max(cur);
                    let start = locs.get(lo).copied()?;
                    let end = locs.get(hi).copied()?;
                    Some((file.path.clone(), lo, hi, start, end))
                })
            });
            if let Some((path, lo, hi, start, end)) = span {
                let locs = state
                    .files
                    .get(state.selected_file)
                    .map(|f| f.addressable_lines())
                    .unwrap_or_default();
                commented_path = Some(path.clone());
                let comments = state.line_comments.entry(path).or_default();
                // Preserve the prose of an existing comment on the span; a fresh
                // suggestion-only comment carries empty prose.
                let prose = comments
                    .iter()
                    .find(|c| {
                        c.covered_indices(&locs)
                            .map(|r| !(*r.end() < lo || *r.start() > hi))
                            .unwrap_or(false)
                    })
                    .map(|c| c.text.clone())
                    .unwrap_or_default();
                comments.retain(|c| {
                    c.covered_indices(&locs)
                        .map(|r| *r.end() < lo || *r.start() > hi)
                        .unwrap_or(true)
                });
                // Keep the comment only if it still carries prose or a suggestion.
                if !prose.is_empty() || suggestion.is_some() {
                    comments.push(LineComment {
                        location: end,
                        start: (lo != hi).then_some(start),
                        text: prose,
                        draft: false,
                        suggestion,
                    });
                    comments.sort_by_key(|c| {
                        let loc = c.start.unwrap_or(c.location);
                        loc.new_line.or(loc.old_line).unwrap_or(0)
                    });
                }
            }
            state.editing_suggestion = false;
            state.comment_anchor = None;
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
        }
        if let Some(path) = commented_path {
            self.diff_review_sync_auto_reject(&path);
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
            if state.review_notes.contains_key(&path) || state.generated_notes.contains_key(&path) {
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
            AppMode::DiffViewer(state) => (
                state.walkthrough_child.take(),
                state.walkthrough_file.take(),
            ),
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

    /// Run an AI co-reviewer pass over the **current file** (reviewer-triggered,
    /// per-file, bounded — see the final-review plan's "opt-in / bounded"
    /// requirement). Spawns a headless Claude that reports findings as
    /// `<line>|<comment>`; `poll_co_review` parses them into *draft* line
    /// comments the reviewer then accepts / edits / dismisses.
    pub fn generate_co_review(&mut self) {
        let (workdir, path, prompt) = {
            let AppMode::DiffViewer(state) = &self.mode else {
                return;
            };
            if !state.review || state.co_review_child.is_some() {
                return;
            }
            let Some(file) = state.files.get(state.selected_file) else {
                return;
            };
            if file.is_binary {
                self.message = Some("AI co-review: binary file skipped".to_string());
                return;
            }
            if file.hunks.is_empty() {
                self.message = Some("AI co-review: nothing to review in this file".to_string());
                return;
            }
            (
                state.workdir.clone(),
                file.path.clone(),
                build_co_review_prompt(file),
            )
        };

        match crate::claude::ClaudeLauncher::spawn_headless(&workdir, &prompt) {
            Ok(child) => {
                self.message = Some(format!("AI co-review running on {path}…"));
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.co_review_child = Some(child);
                    state.co_review_file = Some(path);
                }
            }
            Err(err) => {
                self.message = Some(format!("AI co-review unavailable: {err}"));
            }
        }
    }

    /// Poll an in-flight co-review pass; on completion parse its findings into
    /// draft line comments for the file it ran on. Mirrors
    /// `poll_review_walkthrough`.
    pub fn poll_co_review(&mut self) -> Result<()> {
        let finished = match &mut self.mode {
            AppMode::DiffViewer(state) => match state.co_review_child.as_mut() {
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
                (state.co_review_child.take(), state.co_review_file.take())
            }
            _ => (None, None),
        };
        let (Some(child), Some(path)) = (child, path) else {
            return Ok(());
        };

        let output = child.wait_with_output()?;
        if !status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            self.message = Some(format!("AI co-review failed: {stderr}"));
            return Ok(());
        }
        let text = String::from_utf8_lossy(&output.stdout);

        let added = if let AppMode::DiffViewer(state) = &mut self.mode {
            let Some(file) = state.files.iter().find(|f| f.path == path) else {
                return Ok(());
            };
            let locs = file.addressable_lines();
            let drafts = parse_co_review_output(&text, &locs);
            let existing = state.line_comments.entry(path.clone()).or_default();
            let mut added = 0usize;
            for draft in drafts {
                // Don't stack a draft on a line that already carries a comment
                // (human or a prior draft).
                let overlaps = existing.iter().any(|c| {
                    match (c.covered_indices(&locs), draft.covered_indices(&locs)) {
                        (Some(a), Some(b)) => *a.start() <= *b.end() && *b.start() <= *a.end(),
                        _ => false,
                    }
                });
                if overlaps {
                    continue;
                }
                existing.push(draft);
                added += 1;
            }
            existing.sort_by_key(|c| {
                let loc = c.start.unwrap_or(c.location);
                loc.new_line.or(loc.old_line).unwrap_or(0)
            });
            added
        } else {
            return Ok(());
        };

        self.message = Some(if added == 0 {
            format!("AI co-review: no new findings for {path}")
        } else {
            format!("AI co-review added {added} draft comment(s) — a accept · d dismiss")
        });
        self.persist_review_progress();
        Ok(())
    }

    /// Accept the AI draft comment under the line cursor, promoting it to a
    /// permanent human comment. Returns `true` if a draft was accepted (so the
    /// key handler can stop), `false` if the cursored line carries no draft.
    pub fn diff_review_accept_draft_under_cursor(&mut self) -> bool {
        let acted = if let AppMode::DiffViewer(state) = &mut self.mode {
            let Some(cur) = state.comment_cursor else {
                return false;
            };
            let Some(file) = state.files.get(state.selected_file) else {
                return false;
            };
            let path = file.path.clone();
            let locs = file.addressable_lines();
            match state.line_comments.get_mut(&path).and_then(|comments| {
                comments.iter_mut().find(|c| {
                    c.draft
                        && c.covered_indices(&locs)
                            .is_some_and(|range| range.contains(&cur))
                })
            }) {
                Some(comment) => {
                    comment.draft = false;
                    Some(path)
                }
                None => None,
            }
        } else {
            None
        };
        if let Some(path) = acted {
            self.message = Some("Draft comment accepted".to_string());
            // An accepted draft is a human-affirmed finding: it counts toward
            // the file's implicit "needs revision" verdict like a hand-written
            // comment.
            self.diff_review_sync_auto_reject(&path);
            self.persist_review_progress();
            return true;
        }
        false
    }

    /// Dismiss (delete) the AI draft comment under the line cursor. Returns
    /// `true` if a draft was removed.
    pub fn diff_review_dismiss_draft_under_cursor(&mut self) -> bool {
        let acted = if let AppMode::DiffViewer(state) = &mut self.mode {
            let Some(cur) = state.comment_cursor else {
                return false;
            };
            let Some(file) = state.files.get(state.selected_file) else {
                return false;
            };
            let path = file.path.clone();
            let locs = file.addressable_lines();
            match state.line_comments.get_mut(&path) {
                Some(comments) => {
                    let before = comments.len();
                    comments.retain(|c| {
                        !(c.draft
                            && c.covered_indices(&locs)
                                .is_some_and(|range| range.contains(&cur)))
                    });
                    (comments.len() != before).then_some(path)
                }
                None => None,
            }
        } else {
            None
        };
        if let Some(path) = acted {
            self.message = Some("Draft comment dismissed".to_string());
            // Dismissing a draft can't add a kept comment, but the sync keeps
            // every comment-mutation path uniform.
            self.diff_review_sync_auto_reject(&path);
            self.persist_review_progress();
            return true;
        }
        false
    }

    /// Move the line cursor to the next draft comment in the current file
    /// (wrapping), so a reviewer can Tab through the AI's findings.
    pub fn diff_review_jump_next_draft(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let Some(file) = state.files.get(state.selected_file) else {
                return;
            };
            let locs = file.addressable_lines();
            // Indices of every draft's anchor line, in display order.
            let mut draft_indices: Vec<usize> = state
                .line_comments
                .get(&file.path)
                .into_iter()
                .flatten()
                .filter(|c| c.draft)
                .filter_map(|c| c.covered_indices(&locs).map(|r| *r.start()))
                .collect();
            draft_indices.sort_unstable();
            draft_indices.dedup();
            if draft_indices.is_empty() {
                self.message = Some("No draft comments on this file".to_string());
                return;
            }
            let cur = state.comment_cursor.unwrap_or(0);
            let next = draft_indices
                .iter()
                .find(|&&idx| idx > cur)
                .copied()
                .unwrap_or(draft_indices[0]);
            state.comment_cursor = Some(next);
            state.comment_anchor = None;
            state.cursor_sync_to_view = true;
        }
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
            state.feedback_editor = crate::editor::TextEditor::new(state.general_feedback.clone());
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
            state.editing_suggestion = false;
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
                // The reviewer typed this rejection themselves: it is explicit
                // now and no longer tracks the file's comments.
                state.auto_rejected.remove(&file.path);
            }
            state.feedback_editing = false;
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
        }
        self.diff_review_advance();
        self.persist_review_progress();
    }

    fn diff_review_advance(&mut self) {
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
            // `Changed` is only meaningful when a prior review exists to compare
            // against; otherwise skip straight past it.
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

    /// Toggle where a finished review's "address the feedback" prompt is
    /// dispatched: the feature's existing agent pane (default) or a fresh
    /// dedicated review session (the reviewer picks the harness at finish).
    pub fn diff_review_toggle_fix_target(&mut self) {
        let msg = if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            state.fix_target = match state.fix_target {
                FixTarget::ExistingLive => FixTarget::DedicatedReview,
                FixTarget::DedicatedReview => FixTarget::ExistingLive,
            };
            match state.fix_target {
                FixTarget::DedicatedReview => {
                    "Fixes will run in a fresh dedicated review session (you'll pick the harness)"
                }
                FixTarget::ExistingLive => "Fixes go to the feature's existing agent session",
            }
        } else {
            return;
        };
        self.message = Some(msg.to_string());
    }

    /// Finish the review: write `.claude/final-review-feedback.md` for any
    /// rejected files and return to the feature view with a summary message.
    pub fn finish_final_review(&mut self) -> Result<()> {
        let (workdir, files, decisions, line_comments, general_feedback, from_view, fix_target) =
            match std::mem::replace(&mut self.mode, AppMode::Normal) {
                AppMode::DiffViewer(state) => (
                    state.workdir,
                    state.files,
                    state.decisions,
                    state.line_comments,
                    state.general_feedback,
                    state.from_view,
                    state.fix_target,
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
        // …but record a fingerprint of what was reviewed (even an all-approved
        // round) so the next review can flag files that changed since.
        if !files.is_empty() {
            self.save_review_snapshot(&workdir, &files, &decisions);
        }

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
        let skipped = total
            .saturating_sub(approved)
            .saturating_sub(rejected.len());
        let general_feedback = general_feedback.trim().to_string();
        let post_to_pr = self.config.final_review_post_to_pr;

        // Line comments in file order (each file's comments are already sorted
        // by line). Empty-text comments never reach here (submit deletes them).
        let mut line_comment_sections: Vec<(String, Vec<LineComment>)> = Vec::new();
        let mut line_comment_count = 0usize;
        for file in &files {
            if let Some(comments) = line_comments.get(&file.path) {
                // Unaccepted AI drafts never reach the feedback file or the PR
                // review — only comments the human kept.
                let kept: Vec<LineComment> =
                    comments.iter().filter(|c| !c.draft).cloned().collect();
                if !kept.is_empty() {
                    line_comment_count += kept.len();
                    line_comment_sections.push((file.path.clone(), kept));
                }
            }
        }

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
            self.mode = AppMode::Viewing(from_view);
            return Ok(());
        }
        {
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
                        // For a rejection implied by line comments the comments
                        // are the feedback — send the agent there instead of
                        // reporting a missing rationale.
                        if line_comment_sections.iter().any(|(path, _)| path == file) {
                            round.push_str(
                                "(Needs revision — see this file's line comments below)\n\n",
                            );
                        } else {
                            round.push_str("(No feedback provided — needs revision)\n\n");
                        }
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
                        let anchor = comment_anchor_label(file, comment);
                        round.push_str(&format!("#### {anchor}\n\n"));
                        if !comment.text.is_empty() {
                            round.push_str(&comment.text);
                            round.push_str("\n\n");
                        }
                        // A suggested change is a verbatim replacement: emit it as
                        // a fenced ```suggestion block so the agent applies it as
                        // a patch rather than interpreting prose.
                        if let Some(suggestion) = &comment.suggestion {
                            round.push_str(&format!("```suggestion\n{suggestion}\n```\n\n"));
                        }
                    }
                }
            }

            let out = compose_feedback_log(std::fs::read_to_string(&path).ok().as_deref(), &round);

            if let Err(e) = std::fs::write(&path, out) {
                self.message = Some(format!("Final review: failed to write feedback file: {e}"));
                self.mode = AppMode::Viewing(from_view);
                return Ok(());
            }

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
            // Optionally mirror the feedback onto the branch's GitHub PR as a
            // review (best-effort; the local file is the source of truth either
            // way).
            let pr_note = if post_to_pr {
                self.post_final_review_to_pr(
                    &workdir,
                    &rejected,
                    &line_comment_sections,
                    &general_feedback,
                )
            } else {
                String::new()
            };
            // Dispatch the "address the feedback" prompt to the chosen target.
            // This sets `self.message` and `self.mode` (it may open the harness
            // picker when a fresh dedicated session is needed).
            self.dispatch_review_feedback(from_view, format!("{summary}{pr_note}"), fix_target);
        }
        Ok(())
    }

    /// Dispatch a finished review's "address the feedback" prompt to the agent
    /// session chosen by `fix_target`, then return to the feature view. For the
    /// existing-pane target this pastes into the feature's first agent session
    /// (the shipped behaviour). For the dedicated target it reuses an existing
    /// "Final Review" session, or — when none exists — opens the harness picker
    /// so the reviewer chooses which harness runs the fixes. Sets `self.message`
    /// and `self.mode`.
    fn dispatch_review_feedback(
        &mut self,
        from_view: ViewState,
        summary: String,
        fix_target: FixTarget,
    ) {
        let indices = self.store.projects.iter().enumerate().find_map(|(pi, p)| {
            if p.name != from_view.project_name {
                return None;
            }
            p.features
                .iter()
                .position(|f| f.name == from_view.feature_name)
                .map(|fi| (pi, fi))
        });
        let Some((pi, fi)) = indices else {
            self.message = Some(summary);
            self.mode = AppMode::Viewing(from_view);
            return;
        };

        let feature = &self.store.projects[pi].features[fi];
        let tmux_session = feature.tmux_session.clone();
        let target_window = crate::app::pr_review::fix_session_index(
            feature,
            fix_target,
            FINAL_REVIEW_SESSION_LABEL,
        )
        .map(|si| feature.sessions[si].tmux_window.clone());

        if let Some(window) = target_window {
            let suffix = self.paste_review_prompt(&tmux_session, &window);
            self.message = Some(format!("{summary}{suffix}"));
            self.mode = AppMode::Viewing(from_view);
            return;
        }

        match fix_target {
            // No agent session to paste into — report and stop (shipped
            // behaviour for a feature whose agent isn't running).
            FixTarget::ExistingLive => {
                self.message = Some(summary);
                self.mode = AppMode::Viewing(from_view);
            }
            // A dedicated session must be spun up; let the reviewer pick which
            // harness runs the fixes before it is created.
            FixTarget::DedicatedReview => {
                let harnesses = if self.store.available_harnesses.is_empty() {
                    vec![self.store.projects[pi].preferred_agent.clone()]
                } else {
                    self.store.available_harnesses.clone()
                };
                self.mode = AppMode::ReviewHarnessPick(ReviewHarnessPickState {
                    pi,
                    fi,
                    summary,
                    from_view,
                    harnesses,
                    selected: 0,
                });
            }
        }
    }

    /// Paste the address-feedback prompt into a resolved agent window,
    /// submitting (sending Enter) when configured. Returns a short status suffix
    /// for the finish message.
    fn paste_review_prompt(&mut self, session: &str, window: &str) -> String {
        let submit = self.config.final_review_submit_prompt;
        let pasted = self
            .tmux
            .paste_text(session, window, REVIEW_FEEDBACK_PROMPT)
            .and_then(|()| {
                if submit {
                    self.tmux.send_key_name(session, window, "Enter")
                } else {
                    Ok(())
                }
            });
        match pasted {
            Ok(()) if submit => " — sent to agent".to_string(),
            Ok(()) => " — pasted to agent (not submitted)".to_string(),
            Err(e) => format!(" (couldn't prompt agent: {e})"),
        }
    }

    /// Move the harness-pick selection by `delta` (negative = up), wrapping.
    pub fn review_harness_pick_move(&mut self, delta: isize) {
        if let AppMode::ReviewHarnessPick(state) = &mut self.mode {
            let n = state.harnesses.len();
            if n == 0 {
                return;
            }
            let cur = state.selected as isize;
            state.selected = (cur + delta).rem_euclid(n as isize) as usize;
        }
    }

    /// Confirm the harness pick: create the dedicated review session under the
    /// chosen harness, paste the feedback prompt, and return to the feature view.
    pub fn review_harness_pick_select(&mut self) -> Result<()> {
        let (pi, fi, harness, summary, from_view) = match &self.mode {
            AppMode::ReviewHarnessPick(state) => {
                let Some(harness) = state.harnesses.get(state.selected).cloned() else {
                    return Ok(());
                };
                (
                    state.pi,
                    state.fi,
                    harness,
                    state.summary.clone(),
                    state.from_view.clone(),
                )
            }
            _ => return Ok(()),
        };

        let si = match self.create_dedicated_review_session(
            pi,
            fi,
            FINAL_REVIEW_SESSION_LABEL,
            Some(harness),
        ) {
            Ok(si) => si,
            Err(e) => {
                self.show_error(e);
                self.message = Some(format!(
                    "{summary} (feedback saved; couldn't start review session)"
                ));
                self.mode = AppMode::Viewing(from_view);
                return Ok(());
            }
        };

        let (tmux_session, window) = {
            let feature = &self.store.projects[pi].features[fi];
            (
                feature.tmux_session.clone(),
                feature.sessions[si].tmux_window.clone(),
            )
        };
        let suffix = self.paste_review_prompt(&tmux_session, &window);
        self.message = Some(format!("{summary}{suffix} (dedicated review session)"));
        self.mode = AppMode::Viewing(from_view);
        Ok(())
    }

    /// Cancel the harness pick: the feedback file is already written, so just
    /// return to the feature view without prompting any agent.
    pub fn review_harness_pick_cancel(&mut self) {
        if let AppMode::ReviewHarnessPick(state) = &self.mode {
            let summary = state.summary.clone();
            let from_view = state.from_view.clone();
            self.message = Some(format!("{summary} (feedback saved; no agent prompted)"));
            self.mode = AppMode::Viewing(from_view);
        }
    }

    /// Post a finished review's feedback onto the branch's GitHub PR as a single
    /// review (line comments inline, rejections + general feedback in the
    /// summary). Returns a short suffix for the finish message describing the
    /// outcome. Best-effort: a missing PR, missing `gh`, or an API error is
    /// reported (and logged) but never aborts the finish — the local feedback
    /// file already captured everything.
    fn post_final_review_to_pr(
        &mut self,
        workdir: &Path,
        rejected: &[(String, String)],
        line_comment_sections: &[(String, Vec<LineComment>)],
        general_feedback: &str,
    ) -> String {
        use crate::github::{GhCli, PrResolution};

        let (body, comments) = build_pr_review(rejected, line_comment_sections, general_feedback);
        let pr = match GhCli::resolve_pr(workdir) {
            Ok(PrResolution::Found(pr)) => pr,
            Ok(PrResolution::NoPrForBranch) => {
                return " — no PR for this branch, skipped PR post".to_string();
            }
            Err(err) => {
                self.log_warn("review", format!("PR review post: {err}"));
                return format!(" — couldn't post to PR: {err}");
            }
        };
        match GhCli::create_review(workdir, &pr, &body, "COMMENT", &comments) {
            Ok(()) => {
                let what = if comments.is_empty() {
                    "review summary".to_string()
                } else {
                    format!("{} comment(s)", comments.len())
                };
                format!(" — posted {what} to PR #{}", pr.number)
            }
            Err(err) => {
                self.log_warn("review", format!("PR review post failed: {err}"));
                format!(" — couldn't post to PR #{}: {err}", pr.number)
            }
        }
    }
}

/// Join the current text of the addressable lines in `range` (indices into
/// `addressable_line_texts()`) with newlines. Out-of-range indices are skipped.
/// Used to seed a suggested-change editor with the span's current code.
fn span_current_text(texts: &[String], range: &std::ops::RangeInclusive<usize>) -> String {
    range
        .clone()
        .filter_map(|i| texts.get(i).cloned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `file:line` (or `file:start-end`) heading for a line comment in the
/// feedback file. A base-side (deletion-only) line is tagged `(base)`.
fn comment_anchor_label(file: &str, comment: &LineComment) -> String {
    let line_of = |loc: &crate::diff::DiffLineLocation| loc.new_line.or(loc.old_line);
    let base = comment.location.new_line.is_none() && comment.location.old_line.is_some();
    let suffix = if base { " (base)" } else { "" };
    match (
        comment.start.as_ref().and_then(line_of),
        line_of(&comment.location),
    ) {
        (Some(start), Some(end)) if start != end => format!("{file}:{start}-{end}{suffix}"),
        (_, Some(end)) => format!("{file}:{end}{suffix}"),
        (_, None) => file.to_string(),
    }
}

/// Map a diff-line location to a GitHub `(line, side)` pair: the current-file
/// line (`RIGHT`) when present, else the base-file line (`LEFT`). `None` for an
/// unanchored location.
fn pr_line_side(loc: &crate::diff::DiffLineLocation) -> Option<(usize, &'static str)> {
    match (loc.new_line, loc.old_line) {
        (Some(new_line), _) => Some((new_line, "RIGHT")),
        (None, Some(old_line)) => Some((old_line, "LEFT")),
        (None, None) => None,
    }
}

/// Assemble a GitHub PR review from a finished final review. Line comments
/// become inline review comments — anchored to the current file line
/// (`RIGHT`) or, for a deletion-only line, the base file line (`LEFT`); the
/// general feedback and whole-file rejections (which have no single line to
/// anchor to) become the review's summary body. Returns `(body, comments)`.
fn build_pr_review(
    rejected: &[(String, String)],
    line_comment_sections: &[(String, Vec<LineComment>)],
    general_feedback: &str,
) -> (String, Vec<crate::github::PrReviewComment>) {
    let mut comments = Vec::new();
    for (path, file_comments) in line_comment_sections {
        for comment in file_comments {
            let Some((line, side)) = pr_line_side(&comment.location) else {
                continue;
            };
            // For a span, anchor the start with GitHub's start_line/start_side.
            // Drop the range (post as a single-line comment at the end) if the
            // start can't be mapped to a line/side.
            let (start_line, start_side) = comment
                .start
                .and_then(|s| pr_line_side(&s))
                .map(|(l, sd)| (Some(l), Some(sd)))
                .unwrap_or((None, None));
            // A suggested change posts as a GitHub fenced ```suggestion block so
            // it's one-click-appliable on the PR. Append it to any prose.
            let mut body = comment.text.clone();
            if let Some(suggestion) = &comment.suggestion {
                if !body.is_empty() {
                    body.push_str("\n\n");
                }
                body.push_str(&format!("```suggestion\n{suggestion}\n```"));
            }
            comments.push(crate::github::PrReviewComment {
                path: path.clone(),
                line: line as u32,
                side,
                start_line: start_line.map(|l| l as u32),
                start_side,
                body,
            });
        }
    }

    let mut body = String::new();
    let general = general_feedback.trim();
    if !general.is_empty() {
        body.push_str(general);
        body.push_str("\n\n");
    }
    if !rejected.is_empty() {
        body.push_str("## Files needing revision\n\n");
        for (file, feedback) in rejected {
            let feedback = feedback.trim();
            if feedback.is_empty() {
                body.push_str(&format!("- **{file}** — needs revision\n"));
            } else {
                body.push_str(&format!("- **{file}**\n\n{feedback}\n\n"));
            }
        }
    }
    (body.trim_end().to_string(), comments)
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

/// Build the headless prompt for an AI co-review pass over a single file. The
/// diff is rendered with each current-side line tagged by its **new** line
/// number so the model can anchor findings precisely, and bounded like the
/// walkthrough so a large file can't blow up token cost.
fn build_co_review_prompt(file: &crate::diff::DiffFile) -> String {
    use crate::diff::DiffLineKind;
    const MAX_BODY: usize = 8000;

    let mut body = String::new();
    for hunk in &file.hunks {
        let mut new_line = hunk.new_start;
        for line in &hunk.lines {
            match line.kind {
                DiffLineKind::Context => {
                    body.push_str(&format!("{new_line:>6}   {}\n", line.text));
                    new_line += 1;
                }
                DiffLineKind::Added => {
                    body.push_str(&format!("{new_line:>6} + {}\n", line.text));
                    new_line += 1;
                }
                DiffLineKind::Removed => {
                    body.push_str(&format!("       - {}\n", line.text));
                }
                DiffLineKind::NoNewlineMarker => {}
            }
        }
    }
    if body.len() > MAX_BODY {
        body.truncate(MAX_BODY);
        body.push_str("\n… (diff truncated)");
    }

    format!(
        "You are an AI co-reviewer doing a first pass on a code change so a human \
         reviewer can adjudicate your findings. Review the diff below for bugs, \
         correctness issues, missing edge cases, and clear quality problems. Be \
         selective — only flag things worth a human's attention; skip nits and \
         style unless they matter.\n\n\
         Each line is prefixed with its line number in the new file; `+` marks an \
         added line, `-` a removed line (removed lines have no number).\n\n\
         Output ONLY your findings, one per line, each formatted EXACTLY as \
         `<line>|<comment>` where `<line>` is the new-file line number the comment \
         is about (pick an added or context line, never a removed `-` line). Keep \
         each comment to one or two sentences. If you find nothing worth raising, \
         output nothing at all.\n\n\
         File: {}\n\n```\n{}\n```",
        file.path, body
    )
}

/// Parse the `<line>|<comment>` findings emitted by `build_co_review_prompt`'s
/// pass into draft line comments, anchored by new-file line number onto the
/// file's `addressable_lines()`. Lines that don't parse or don't resolve to a
/// commentable current-side line are skipped.
fn parse_co_review_output(
    output: &str,
    locs: &[crate::diff::DiffLineLocation],
) -> Vec<LineComment> {
    let mut out = Vec::new();
    for raw in output.lines() {
        let line = raw.trim();
        let Some((num, text)) = line.split_once('|') else {
            continue;
        };
        // Tolerate a leading bullet / marker before the number (e.g. "- 42|…").
        let Ok(new_line) = num
            .trim()
            .trim_start_matches(['-', '*', '•', ' '])
            .trim()
            .parse::<usize>()
        else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let Some(location) = locs.iter().find(|l| l.new_line == Some(new_line)).copied() else {
            continue;
        };
        out.push(LineComment {
            location,
            start: None,
            text: text.to_string(),
            draft: true,
            suggestion: None,
        });
    }
    out
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
    use super::{
        build_pr_review, build_walkthrough_prompt, compose_feedback_log, parse_co_review_output,
        parse_review_notes,
    };
    use crate::app::LineComment;
    use crate::diff::DiffLineLocation;

    fn line_comment(new_line: Option<usize>, old_line: Option<usize>, text: &str) -> LineComment {
        LineComment {
            location: DiffLineLocation { old_line, new_line },
            start: None,
            text: text.to_string(),
            draft: false,
            suggestion: None,
        }
    }

    /// A multi-line comment spanning `start`..`end` on the current (RIGHT) side.
    fn ranged_comment(start: usize, end: usize, text: &str) -> LineComment {
        LineComment {
            location: DiffLineLocation {
                old_line: None,
                new_line: Some(end),
            },
            start: Some(DiffLineLocation {
                old_line: None,
                new_line: Some(start),
            }),
            text: text.to_string(),
            draft: false,
            suggestion: None,
        }
    }

    #[test]
    fn co_review_output_parses_findings_onto_addressable_lines() {
        let locs = vec![
            DiffLineLocation {
                old_line: Some(10),
                new_line: Some(10),
            },
            DiffLineLocation {
                old_line: None,
                new_line: Some(11),
            },
            DiffLineLocation {
                old_line: Some(12),
                new_line: None,
            },
        ];
        let output = "11|This add looks risky.\n\
             10 | context concern\n\
             - 11|tolerate a bullet prefix\n\
             999|no such line, dropped\n\
             not a finding line\n\
             11|\n";
        let drafts = parse_co_review_output(output, &locs);

        // The 999 finding has no matching new_line and the empty-text and
        // non-matching lines are skipped; the rest become draft comments.
        assert_eq!(drafts.len(), 3);
        assert!(drafts.iter().all(|c| c.draft && c.start.is_none()));
        assert_eq!(drafts[0].location.new_line, Some(11));
        assert_eq!(drafts[0].text, "This add looks risky.");
        // ` 10 | …` (spaces around the pipe) still resolves to new_line 10.
        assert_eq!(drafts[1].location.new_line, Some(10));
        assert_eq!(drafts[1].text, "context concern");
        // The leading-bullet finding still anchors to line 11.
        assert_eq!(drafts[2].location.new_line, Some(11));
    }

    #[test]
    fn pr_review_maps_lines_inline_and_folds_files_into_body() {
        let rejected = vec![
            ("src/a.rs".to_string(), "tighten this up".to_string()),
            ("src/b.rs".to_string(), String::new()),
        ];
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![
                line_comment(Some(42), None, "off-by-one"),
                line_comment(None, Some(7), "why delete this?"),
            ],
        )];
        let (body, comments) = build_pr_review(&rejected, &line_comments, "overall LGTM-ish");

        // Inline comments: an added/current line posts on RIGHT, a deletion-only
        // line on LEFT.
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].path, "src/c.rs");
        assert_eq!(comments[0].line, 42);
        assert_eq!(comments[0].side, "RIGHT");
        assert_eq!(comments[1].line, 7);
        assert_eq!(comments[1].side, "LEFT");

        // Body carries general feedback + whole-file rejections, including the
        // bare "needs revision" line when no feedback text was given.
        assert!(body.contains("overall LGTM-ish"));
        assert!(body.contains("## Files needing revision"));
        assert!(body.contains("**src/a.rs**"));
        assert!(body.contains("tighten this up"));
        assert!(body.contains("**src/b.rs** — needs revision"));
    }

    #[test]
    fn pr_review_body_empty_when_only_line_comments() {
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![line_comment(Some(3), None, "nit")],
        )];
        let (body, comments) = build_pr_review(&[], &line_comments, "");
        assert!(body.is_empty());
        assert_eq!(comments.len(), 1);
    }

    #[test]
    fn pr_review_emits_start_line_for_a_ranged_comment() {
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![ranged_comment(10, 14, "this whole block")],
        )];
        let (_, comments) = build_pr_review(&[], &line_comments, "");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].line, 14);
        assert_eq!(comments[0].side, "RIGHT");
        assert_eq!(comments[0].start_line, Some(10));
        assert_eq!(comments[0].start_side, Some("RIGHT"));
    }

    #[test]
    fn single_line_comment_has_no_start_line() {
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![line_comment(Some(5), None, "nit")],
        )];
        let (_, comments) = build_pr_review(&[], &line_comments, "");
        assert_eq!(comments[0].start_line, None);
        assert_eq!(comments[0].start_side, None);
    }

    #[test]
    fn pr_review_appends_suggestion_block_to_comment_body() {
        let mut comment = line_comment(Some(5), None, "use a guard");
        comment.suggestion = Some("let x = y?;".to_string());
        let line_comments = vec![("src/c.rs".to_string(), vec![comment])];
        let (_, comments) = build_pr_review(&[], &line_comments, "");
        assert_eq!(comments.len(), 1);
        assert_eq!(
            comments[0].body,
            "use a guard\n\n```suggestion\nlet x = y?;\n```"
        );
    }

    #[test]
    fn pr_review_suggestion_only_comment_is_just_the_block() {
        let mut comment = line_comment(Some(5), None, "");
        comment.suggestion = Some("let x = y?;".to_string());
        let line_comments = vec![("src/c.rs".to_string(), vec![comment])];
        let (_, comments) = build_pr_review(&[], &line_comments, "");
        assert_eq!(comments[0].body, "```suggestion\nlet x = y?;\n```");
    }

    #[test]
    fn anchor_label_renders_single_and_range_and_base() {
        use super::comment_anchor_label;
        assert_eq!(
            comment_anchor_label("src/a.rs", &line_comment(Some(42), None, "x")),
            "src/a.rs:42"
        );
        assert_eq!(
            comment_anchor_label("src/a.rs", &ranged_comment(10, 14, "x")),
            "src/a.rs:10-14"
        );
        assert_eq!(
            comment_anchor_label("src/a.rs", &line_comment(None, Some(7), "x")),
            "src/a.rs:7 (base)"
        );
    }

    #[test]
    fn first_round_writes_title_then_round() {
        let round = "## Review — 2026-06-25T00:00:00Z\n\nbody.\n\n";
        let out = compose_feedback_log(None, round);
        assert_eq!(
            out,
            "# Final Review Feedback\n\n## Review — 2026-06-25T00:00:00Z\n\nbody.\n\n"
        );
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
