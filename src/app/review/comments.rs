use crate::app::{
    App, AppMode, CommentAnchorContext, DiffViewerState, FileComment, LineComment,
    PendingEditorOpen, ReviewDecision, Severity,
};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Default)]
pub(super) struct SuggestionApplyReport {
    pub(super) applied: Vec<String>,
    pub(super) failures: Vec<String>,
}

#[derive(Debug)]
pub(super) struct PlannedSuggestion {
    pub(super) comment_index: usize,
    pub(super) anchor: String,
    pub(super) start_line: usize,
    pub(super) end_line: usize,
    pub(super) replacement: String,
}

#[derive(Debug, Default)]
pub(super) struct FileSuggestionApplyReport {
    pub(super) applied: Vec<(usize, String)>,
    pub(super) failures: Vec<String>,
}

pub(super) fn local_suggestion_summary(applied: &[String], failures: &[String]) -> String {
    let mut summary = String::new();
    if !applied.is_empty() {
        summary.push_str(&format!(
            ", {} suggestion(s) applied locally ({})",
            applied.len(),
            applied.join(", ")
        ));
    }
    if !failures.is_empty() {
        summary.push_str(&format!(
            ", {} suggestion(s) not applied locally",
            failures.len()
        ));
    }
    summary
}

impl App {
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
                        .map(|range| (c.text.clone(), c.severity, range))
                })
            });
            let text = if let Some((text, severity, range)) = existing {
                if state.comment_anchor.is_none() {
                    state.comment_anchor = Some(*range.start());
                    state.comment_cursor = Some(*range.end());
                }
                // Editing an existing comment resumes at its severity; a fresh
                // comment starts at the neutral default.
                state.comment_severity = severity;
                text
            } else {
                state.comment_severity = crate::app::Severity::default();
                String::new()
            };
            state.reset_feedback_editor(text);
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
                let overlapping = comments.iter().find(|c| {
                    c.covered_indices(&locs)
                        .map(|r| !(*r.end() < lo || *r.start() > hi))
                        .unwrap_or(false)
                });
                let suggestion = overlapping.and_then(|c| c.suggestion.clone());
                // Likewise its round of origin: editing a thread inherited from a
                // previous round must not re-brand it as freshly authored.
                let carried = overlapping.is_some_and(|c| c.carried);
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
                        severity: state.comment_severity,
                        // Captured by `recapture_anchor_contexts` on the persist
                        // that immediately follows this mutation.
                        anchor_context: None,
                        start_anchor_context: None,
                        anchor_lost: false,
                        // Writing on a thread re-opens it: fresh prose means the
                        // reviewer has more to say, settled or not.
                        resolved: false,
                        carried,
                    });
                    comments.sort_by_key(|c| {
                        let loc = c.start.unwrap_or(c.location);
                        loc.new_line.or(loc.old_line).unwrap_or(0)
                    });
                }
            }
            state.editing_line_comment = false;
            state.comment_anchor = None;
            state.reset_feedback_editor(String::new());
        }
        if let Some(path) = commented_path {
            self.diff_review_sync_auto_reject(&path);
        }
        self.persist_review_progress();
    }

    /// Reconcile a file's implicit "needs revision" verdict with its open
    /// threads (kept, non-draft, unresolved comments): an open thread is a file
    /// that needs work, so the first one defaults an undecided file to `Reject`
    /// with empty feedback (the comments carry the specifics), and a file with
    /// no more open threads — every comment removed or resolved — clears a
    /// rejection that was auto-set this way. An explicit verdict —
    /// approve/skip/reject, or a carried-over approval — is never overridden.
    /// Called after every comment mutation, before the progress persist, so
    /// what's on disk always reflects the synced verdict.
    pub(super) fn diff_review_sync_auto_reject(&mut self, path: &str) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let has_kept = state
                .line_comments
                .get(path)
                .is_some_and(|comments| comments.iter().any(|c| c.is_open_thread()));
            if has_kept {
                if !state.decisions.contains_key(path) {
                    state.decisions.insert(
                        path.to_string(),
                        ReviewDecision::Reject {
                            feedback: String::new(),
                            // The file's real severity lives on its line comments;
                            // the auto-rejection verdict itself stays neutral.
                            severity: crate::app::Severity::default(),
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
                        .map(|range| (c.suggestion.clone(), c.severity, range))
                })
            });
            let prefill = match existing {
                Some((suggestion, severity, range)) => {
                    if state.comment_anchor.is_none() {
                        state.comment_anchor = Some(*range.start());
                        state.comment_cursor = Some(*range.end());
                    }
                    // Carry the existing comment's severity onto the (re)written
                    // suggestion; the suggestion editor doesn't cycle it.
                    state.comment_severity = severity;
                    // Existing suggestion: edit it. Otherwise seed from the span's
                    // current code.
                    suggestion.unwrap_or_else(|| span_current_text(&texts, &range))
                }
                None => {
                    state.comment_severity = crate::app::Severity::default();
                    let lo = state.comment_anchor.unwrap_or(cur).min(cur);
                    let hi = state.comment_anchor.unwrap_or(cur).max(cur);
                    span_current_text(&texts, &(lo..=hi))
                }
            };
            state.reset_feedback_editor(prefill);
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
                // Preserve the prose (and severity) of an existing comment on the
                // span; a fresh suggestion-only comment carries empty prose and
                // the composed default severity.
                let existing = comments.iter().find(|c| {
                    c.covered_indices(&locs)
                        .map(|r| !(*r.end() < lo || *r.start() > hi))
                        .unwrap_or(false)
                });
                let prose = existing.map(|c| c.text.clone()).unwrap_or_default();
                let severity = existing
                    .map(|c| c.severity)
                    .unwrap_or(state.comment_severity);
                let carried = existing.is_some_and(|c| c.carried);
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
                        severity,
                        anchor_context: None,
                        start_anchor_context: None,
                        anchor_lost: false,
                        // Attaching a suggestion re-opens a settled thread, as
                        // writing prose on it does.
                        resolved: false,
                        carried,
                    });
                    comments.sort_by_key(|c| {
                        let loc = c.start.unwrap_or(c.location);
                        loc.new_line.or(loc.old_line).unwrap_or(0)
                    });
                }
            }
            state.editing_suggestion = false;
            state.comment_anchor = None;
            state.reset_feedback_editor(String::new());
        }
        if let Some(path) = commented_path {
            self.diff_review_sync_auto_reject(&path);
        }
        self.persist_review_progress();
    }

    /// Apply the kept suggestion under the line cursor directly to the
    /// worktree. The write is guarded against a dirty/stale file and an anchor
    /// that no longer matches. A successful application settles the thread,
    /// removes the now-consumed suggestion block, and refreshes the diff.
    pub fn diff_review_apply_suggestion_under_cursor(&mut self) {
        let selected = match &self.mode {
            AppMode::DiffViewer(state) if state.review => {
                let Some(cursor) = state.comment_cursor else {
                    self.message = Some("Activate the line cursor on a suggestion first".into());
                    return;
                };
                let Some(file) = state.files.get(state.selected_file) else {
                    return;
                };
                let locations = file.addressable_lines();
                state.line_comments.get(&file.path).and_then(|comments| {
                    comments.iter().enumerate().find_map(|(index, comment)| {
                        (comment.is_open_thread()
                            && comment.suggestion.is_some()
                            && comment
                                .covered_indices(&locations)
                                .is_some_and(|range| range.contains(&cursor)))
                        .then_some((file.path.clone(), index))
                    })
                })
            }
            _ => return,
        };
        let Some((path, index)) = selected else {
            self.message = Some("No open suggested change under the cursor".to_string());
            return;
        };

        let report = self.apply_review_suggestion_jobs(Some((&path, index)));
        if report.applied.is_empty() {
            self.message = Some(format!(
                "Suggestion not applied: {}",
                report
                    .failures
                    .first()
                    .map(String::as_str)
                    .unwrap_or("the suggestion is no longer applicable")
            ));
        } else {
            self.message = Some(format!(
                "Applied suggestion locally: {}",
                report.applied.join(", ")
            ));
        }
    }

    /// Toggle the explicit opt-in to apply all remaining suggestions immediately
    /// before the finish-time check command. No suggestions are ever written by
    /// merely pressing `q` unless this has been enabled.
    pub fn diff_review_toggle_apply_suggestions_on_finish(&mut self) {
        let message = if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let pending = state.pending_suggestion_count();
            if pending == 0 {
                state.apply_suggestions_on_finish = false;
                "No open suggestions to apply".to_string()
            } else {
                state.apply_suggestions_on_finish = !state.apply_suggestions_on_finish;
                if state.apply_suggestions_on_finish {
                    format!("Will apply {pending} suggestion(s) locally when finishing")
                } else {
                    "Suggestions will be sent to the fixing agent without local application"
                        .to_string()
                }
            }
        } else {
            return;
        };
        self.persist_review_progress();
        self.message = Some(message);
    }

    /// Apply either one requested suggestion or every open suggestion. Groups
    /// work by file so multiple replacements are validated against one reviewed
    /// snapshot and committed in one bottom-up write. Successful comment indices
    /// are then settled in live review state before the diff is refreshed.
    pub(super) fn apply_review_suggestion_jobs(
        &mut self,
        only: Option<(&str, usize)>,
    ) -> SuggestionApplyReport {
        let (workdir, jobs) = match &self.mode {
            AppMode::DiffViewer(state) if state.review => {
                let jobs = state
                    .files
                    .iter()
                    .filter_map(|file| {
                        let comments = state.line_comments.get(&file.path)?;
                        let selected: Vec<(usize, LineComment)> = comments
                            .iter()
                            .enumerate()
                            .filter(|(index, comment)| {
                                comment.is_open_thread()
                                    && comment.suggestion.is_some()
                                    && only.is_none_or(|(path, wanted)| {
                                        file.path == path && *index == wanted
                                    })
                            })
                            .map(|(index, comment)| (index, comment.clone()))
                            .collect();
                        (!selected.is_empty()).then_some((file.clone(), selected))
                    })
                    .collect::<Vec<_>>();
                (state.workdir.clone(), jobs)
            }
            _ => return SuggestionApplyReport::default(),
        };

        let mut per_file = Vec::new();
        let mut report = SuggestionApplyReport::default();
        for (file, comments) in jobs {
            let file_report = apply_suggestions_to_file(&workdir, &file, &comments);
            report
                .applied
                .extend(file_report.applied.iter().map(|(_, anchor)| anchor.clone()));
            report.failures.extend(file_report.failures.clone());
            per_file.push((file.path, file_report));
        }

        let mut changed_paths = Vec::new();
        if let AppMode::DiffViewer(state) = &mut self.mode {
            for (path, file_report) in &per_file {
                if file_report.applied.is_empty() {
                    continue;
                }
                let applied_indices: std::collections::HashSet<usize> = file_report
                    .applied
                    .iter()
                    .map(|(index, _)| *index)
                    .collect();
                let remove_comment_entry = if let Some(comments) = state.line_comments.get_mut(path)
                {
                    for (index, comment) in comments.iter_mut().enumerate() {
                        if applied_indices.contains(&index) {
                            comment.suggestion = None;
                            comment.resolved = true;
                        }
                    }
                    // A suggestion-only comment has no conversation left once
                    // applied; prose comments remain as settled threads/history.
                    comments.retain(|comment| {
                        !comment.text.trim().is_empty() || comment.suggestion.is_some()
                    });
                    comments.is_empty()
                } else {
                    false
                };
                if remove_comment_entry {
                    state.line_comments.remove(path);
                }
                changed_paths.push(path.clone());
            }
            state
                .applied_suggestions
                .extend(report.applied.iter().cloned());
        }
        for path in &changed_paths {
            self.diff_review_sync_auto_reject(path);
        }
        self.persist_review_progress();

        if !report.applied.is_empty() {
            // Re-load immediately so subsequent comments and the finish snapshot
            // use the source that was actually written, not the pre-apply patch.
            self.refresh_diff_viewer();
            self.complete_diff_viewer_loading();
            self.persist_review_progress();
        }
        report
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

    /// Toggle the resolved state of the kept (non-draft) comment under the line
    /// cursor — the reviewer marking a conversation settled, or re-opening one
    /// they marked too soon. Returns `true` if a comment was toggled.
    pub fn diff_review_toggle_resolved(&mut self) -> bool {
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
                    !c.draft
                        && c.covered_indices(&locs)
                            .is_some_and(|range| range.contains(&cur))
                })
            }) {
                Some(comment) => {
                    comment.resolved = !comment.resolved;
                    Some((path, comment.resolved))
                }
                None => None,
            }
        } else {
            None
        };
        match acted {
            Some((path, resolved)) => {
                self.message = Some(
                    if resolved {
                        "Thread resolved"
                    } else {
                        "Thread re-opened"
                    }
                    .to_string(),
                );
                // A resolved thread is settled, so it can't be the reason a file
                // stays auto-rejected; re-opening one can put that back.
                self.diff_review_sync_auto_reject(&path);
                self.persist_review_progress();
                true
            }
            None => false,
        }
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

    /// Every line comment across the *visible* files as `(file index, first
    /// covered line index)` pairs, in file order and then in diff-line order —
    /// the itinerary `{` / `}` walk. Also reports how many comments had no
    /// anchor to park on, so an all-lost file can say why it went nowhere.
    ///
    /// `addressable_lines()` is only computed for files that actually carry a
    /// comment, so a large changeset with a handful of annotations stays cheap.
    pub(super) fn review_comment_stops(state: &DiffViewerState) -> (Vec<(usize, usize)>, usize) {
        let mut stops: Vec<(usize, usize)> = Vec::new();
        let mut lost = 0usize;
        for file_idx in state.visible_file_indices() {
            let Some(file) = state.files.get(file_idx) else {
                continue;
            };
            let Some(comments) = state.line_comments.get(&file.path) else {
                continue;
            };
            if comments.is_empty() {
                continue;
            }
            let locs = file.addressable_lines();
            let mut indices: Vec<usize> = Vec::new();
            for comment in comments {
                // A comment whose anchor no longer resolves (moved code, a
                // reload) has no line to put the cursor on — count it rather
                // than pretending it isn't there.
                match comment.covered_indices(&locs) {
                    Some(range) => indices.push(*range.start()),
                    None => lost += 1,
                }
            }
            indices.sort_unstable();
            indices.dedup();
            stops.extend(indices.into_iter().map(|idx| (file_idx, idx)));
        }
        (stops, lost)
    }

    /// Move the line cursor to the next (`dir >= 0`) or previous comment
    /// anywhere in the review, wrapping at either end. Unlike `Tab` — which
    /// cycles the AI's *drafts* within the current file — this walks every
    /// comment (draft or kept) across every visible file, so a reviewer can
    /// sweep their whole annotation set before finishing without re-finding
    /// each file by hand.
    pub fn diff_review_jump_comment(&mut self, dir: isize) {
        let message = match &mut self.mode {
            AppMode::DiffViewer(state) if state.review => {
                let (stops, lost) = Self::review_comment_stops(state);
                if stops.is_empty() {
                    Some(if lost > 0 {
                        format!("No comment anchors to jump to ({lost} lost their anchor)")
                    } else {
                        "No comments in this review yet".to_string()
                    })
                } else {
                    let sel = state.selected_file;
                    // With the cursor off, forward starts before the current file's
                    // first comment and backward after its last, so the first press
                    // lands inside the file the reviewer is already looking at.
                    let cursor = state.comment_cursor.map(|c| c as isize);
                    let target_pos = if dir >= 0 {
                        let from = cursor.unwrap_or(-1);
                        stops
                            .iter()
                            .position(|&(f, i)| f > sel || (f == sel && (i as isize) > from))
                            .unwrap_or(0)
                    } else {
                        let from = cursor.unwrap_or(isize::MAX);
                        stops
                            .iter()
                            .rposition(|&(f, i)| f < sel || (f == sel && (i as isize) < from))
                            .unwrap_or(stops.len() - 1)
                    };
                    let (file_idx, line_idx) = stops[target_pos];
                    if state.selected_file != file_idx {
                        state.selected_file = file_idx;
                        state.on_file_changed();
                    }
                    // Set after `on_file_changed` (which would otherwise reset the
                    // cursor to line 0), and unconditionally, so jumping to a
                    // comment also turns the cursor on when it was off.
                    state.comment_cursor = Some(line_idx);
                    state.comment_anchor = None;
                    state.cursor_sync_to_view = true;

                    let anchor = state
                        .files
                        .get(file_idx)
                        .and_then(|file| {
                            let locs = file.addressable_lines();
                            let comment =
                                state.line_comments.get(&file.path)?.iter().find(|c| {
                                    c.covered_indices(&locs)
                                        .is_some_and(|range| range.contains(&line_idx))
                                })?;
                            Some(comment_anchor_label(&file.path, comment))
                        })
                        .unwrap_or_default();
                    let lost_note = if lost > 0 {
                        format!(" ({lost} anchor-lost skipped)")
                    } else {
                        String::new()
                    };
                    Some(format!(
                        "Comment {}/{} — {anchor}{lost_note}",
                        target_pos + 1,
                        stops.len()
                    ))
                }
            }
            _ => None,
        };
        if let Some(message) = message {
            self.message = Some(message);
        }
    }

    /// Open the current file in `$VISUAL`/`$EDITOR`, at the cursored line when
    /// the line cursor is active. Only resolves and validates the target here —
    /// the actual suspend/run/restore is the main loop's job, since it owns the
    /// terminal state (`PendingEditorOpen`).
    pub fn diff_review_open_in_editor(&mut self) {
        let resolved = match &self.mode {
            AppMode::DiffViewer(state) => {
                let Some(file) = state.files.get(state.selected_file) else {
                    self.message = Some("No file to open".to_string());
                    return;
                };
                // A deletion has no file on disk, and a binary one has nothing
                // an editor can usefully show — say so rather than failing on
                // the filesystem check with a vaguer message.
                if matches!(file.status, crate::diff::DiffFileStatus::Deleted) {
                    self.message = Some(format!("{} was deleted — nothing to open", file.path));
                    return;
                }
                if file.is_binary {
                    self.message = Some(format!("{} is binary — nothing to edit", file.path));
                    return;
                }
                match guarded_worktree_file(&state.workdir, &file.path) {
                    Ok(path) => Ok(PendingEditorOpen {
                        path,
                        workdir: state.workdir.clone(),
                        line: editor_target_line(file, state.comment_cursor),
                        display: file.path.clone(),
                    }),
                    Err(reason) => Err(format!("Cannot open {}: {reason}", file.path)),
                }
            }
            _ => return,
        };
        match resolved {
            Ok(request) => self.pending_editor = Some(request),
            Err(message) => self.message = Some(message),
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
                    ReviewDecision::Reject { feedback, severity } => {
                        Some((feedback.clone(), *severity))
                    }
                    ReviewDecision::Approve => None,
                });
            // Resume an existing rejection's severity; a fresh rejection defaults
            // to Blocker — rejecting a file outright is a must-fix signal.
            let (text, severity) = existing.unwrap_or((String::new(), Severity::Blocker));
            state.comment_severity = severity;
            state.reset_feedback_editor(text);
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
            state.reset_feedback_editor(state.general_feedback.clone());
            state.feedback_scroll = 0;
            state.feedback_sync_to_cursor = true;
            state.editing_general = true;
            state.feedback_editing = false;
        }
    }

    /// Edit a verdict-free comment anchored to the current file. A file
    /// comment is deliberately independent of approve/reject and therefore
    /// never participates in the line-comment auto-reject rule.
    pub fn diff_review_start_file_comment(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.review {
                return;
            }
            let Some(file) = state.files.get(state.selected_file) else {
                return;
            };
            let existing = state.file_comments.get(&file.path);
            state.comment_severity = existing.map(|c| c.severity).unwrap_or_default();
            state.reset_feedback_editor(existing.map(|c| c.text.clone()).unwrap_or_default());
            state.feedback_scroll = 0;
            state.feedback_sync_to_cursor = true;
            state.editing_file_comment = true;
            state.feedback_editing = false;
            state.editing_general = false;
            state.editing_line_comment = false;
            state.editing_suggestion = false;
        }
    }

    /// Store the current file comment; an empty editor deletes it. Editing a
    /// carried or resolved thread re-opens it while retaining its round origin.
    pub fn diff_review_submit_file_comment(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.editing_file_comment {
                return;
            }
            let text = state.feedback_editor.text().trim().to_string();
            if let Some(file) = state.files.get(state.selected_file) {
                let path = file.path.clone();
                if text.is_empty() {
                    state.file_comments.remove(&path);
                } else {
                    let carried = state.file_comments.get(&path).is_some_and(|c| c.carried);
                    state.file_comments.insert(
                        path,
                        FileComment {
                            text,
                            severity: state.comment_severity,
                            resolved: false,
                            carried,
                        },
                    );
                }
            }
            state.editing_file_comment = false;
            state.reset_feedback_editor(String::new());
        }
        self.persist_review_progress();
    }

    /// Resolve or re-open the current file's whole-file thread.
    pub fn diff_review_toggle_file_comment_resolved(&mut self) -> bool {
        let resolved = if let AppMode::DiffViewer(state) = &mut self.mode {
            let Some(file) = state.files.get(state.selected_file) else {
                return false;
            };
            let Some(comment) = state.file_comments.get_mut(&file.path) else {
                self.message = Some("No file comment on this file".to_string());
                return false;
            };
            comment.resolved = !comment.resolved;
            Some(comment.resolved)
        } else {
            None
        };
        if let Some(resolved) = resolved {
            self.message = Some(if resolved {
                "File comment resolved".to_string()
            } else {
                "File comment re-opened".to_string()
            });
            self.persist_review_progress();
            true
        } else {
            false
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
            state.reset_feedback_editor(String::new());
        }
        self.persist_review_progress();
    }

    pub fn diff_review_cancel_feedback(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.feedback_editing = false;
            state.editing_general = false;
            state.editing_line_comment = false;
            state.editing_file_comment = false;
            state.editing_suggestion = false;
            state.reset_feedback_editor(String::new());
        }
    }

    /// Toggle Vim for every comment and suggestion editor in the current
    /// review session. The active editor's text survives the keymap reset;
    /// future editors inherit the same transient preference.
    pub fn diff_review_toggle_vim(&mut self) {
        let enabled = match &mut self.mode {
            AppMode::DiffViewer(state) if state.review => state.toggle_feedback_vim(),
            _ => return,
        };
        self.message = Some(if enabled {
            "Review editor Vim mode enabled · Normal".to_string()
        } else {
            "Review editor Vim mode disabled".to_string()
        });
    }

    /// Record the typed feedback as a rejection for the current file, then
    /// advance to the next file.
    pub fn diff_review_submit_feedback(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            if !state.feedback_editing {
                return;
            }
            let feedback = state.feedback_editor.text().trim().to_string();
            let severity = state.comment_severity;
            if let Some(path) = state.files.get(state.selected_file).map(|f| f.path.clone()) {
                let decision = ReviewDecision::Reject { feedback, severity };
                state.push_verdict_undo(&path, Some(&decision));
                state.decisions.insert(path.clone(), decision);
                // The reviewer typed this rejection themselves: it is explicit
                // now and no longer tracks the file's comments.
                state.auto_rejected.remove(&path);
            }
            state.feedback_editing = false;
            state.reset_feedback_editor(String::new());
        }
        self.diff_review_advance();
        self.persist_review_progress();
    }
}

/// Join the current text of the addressable lines in `range` (indices into
/// `addressable_line_texts()`) with newlines. Out-of-range indices are skipped.
/// Used to seed a suggested-change editor with the span's current code.
pub(super) fn span_current_text(
    texts: &[String],
    range: &std::ops::RangeInclusive<usize>,
) -> String {
    range
        .clone()
        .filter_map(|i| texts.get(i).cloned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Byte bounds of an inclusive, one-based line span. The returned end includes
/// the final line ending when one exists, which lets a replacement preserve the
/// file's existing EOF/newline shape.
pub(super) fn content_line_bounds(
    content: &str,
    start_line: usize,
    end_line: usize,
) -> Option<(usize, usize)> {
    if start_line == 0 || end_line < start_line {
        return None;
    }
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for (idx, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            ranges.push((start, idx + 1));
            start = idx + 1;
        }
    }
    if start < content.len() {
        ranges.push((start, content.len()));
    }
    let first = ranges.get(start_line - 1)?.0;
    let last = ranges.get(end_line - 1)?.1;
    Some((first, last))
}

pub(super) fn line_text_without_ending(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

/// Validate that a suggestion still points at a contiguous current-side span
/// and that the diff text under the anchor exactly matches the reviewed file
/// content. Deletion-side and mixed old/new ranges are intentionally refused:
/// they do not describe an unambiguous replacement in the worktree file.
pub(super) fn plan_local_suggestion(
    file: &crate::diff::DiffFile,
    comment_index: usize,
    comment: &LineComment,
) -> std::result::Result<PlannedSuggestion, String> {
    let anchor = comment_anchor_label(&file.path, comment);
    if comment.anchor_lost {
        return Err("anchor is no longer present in the current diff".to_string());
    }
    let replacement = comment
        .suggestion
        .clone()
        .ok_or_else(|| "comment has no suggested replacement".to_string())?;
    let content = file
        .new_content
        .as_deref()
        .ok_or_else(|| "file has no current text content".to_string())?;
    let locations = file.addressable_lines();
    let texts = file.addressable_line_texts();
    let covered = comment
        .covered_indices(&locations)
        .ok_or_else(|| "suggestion span no longer resolves in the diff".to_string())?;

    let mut new_lines = Vec::new();
    let mut reviewed_lines = Vec::new();
    for idx in covered {
        let line = locations
            .get(idx)
            .and_then(|location| location.new_line)
            .ok_or_else(|| {
                "suggestion includes a deletion-only line and cannot be applied locally".to_string()
            })?;
        new_lines.push(line);
        let reviewed = texts.get(idx).map(String::as_str).unwrap_or_default();
        reviewed_lines.push(reviewed.strip_suffix('\r').unwrap_or(reviewed).to_string());
    }
    if new_lines
        .windows(2)
        .any(|pair| pair[1] != pair[0].saturating_add(1))
    {
        return Err("suggestion span is not contiguous in the current file".to_string());
    }
    let start_line = *new_lines
        .first()
        .ok_or_else(|| "suggestion span is empty".to_string())?;
    let end_line = *new_lines.last().unwrap_or(&start_line);
    let (start, end) = content_line_bounds(content, start_line, end_line)
        .ok_or_else(|| "suggestion span falls outside the current file".to_string())?;
    let current_lines: Vec<&str> = content[start..end]
        .split_inclusive('\n')
        .map(line_text_without_ending)
        .collect();
    if current_lines.len() != reviewed_lines.len()
        || current_lines
            .iter()
            .zip(&reviewed_lines)
            .any(|(current, reviewed)| *current != reviewed)
    {
        return Err("the anchored lines no longer match the reviewed diff".to_string());
    }

    Ok(PlannedSuggestion {
        comment_index,
        anchor,
        start_line,
        end_line,
        replacement,
    })
}

pub(super) fn replacement_with_preserved_line_endings(replacement: &str, replaced: &str) -> String {
    let line_ending = if replaced.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut normalized = replacement.replace("\r\n", "\n");
    if line_ending == "\r\n" {
        normalized = normalized.replace('\n', "\r\n");
    }
    if replaced.ends_with('\n') && !normalized.ends_with(line_ending) {
        normalized.push_str(line_ending);
    }
    normalized
}

pub(super) fn replace_content_line_span(
    content: &mut String,
    start_line: usize,
    end_line: usize,
    replacement: &str,
) -> std::result::Result<(), String> {
    let (start, end) = content_line_bounds(content, start_line, end_line)
        .ok_or_else(|| "suggestion span falls outside the current file".to_string())?;
    let replacement = replacement_with_preserved_line_endings(replacement, &content[start..end]);
    content.replace_range(start..end, &replacement);
    Ok(())
}

pub(super) fn guarded_worktree_file(
    workdir: &Path,
    relative: &str,
) -> std::result::Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err("file path is not a safe worktree-relative path".to_string());
    }
    let path = workdir.join(relative);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|err| format!("could not inspect file: {err}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("target is not a regular worktree file".to_string());
    }
    let root = std::fs::canonicalize(workdir)
        .map_err(|err| format!("could not resolve worktree path: {err}"))?;
    let resolved = std::fs::canonicalize(&path)
        .map_err(|err| format!("could not resolve file path: {err}"))?;
    if !resolved.starts_with(&root) {
        return Err("file resolves outside the worktree".to_string());
    }
    Ok(path)
}

/// The 1-based line in the file *on disk* that the editor should open at.
///
/// The cursor indexes `addressable_lines()`, which includes removed lines that
/// no longer exist in the working copy. For one of those, land on the nearest
/// surviving line above (else below) so the editor still arrives at the change
/// instead of the top of the file. With the cursor off, this resolves to the
/// first line of the first hunk.
pub(super) fn editor_target_line(
    file: &crate::diff::DiffFile,
    cursor: Option<usize>,
) -> Option<usize> {
    let locations = file.addressable_lines();
    if locations.is_empty() {
        return None;
    }
    let start = cursor.unwrap_or(0).min(locations.len() - 1);
    if let Some(line) = locations[start].new_line {
        return Some(line);
    }
    locations[..start]
        .iter()
        .rev()
        .find_map(|location| location.new_line)
        .or_else(|| {
            locations[start + 1..]
                .iter()
                .find_map(|location| location.new_line)
        })
}

/// GUI editors fork and return immediately unless told to block, which would
/// drop the reviewer back into a redrawn TUI with the file still open
/// elsewhere. Only editors that actually accept `--wait` are listed.
pub(super) fn editor_needs_wait(stem: &str) -> bool {
    matches!(
        stem,
        "code"
            | "code-insiders"
            | "codium"
            | "vscodium"
            | "cursor"
            | "windsurf"
            | "subl"
            | "sublime_text"
            | "zed"
    )
}

/// Build the `(program, args)` to run for an `$EDITOR` value, placing the
/// cursor on `line` where the editor is known to support it.
///
/// `editor` is split on whitespace so a value carrying its own flags
/// (`code --wait`, `emacsclient -nw`) works; full shell quoting is deliberately
/// not emulated. An editor we don't recognise is opened at the top of the file
/// rather than guessing a flag it would read as a second filename.
pub(super) fn editor_invocation(
    editor: &str,
    path: &str,
    line: Option<usize>,
) -> Option<(String, Vec<String>)> {
    let mut parts = editor.split_whitespace();
    let program = parts.next()?.to_string();
    let mut args: Vec<String> = parts.map(str::to_string).collect();

    let stem = Path::new(&program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if editor_needs_wait(&stem) && !args.iter().any(|arg| arg == "--wait" || arg == "-w") {
        args.push("--wait".to_string());
    }

    match line {
        // `+N file` — the convention vi and its descendants share.
        Some(line)
            if matches!(
                stem.as_str(),
                "vi" | "vim"
                    | "nvim"
                    | "view"
                    | "gvim"
                    | "mvim"
                    | "nano"
                    | "pico"
                    | "emacs"
                    | "emacsclient"
                    | "joe"
                    | "jed"
                    | "mg"
                    | "ne"
                    | "kak"
                    | "micro"
            ) =>
        {
            args.push(format!("+{line}"));
            args.push(path.to_string());
        }
        // VS Code and its forks need an explicit --goto.
        Some(line)
            if matches!(
                stem.as_str(),
                "code" | "code-insiders" | "codium" | "vscodium" | "cursor" | "windsurf"
            ) =>
        {
            args.push("--goto".to_string());
            args.push(format!("{path}:{line}"));
        }
        // `file:line` as a single argument.
        Some(line)
            if matches!(
                stem.as_str(),
                "hx" | "helix" | "subl" | "sublime_text" | "zed"
            ) =>
        {
            args.push(format!("{path}:{line}"));
        }
        _ => args.push(path.to_string()),
    }

    Some((program, args))
}

/// Resolve the editor to run: `$VISUAL`, then `$EDITOR`, then `vi` — the
/// standard precedence, so AMF honours whatever the reviewer's shell already
/// configures. Blank values are ignored rather than treated as a program name.
pub fn resolve_editor_command(
    path: &str,
    line: Option<usize>,
) -> std::result::Result<(String, Vec<String>), String> {
    let configured = ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|name| match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => Some(value),
            _ => None,
        })
        .unwrap_or_else(|| "vi".to_string());

    editor_invocation(&configured, path, line)
        .ok_or_else(|| format!("$EDITOR ({configured}) is not a runnable command"))
}

/// Apply a set of suggestions for one file in a single write. The whole-file
/// equality check is the dirty-file guard; bottom-up replacements keep the
/// original line coordinates valid when earlier suggestions add/remove lines.
pub(super) fn apply_suggestions_to_file(
    workdir: &Path,
    file: &crate::diff::DiffFile,
    comments: &[(usize, LineComment)],
) -> FileSuggestionApplyReport {
    let mut report = FileSuggestionApplyReport::default();
    let fail_all = |reason: String, report: &mut FileSuggestionApplyReport| {
        report.failures.extend(comments.iter().map(|(_, comment)| {
            format!("{}: {reason}", comment_anchor_label(&file.path, comment))
        }));
    };

    let Some(reviewed_content) = file.new_content.as_deref() else {
        fail_all("file has no current text content".to_string(), &mut report);
        return report;
    };
    let path = match guarded_worktree_file(workdir, &file.path) {
        Ok(path) => path,
        Err(reason) => {
            fail_all(reason, &mut report);
            return report;
        }
    };
    let live_content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => {
            fail_all(format!("could not read file: {err}"), &mut report);
            return report;
        }
    };
    if live_content != reviewed_content {
        fail_all(
            "file changed since the diff was loaded; refresh before applying".to_string(),
            &mut report,
        );
        return report;
    }

    let mut plans = Vec::new();
    for (index, comment) in comments {
        match plan_local_suggestion(file, *index, comment) {
            Ok(plan) => plans.push(plan),
            Err(reason) => report.failures.push(format!(
                "{}: {reason}",
                comment_anchor_label(&file.path, comment)
            )),
        }
    }
    plans.sort_by_key(|plan| (plan.start_line, plan.end_line));
    let mut accepted: Vec<PlannedSuggestion> = Vec::new();
    for plan in plans {
        if let Some(previous) = accepted.last()
            && plan.start_line <= previous.end_line
        {
            report.failures.push(format!(
                "{}: suggestion overlaps another local suggestion",
                plan.anchor
            ));
        } else {
            accepted.push(plan);
        }
    }

    let mut updated = live_content;
    for plan in accepted.iter().rev() {
        if let Err(reason) = replace_content_line_span(
            &mut updated,
            plan.start_line,
            plan.end_line,
            &plan.replacement,
        ) {
            report.failures.push(format!("{}: {reason}", plan.anchor));
            return report;
        }
    }
    if accepted.is_empty() {
        return report;
    }
    if let Err(err) = std::fs::write(&path, updated) {
        report.failures.extend(
            accepted
                .iter()
                .map(|plan| format!("{}: could not write file: {err}", plan.anchor)),
        );
        return report;
    }
    report.applied = accepted
        .into_iter()
        .map(|plan| (plan.comment_index, plan.anchor))
        .collect();
    report
}

/// Re-locate one file's line comments against its freshly-loaded diff, in place.
/// Returns `(moved, lost)`: how many comments were re-anchored to a new line,
/// and how many newly lost their anchor.
///
/// A comment whose anchors still resolve exactly is left alone (and un-flagged).
/// Otherwise its captured context snippet is fuzzy-matched against the new diff.
/// A range whose `start` can no longer be found degrades to a single-line comment
/// on the re-found end rather than inventing a span. A comment with no snippet,
/// or whose snippet matches nowhere / ambiguously, is flagged `anchor_lost`.
pub(super) fn reanchor_file_comments(
    file: &crate::diff::DiffFile,
    comments: &mut [LineComment],
) -> (usize, usize) {
    let locs = file.addressable_lines();
    let texts = file.addressable_line_texts();
    let (mut moved, mut lost) = (0usize, 0usize);
    for comment in comments.iter_mut() {
        // A line number can remain present after an edit while now pointing at
        // different text (especially when a local suggestion adds/removes
        // lines). When a context snippet exists, require its anchor text to
        // still match too; otherwise fall through to the fuzzy relocation pass.
        let location_still_matches =
            |location: crate::diff::DiffLineLocation, context: Option<&CommentAnchorContext>| {
                locs.iter()
                    .position(|candidate| *candidate == location)
                    .is_some_and(|idx| {
                        context.is_none_or(|context| {
                            texts
                                .get(idx)
                                .is_some_and(|text| text.trim() == context.line.trim())
                        })
                    })
            };
        let end_ok = location_still_matches(comment.location, comment.anchor_context.as_ref());
        let start_ok = comment.start.is_none_or(|start| {
            location_still_matches(start, comment.start_anchor_context.as_ref())
        });
        if end_ok && start_ok {
            comment.anchor_lost = false;
            continue;
        }
        let relocate = |ctx: Option<&CommentAnchorContext>| {
            ctx.and_then(|ctx| ctx.best_match(&texts))
                .and_then(|idx| locs.get(idx).copied())
        };
        // The end anchor drives the comment; without it there is nothing to
        // re-attach to. Keep it untouched when it still resolves — only a moved
        // anchor needs the fuzzy match.
        let end = if end_ok {
            Some(comment.location)
        } else {
            relocate(comment.anchor_context.as_ref())
        };
        let Some(end) = end else {
            if !comment.anchor_lost {
                lost += 1;
            }
            comment.anchor_lost = true;
            continue;
        };
        // Re-find the span start too; if it's gone, keep the comment as a
        // single-line note on the re-found end rather than guess a span.
        let start = match comment.start {
            None => None,
            Some(start) if start_ok => Some(start),
            Some(_) => relocate(comment.start_anchor_context.as_ref()),
        };
        comment.start = start.filter(|start| *start != end);
        comment.location = end;
        comment.anchor_lost = false;
        moved += 1;
    }
    (moved, lost)
}

/// The `file:line` (or `file:start-end`) heading for a line comment in the
/// feedback file. A base-side (deletion-only) line is tagged `(base)`. A comment
/// whose anchor could not be re-located after the diff changed carries a stale
/// line number, so it is labelled by file alone — the reviewer (and the agent)
/// are told the line is gone rather than pointed at the wrong one.
pub(super) fn comment_anchor_label(file: &str, comment: &LineComment) -> String {
    if comment.anchor_lost {
        return format!("{file} (anchor lost — possibly addressed)");
    }
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
