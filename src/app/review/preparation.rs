use super::comments::reanchor_file_comments;
use super::headless::CheckOutcome;
use crate::app::{
    App, AppMode, CommentAnchorContext, DiffViewerState, FileComment, FileFilter, LineComment,
    ReviewDecision, ReviewHistoryRound,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Label (and de-facto identity) of the dedicated final-review agent session.
/// Found-or-created by this label so re-running the review reuses the same
/// window. Kept distinct from PR Triage's "PR Triage" session so the two
/// dedicated targets never collide on one feature.
pub(crate) const FINAL_REVIEW_SESSION_LABEL: &str = "Final Review";

/// The prompt dispatched to the agent after a review finishes, asking it to act
/// on the feedback file's most recent round.
pub(super) const REVIEW_FEEDBACK_PROMPT: &str = "A reviewer left feedback on these changes in \
     .claude/final-review-feedback.md. Read that file and address every item in the most recent \
     review round (the first \"## Review\" section); earlier sections are prior rounds kept for \
     history. Each item is tagged with a severity in brackets: [blocker] must be fixed, \
     [suggestion] and [nit] are improvements worth making, [question] wants an answer (not \
     necessarily a code change), and [praise] needs no action. Prioritize the blockers. \
     After you address an item, append a reply directly under it in that same file on its own \
     line, starting with \"**Agent:** \" — say what you changed (e.g. \"fixed in src/foo.rs\") or, \
     if you disagree or are answering a [question], why. Keep each reply to a sentence or two. \
     These replies are shown to the reviewer beside your changes on the next review round.";

/// Cap on how many files' notes stay in `.claude/review-notes.md`. In Review
/// Mode the feature agent blind-appends sections and never reads the file back
/// (`ensure_review_claude_md`); AMF collapses it to the newest note per file
/// after every agent turn, keeping only the `MAX_LIVE_REVIEW_NOTE_FILES` most
/// recently documented. Older and superseded sections remain available to AMF
/// in the archive.
pub(super) const MAX_LIVE_REVIEW_NOTE_FILES: usize = 50;

pub(super) const REVIEW_NOTES_ARCHIVE_TITLE: &str = "# Review Notes Archive\n\n";

/// The resumable parts of an in-flight final review, persisted to
/// `.claude/final-review-progress.json` so a long review can be paused
/// (or survive an AMF quit / crash) and picked up where it left off.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct ReviewProgress {
    #[serde(default)]
    pub(super) decisions: std::collections::HashMap<String, ReviewDecision>,
    /// Which of `decisions`' rejections were auto-set by a line comment rather
    /// than explicitly. Defaulted so older progress files load unchanged (their
    /// rejections simply read as explicit — the conservative direction).
    #[serde(default)]
    pub(super) auto_rejected: std::collections::HashSet<String>,
    #[serde(default)]
    pub(super) line_comments: std::collections::HashMap<String, Vec<LineComment>>,
    #[serde(default)]
    pub(super) file_comments: std::collections::HashMap<String, FileComment>,
    #[serde(default)]
    pub(super) general_feedback: String,
    #[serde(default)]
    pub(super) apply_suggestions_on_finish: bool,
    #[serde(default)]
    pub(super) applied_suggestions: Vec<String>,
    #[serde(default)]
    pub(super) selected_file: usize,
}

/// Path of the saved review-progress file for a feature workdir.
pub(super) fn review_progress_path(workdir: &Path) -> PathBuf {
    workdir.join(".claude").join("final-review-progress.json")
}

/// Best-effort load of any saved review progress for `workdir`.
pub(super) fn load_review_progress(workdir: &Path) -> Option<ReviewProgress> {
    let content = std::fs::read_to_string(review_progress_path(workdir)).ok()?;
    serde_json::from_str(&content).ok()
}

/// A fingerprint of the diff as it stood at the last *finished* review round,
/// keyed by file path. Persisted to `.claude/final-review-snapshot.json` (kept
/// across rounds, unlike the progress file) so the next review can flag which
/// files changed since the reviewer last looked — the re-review loop.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct ReviewSnapshot {
    #[serde(default)]
    pub(super) reviewed_at: String,
    /// file path -> diff fingerprint at the time of the last finished review.
    #[serde(default)]
    pub(super) files: std::collections::HashMap<String, String>,
    /// file path -> the verdict that file carried when the round finished, so a
    /// re-review can restore approvals for files that have not changed since.
    /// Defaulted so snapshots written before this field load unchanged (no
    /// carried verdicts, everything simply re-checked).
    #[serde(default)]
    pub(super) decisions: std::collections::HashMap<String, ReviewDecision>,
    /// file path -> the kept (non-draft) line comments the round finished with,
    /// each tagged `carried` and keeping its `resolved` flag. Restored as live
    /// threads when the next round opens, so a conversation survives the agent
    /// addressing it — the cross-round half of the re-anchor machinery.
    /// Defaulted so snapshots written before this field load unchanged.
    #[serde(default)]
    pub(super) threads: std::collections::HashMap<String, Vec<LineComment>>,
    /// Whole-file threads carried across finished review rounds.
    #[serde(default)]
    pub(super) file_threads: std::collections::HashMap<String, FileComment>,
    /// file path -> the file's `new_content` as it stood when this round
    /// finished. Used to compute an on-demand "since last review" interdiff
    /// for a changed file (`open_interdiff`) without re-reading history from
    /// git. Absent for binary files and deletions (no content to diff from).
    /// Defaulted so snapshots written before this field load unchanged —
    /// interdiff simply has nothing to diff against until the next round
    /// refreshes the snapshot.
    #[serde(default)]
    pub(super) content: std::collections::HashMap<String, String>,
}

/// Path of the saved review-snapshot file for a feature workdir.
pub(super) fn review_snapshot_path(workdir: &Path) -> PathBuf {
    workdir.join(".claude").join("final-review-snapshot.json")
}

/// Best-effort load of the last review snapshot for `workdir`.
pub(super) fn load_review_snapshot(workdir: &Path) -> Option<ReviewSnapshot> {
    let content = std::fs::read_to_string(review_snapshot_path(workdir)).ok()?;
    serde_json::from_str(&content).ok()
}

/// Compute the diff between two arbitrary content strings as an in-memory
/// `DiffFile`, by materializing each to a temp file and reusing
/// `crate::diff::load_review_file` — the same plumbing the config-wizard
/// confirm dialog (`build_config_confirm_diff`) and the Claude-hook diff-
/// review prompt already use to diff two blobs that aren't necessarily
/// checked into git history.
pub(super) fn build_interdiff(
    old_content: &str,
    new_content: &str,
    display_path: &str,
) -> Result<crate::diff::DiffFile> {
    let mut original = tempfile::NamedTempFile::new()?;
    original.write_all(old_content.as_bytes())?;
    let mut modified = tempfile::NamedTempFile::new()?;
    modified.write_all(new_content.as_bytes())?;
    crate::diff::load_review_file(original.path(), modified.path(), display_path)
}

/// A stable-enough fingerprint of a file's diff. Hashes the patch plus the
/// status / line counts so a content change reads as "changed". This is a local
/// cache only: `DefaultHasher` is not guaranteed stable across toolchain
/// versions, so after an upgrade everything reads as changed — the safe default
/// (the reviewer simply re-checks).
pub(super) fn file_fingerprint(file: &crate::diff::DiffFile) -> String {
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
        // Refresh each comment's re-anchor snippet against the live diff first,
        // so whatever we persist can be re-located after a later refresh.
        self.recapture_anchor_contexts();
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
            file_comments: state.file_comments.clone(),
            general_feedback: state.general_feedback.clone(),
            apply_suggestions_on_finish: state.apply_suggestions_on_finish,
            applied_suggestions: state.applied_suggestions.clone(),
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

    /// Refresh the re-anchor context snippet of every line comment that still
    /// resolves against the live diff. Called from `persist_review_progress`,
    /// the choke point every state-changing review action funnels through, so a
    /// freshly created / edited comment picks up its snippet without each
    /// creation site having to capture one.
    ///
    /// A comment whose anchor is currently lost has no valid index to capture
    /// from, so its previously-captured snippet is deliberately left intact —
    /// that snippet is exactly what a later refresh needs to re-find it.
    pub(super) fn recapture_anchor_contexts(&mut self) {
        let AppMode::DiffViewer(state) = &mut self.mode else {
            return;
        };
        if !state.review || state.line_comments.is_empty() {
            return;
        }
        for file in &state.files {
            let Some(comments) = state.line_comments.get_mut(&file.path) else {
                continue;
            };
            let locs = file.addressable_lines();
            let texts = file.addressable_line_texts();
            for comment in comments.iter_mut() {
                if comment.anchor_lost {
                    continue;
                }
                if let Some(idx) = locs.iter().position(|l| *l == comment.location) {
                    comment.anchor_context = CommentAnchorContext::capture(&texts, idx);
                }
                comment.start_anchor_context = comment
                    .start
                    .and_then(|start| locs.iter().position(|l| *l == start))
                    .and_then(|idx| CommentAnchorContext::capture(&texts, idx));
            }
        }
    }

    /// Re-locate line comments after the diff reloaded underneath them (an `r`
    /// refresh once the agent edited the code, or a base-ref change). A comment
    /// anchors to an exact `DiffLineLocation`; when the line moves, that anchor
    /// no longer exists in the fresh `addressable_lines()` and the comment would
    /// silently vanish from the gutter.
    ///
    /// Reports what moved so the reviewer is never silently surprised.
    pub fn reanchor_line_comments(&mut self) {
        let AppMode::DiffViewer(state) = &mut self.mode else {
            return;
        };
        if !state.review || state.line_comments.is_empty() {
            return;
        }
        let (mut moved, mut lost) = (0usize, 0usize);
        for file in &state.files {
            let Some(comments) = state.line_comments.get_mut(&file.path) else {
                continue;
            };
            let (file_moved, file_lost) = reanchor_file_comments(file, comments);
            moved += file_moved;
            lost += file_lost;
        }
        if moved == 0 && lost == 0 {
            return;
        }
        let mut parts = Vec::new();
        if moved > 0 {
            parts.push(format!("re-anchored {moved} comment(s)"));
        }
        if lost > 0 {
            parts.push(format!(
                "{lost} comment(s) lost their anchor — possibly addressed"
            ));
        }
        self.message = Some(format!("Diff changed: {}", parts.join(", ")));
    }

    /// Remove any saved review progress for `workdir`. Called once a review
    /// finishes so the next review for the feature starts fresh.
    pub(super) fn clear_review_progress(workdir: &Path) {
        let _ = std::fs::remove_file(review_progress_path(workdir));
    }

    /// Record a fingerprint of the just-reviewed diff to
    /// `.claude/final-review-snapshot.json` so the next review can flag which
    /// files changed since (the re-review loop). Best-effort: a write failure is
    /// logged, not surfaced — it only degrades change detection on the next
    /// round. Unlike the progress file, this is *not* cleared on finish.
    pub(super) fn save_review_snapshot(
        &mut self,
        workdir: &Path,
        files: &[crate::diff::DiffFile],
        decisions: &std::collections::HashMap<String, ReviewDecision>,
        line_comments: &std::collections::HashMap<String, Vec<LineComment>>,
        file_comments: &std::collections::HashMap<String, FileComment>,
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
            // Carry every kept comment — resolved or not — into the next round as
            // a thread. Unadjudicated AI drafts are dropped: a finding the human
            // never accepted is not a conversation worth reopening.
            threads: files
                .iter()
                .filter_map(|f| {
                    let kept: Vec<LineComment> = line_comments
                        .get(&f.path)?
                        .iter()
                        .filter(|c| !c.draft)
                        .map(|c| LineComment {
                            carried: true,
                            ..c.clone()
                        })
                        .collect();
                    (!kept.is_empty()).then(|| (f.path.clone(), kept))
                })
                .collect(),
            file_threads: files
                .iter()
                .filter_map(|f| {
                    file_comments.get(&f.path).map(|c| {
                        (
                            f.path.clone(),
                            FileComment {
                                carried: true,
                                ..c.clone()
                            },
                        )
                    })
                })
                .collect(),
            content: files
                .iter()
                .filter_map(|f| f.new_content.clone().map(|c| (f.path.clone(), c)))
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

    /// Decide what state a final review opens with. The single choke point for
    /// restoration, with three cases taken in order:
    ///
    /// 1. The in-memory review already holds verdicts / comments / feedback —
    ///    an in-review `r` refresh or base-ref change. Leave it alone; a reload
    ///    must never clobber work in progress.
    /// 2. A `.claude/final-review-progress.json` exists — a review that was
    ///    paused (or interrupted by an AMF quit / crash). Restore it verbatim.
    ///    Its `line_comments` already contain any threads carried in at the open
    ///    that started it, so case 3 must not also run.
    /// 3. Otherwise this is a fresh round after a finished one. Seed the line
    ///    comments from the last snapshot's threads so unresolved conversations
    ///    survive the agent addressing them.
    ///
    /// In cases 2 and 3, entries for paths no longer in the diff are dropped.
    pub fn restore_review_progress(&mut self) {
        let AppMode::DiffViewer(state) = &mut self.mode else {
            return;
        };
        if !state.review
            || !state.decisions.is_empty()
            || !state.line_comments.is_empty()
            || !state.file_comments.is_empty()
            || !state.general_feedback.is_empty()
            || state.apply_suggestions_on_finish
            || !state.applied_suggestions.is_empty()
        {
            return;
        }
        let known: std::collections::HashSet<&str> =
            state.files.iter().map(|f| f.path.as_str()).collect();

        let Some(progress) = load_review_progress(&state.workdir) else {
            // Fresh round: carry the previous round's threads back in.
            let Some(snapshot) = load_review_snapshot(&state.workdir) else {
                return;
            };
            state.line_comments = snapshot
                .threads
                .into_iter()
                .filter(|(path, _)| known.contains(path.as_str()))
                .filter_map(|(path, comments)| {
                    let live: Vec<LineComment> = comments
                        .into_iter()
                        // A settled thread whose code is gone has nothing left to
                        // show, and would otherwise pile up round after round. An
                        // *unresolved* lost anchor is kept and surfaced — it may
                        // simply not have been addressed yet.
                        .filter(|c| !(c.resolved && c.anchor_lost))
                        .map(|c| LineComment { carried: true, ..c })
                        .collect();
                    (!live.is_empty()).then_some((path, live))
                })
                .collect();
            state.file_comments = snapshot
                .file_threads
                .into_iter()
                .filter(|(path, comment)| {
                    known.contains(path.as_str()) && !(comment.resolved && comment.text.is_empty())
                })
                .map(|(path, comment)| {
                    (
                        path,
                        FileComment {
                            carried: true,
                            ..comment
                        },
                    )
                })
                .collect();
            return;
        };
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
        state.file_comments = progress
            .file_comments
            .into_iter()
            .filter(|(path, _)| known.contains(path.as_str()))
            .collect();
        state.general_feedback = progress.general_feedback;
        state.apply_suggestions_on_finish = progress.apply_suggestions_on_finish;
        state.applied_suggestions = progress.applied_suggestions;
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
        // `line_comments` alone can't gate this: `restore_review_progress` just
        // seeded it with threads carried in from the last round, which is
        // exactly the state a fresh re-review opens with — so only comments
        // authored *this* session count as work in progress.
        let pristine = state.decisions.is_empty()
            && state.has_only_carried_comments()
            && state.general_feedback.is_empty()
            && state.applied_suggestions.is_empty()
            && state.file_filter == FileFilter::All;
        if !pristine {
            return;
        }
        // Fresh open of a re-review: carry over approvals for files that have not
        // changed since the last finished round, so the reviewer resumes from the
        // saved approved state and only re-checks the changed / rejected files.
        // Changed files were dropped from `changed_since_last`'s complement, so
        // their stale approval is deliberately not restored. A file with an open
        // thread is also excluded — an unresolved conversation means the file
        // still needs a look, whatever its stale verdict says.
        let carry: Vec<String> = state
            .files
            .iter()
            .filter(|f| !state.changed_since_last.contains(&f.path))
            .filter(|f| !state.file_has_unresolved_thread(&f.path))
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
        // Threads carried in from the last round are restored by
        // `restore_review_progress` before this runs, so this count already
        // reflects what survived into the fresh round.
        let unresolved = state.unresolved_thread_count();
        let thread_note = if unresolved == 0 {
            String::new()
        } else {
            format!(
                " · {unresolved} unresolved thread{} carried over",
                if unresolved == 1 { "" } else { "s" }
            )
        };
        if changed == 0 {
            self.message = Some(format!(
                "Re-review: no files changed since the last review{when}{thread_note}"
            ));
        } else if changed == total {
            self.message = Some(format!(
                "Re-review: all {total} file(s) changed since the last review{when}{thread_note}"
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
                 review{when}{thread_note} — showing changed only (F to cycle filter)"
            ));
        }
    }

    /// Open the "since last review" interdiff modal for the current file: the
    /// diff between its content when the last review round finished and its
    /// content now (`I` in the final review). Computed on demand — a single
    /// local `git diff --no-index`, not a headless pass — so there is no
    /// caching/polling machinery to manage, unlike the changeset overview.
    /// A no-op with a message when there is nothing meaningful to show: no
    /// prior review, the file has no saved content from last round (new since
    /// then, or was binary/deleted), or the content is actually unchanged
    /// (the file's fingerprint can also move for reasons other than its own
    /// content, e.g. the base ref shifted underneath it).
    pub fn open_interdiff(&mut self) {
        let AppMode::DiffViewer(state) = &self.mode else {
            return;
        };
        if !state.review {
            return;
        }
        let Some(file) = state.files.get(state.selected_file) else {
            return;
        };
        let Some(snapshot) = load_review_snapshot(&state.workdir) else {
            self.message = Some("No prior review to diff against".to_string());
            return;
        };
        let Some(old_content) = snapshot.content.get(&file.path).cloned() else {
            self.message = Some("No prior review content for this file".to_string());
            return;
        };
        let new_content = file.new_content.clone().unwrap_or_default();
        let path = file.path.clone();
        match build_interdiff(&old_content, &new_content, &path) {
            Ok(diff_file) if diff_file.hunks.is_empty() && !diff_file.is_binary => {
                self.message = Some("No changes to this file since the last review".to_string());
            }
            Ok(diff_file) => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.interdiff_file = Some(diff_file);
                    state.interdiff_open = true;
                    state.interdiff_scroll = 0;
                }
            }
            Err(err) => {
                self.message = Some(format!("Failed to compute interdiff: {err}"));
            }
        }
    }

    pub fn close_interdiff(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.interdiff_open = false;
        }
    }

    /// Max scroll offset for the interdiff modal, approximated from the raw
    /// patch line count (unified layout only), mirroring
    /// `diff_patch_line_count`'s estimate elsewhere.
    pub(super) fn interdiff_max_scroll(state: &DiffViewerState) -> usize {
        state
            .interdiff_file
            .as_ref()
            .map(|file| file.patch.lines().count().saturating_sub(1))
            .unwrap_or(0)
    }

    pub fn interdiff_scroll_down(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            let max = Self::interdiff_max_scroll(state);
            state.interdiff_scroll = (state.interdiff_scroll + amount).min(max);
        }
    }

    pub fn interdiff_scroll_up(&mut self, amount: usize) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.interdiff_scroll = state.interdiff_scroll.saturating_sub(amount);
        }
    }

    pub fn interdiff_scroll_top(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.interdiff_scroll = 0;
        }
    }

    pub fn interdiff_scroll_bottom(&mut self) {
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.interdiff_scroll = Self::interdiff_max_scroll(state);
        }
    }

    /// On opening a re-review, load the feature agent's replies from the last
    /// round of `.claude/final-review-feedback.md` (the `**Agent:**` blocks it
    /// was prompted to append) so they can be shown beside each file's diff.
    /// Only files still present in the diff are kept; a first review (no feedback
    /// file) leaves the map empty.
    pub fn load_prior_agent_responses(&mut self) {
        let AppMode::DiffViewer(state) = &mut self.mode else {
            return;
        };
        if !state.review {
            return;
        }
        state.prior_agent_responses.clear();
        let path = state
            .workdir
            .join(".claude")
            .join("final-review-feedback.md");
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return;
        };
        let known: std::collections::HashSet<&str> =
            state.files.iter().map(|f| f.path.as_str()).collect();
        state.prior_agent_responses = parse_agent_responses(&contents)
            .into_iter()
            .filter(|(path, _)| known.contains(path.as_str()))
            .collect();
    }
}

/// Document title that heads the feedback log. Each review round is prepended
/// directly under it.
pub(super) const FEEDBACK_TITLE: &str = "# Final Review Feedback\n\n";

/// Prepend a freshly-built review round to the existing feedback log so every
/// round is preserved as a trail rather than overwritten. `existing` is the
/// prior file content (if any); `round` is the new round's body (starting at
/// its `## Review …` heading and ending with a blank line). The newest round
/// lands directly under the single title, with prior rounds following.
pub(super) fn compose_feedback_log(existing: Option<&str>, round: &str) -> String {
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

/// Start a persisted review round with the metadata shared by both actionable
/// feedback rounds and all-approved rounds. Keeping successful rounds too is
/// what lets the history browser represent the complete review conversation
/// instead of silently omitting its positive outcomes.
#[allow(clippy::too_many_arguments)]
pub(super) fn review_round_preamble(
    total: usize,
    approved: usize,
    rejected: usize,
    skipped: usize,
    file_comments: usize,
    line_comments: usize,
    check: Option<&CheckOutcome>,
    applied_suggestions: &[String],
    suggestion_apply_failures: &[String],
) -> String {
    let mut round = String::new();
    round.push_str(&format!(
        "## Review — {}\n\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    ));
    round.push_str(&format!(
        "**Files reviewed:** {total} | **Approved:** {approved} | \
         **Needs work:** {rejected} | **Skipped:** {skipped} | \
         **File comments:** {file_comments} | **Line comments:** {line_comments}\n\n"
    ));
    if let Some(c) = check {
        round.push_str(&format!(
            "**Check:** `{}` — {}\n\n",
            c.command,
            if c.passed { "passed" } else { "FAILED" }
        ));
        if !c.passed {
            round.push_str("```\n");
            round.push_str(&c.output);
            round.push_str("\n```\n\n");
        }
    }
    if !applied_suggestions.is_empty() || !suggestion_apply_failures.is_empty() {
        round.push_str("**Local suggestion application:** ");
        if !applied_suggestions.is_empty() {
            round.push_str(&format!(
                "{} applied ({})",
                applied_suggestions.len(),
                applied_suggestions.join(", ")
            ));
        }
        if !suggestion_apply_failures.is_empty() {
            if !applied_suggestions.is_empty() {
                round.push_str("; ");
            }
            round.push_str(&format!(
                "{} not applied ({})",
                suggestion_apply_failures.len(),
                suggestion_apply_failures.join("; ")
            ));
        }
        round.push_str("\n\n");
    }
    round
}

/// Review rounds kept in the live feedback file: the newest round the agent
/// is asked to address, plus one prior round for context. Anything older is
/// moved to `final-review-feedback-archive.md` by `split_overflow_rounds` —
/// only the newest round is ever consumed (by `REVIEW_FEEDBACK_PROMPT` and by
/// `parse_agent_responses`), so keeping the rest in the live file would only
/// cost the agent a bigger read every round without it ever being used.
pub(super) const MAX_LIVE_ROUNDS: usize = 2;

/// Split a feedback log's body (title already stripped) into per-round
/// chunks, each starting at a `## Review` heading line and running up to (not
/// including) the next one.
pub(super) fn split_rounds(body: &str) -> Vec<String> {
    let mut rounds: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in body.lines() {
        if line.starts_with("## Review") && !current.is_empty() {
            rounds.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        rounds.push(current);
    }
    rounds
}

/// Parse a feedback/history document into self-contained rounds, preserving its
/// on-disk order. The live log is newest-first; the archive caller reverses its
/// oldest-first result before appending it to the timeline.
pub(super) fn parse_review_history_rounds(
    content: &str,
    document_title: &str,
) -> Vec<ReviewHistoryRound> {
    let body = content.strip_prefix(document_title).unwrap_or(content);
    split_rounds(body)
        .into_iter()
        .filter(|round| round.trim_start().starts_with("## Review"))
        .map(|markdown| {
            let title = markdown
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("## "))
                .unwrap_or("Review")
                .trim()
                .to_string();
            let carried_unresolved = markdown
                .matches("(unresolved from a previous round)")
                .count();
            ReviewHistoryRound {
                title,
                markdown,
                carried_unresolved,
            }
        })
        .collect()
}

/// Split a composed feedback log (title + rounds, newest first) into the live
/// content to keep (the newest `keep` rounds, re-titled) and, when there are
/// more rounds than that, the older ones to move out to the archive file
/// (`None` when nothing overflows).
pub(super) fn split_overflow_rounds(content: &str, keep: usize) -> (String, Option<String>) {
    let body = content.strip_prefix(FEEDBACK_TITLE).unwrap_or(content);
    let rounds = split_rounds(body);
    if rounds.len() <= keep {
        return (content.to_string(), None);
    }
    let (live_rounds, old_rounds) = rounds.split_at(keep);
    let mut live = String::from(FEEDBACK_TITLE);
    for r in live_rounds {
        live.push_str(r);
    }
    (live, Some(old_rounds.concat()))
}

/// Reduce an item's anchor heading (`src/foo.rs:42`, `src/foo.rs:42-48 (base)`,
/// `src/foo.rs (anchor lost — possibly addressed)`, or a bare `src/foo.rs`) to
/// the file path it belongs to. A trailing ` (…)` note is stripped first — an
/// anchor-lost heading carries no line number, so without this the whole heading
/// would read as the path. Then a `:` followed by a digit marks the start of the
/// line suffix; anything before it is the path.
pub(super) fn anchor_file_path(anchor: &str) -> &str {
    let anchor = match anchor.find(" (") {
        Some(idx) => &anchor[..idx],
        None => anchor,
    };
    match anchor.find(':') {
        Some(idx) if anchor[idx + 1..].starts_with(|c: char| c.is_ascii_digit()) => &anchor[..idx],
        _ => anchor,
    }
}

/// Parse the latest review round's agent replies from a feedback file, grouped
/// by file path. Rounds are prepended (see `compose_feedback_log`), so the first
/// `## Review` section is the most recent. Each `#### {anchor} — [severity]` item
/// may be followed by a `**Agent:** …` reply the feature agent appended (see
/// `REVIEW_FEEDBACK_PROMPT`); only items whose agent actually replied are
/// returned. The reply runs from the `**Agent:**` marker to the next heading.
pub(super) fn parse_agent_responses(
    feedback: &str,
) -> std::collections::HashMap<String, Vec<crate::app::state::AgentResponse>> {
    use crate::app::state::AgentResponse;
    let mut out: std::collections::HashMap<String, Vec<AgentResponse>> =
        std::collections::HashMap::new();

    let mut lines = feedback.lines();
    // Advance to the newest round; bail if the file has none.
    for line in lines.by_ref() {
        if line.starts_with("## Review") {
            break;
        }
    }

    let mut anchor: Option<String> = None;
    // The reply text currently being collected for `anchor`, once a `**Agent:**`
    // marker has been seen. `None` until the marker, so the item's own text is
    // skipped.
    let mut reply: Option<Vec<String>> = None;

    // Flush the collected reply (if any) onto the current anchor.
    let flush = |anchor: &Option<String>,
                 reply: &mut Option<Vec<String>>,
                 out: &mut std::collections::HashMap<String, Vec<AgentResponse>>| {
        if let (Some(anchor), Some(body)) = (anchor.as_ref(), reply.take()) {
            let response = body.join("\n").trim().to_string();
            if !response.is_empty() {
                out.entry(anchor_file_path(anchor).to_string())
                    .or_default()
                    .push(AgentResponse {
                        anchor: anchor.clone(),
                        response,
                    });
            }
        }
    };

    for line in lines {
        // A new round ends the latest one — stop before older history.
        if line.starts_with("## Review") || line.starts_with("# ") {
            flush(&anchor, &mut reply, &mut out);
            break;
        }
        if let Some(rest) = line.strip_prefix("#### ") {
            flush(&anchor, &mut reply, &mut out);
            // Heading is `{anchor} — [severity]`; keep just the anchor.
            anchor = Some(rest.split(" — [").next().unwrap_or(rest).trim().to_string());
            continue;
        }
        if line.starts_with("### ") {
            // A section heading (General Feedback / Files Needing Revision /
            // Line Comments) ends the current item's reply but stays in-round.
            flush(&anchor, &mut reply, &mut out);
            anchor = None;
            continue;
        }
        if let Some(body) = &mut reply {
            body.push(line.to_string());
        } else if let Some(rest) = line.trim_start().strip_prefix("**Agent:**") {
            reply = Some(vec![rest.trim_start().to_string()]);
        }
    }
    flush(&anchor, &mut reply, &mut out);
    out
}

pub(super) fn review_note_path_from_heading(line: &str) -> Option<String> {
    let heading = line
        .strip_prefix("### ")
        .or_else(|| line.strip_prefix("## "))?
        .trim();
    Some(
        heading
            .split(" — ")
            .next()
            .unwrap_or(heading)
            .split(" - ")
            .next()
            .unwrap_or(heading)
            .trim()
            .to_string(),
    )
}

/// Split a review-notes document into its non-section preamble and raw note
/// sections. A section starts at the same `##` / `###` headings recognized by
/// [`parse_review_notes`] and retains its original markdown verbatim.
pub(super) fn review_note_sections(content: &str) -> (String, Vec<(String, String)>) {
    let mut preamble = String::new();
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;

    for line in content.split_inclusive('\n') {
        let heading_line = line.trim_end_matches(['\r', '\n']);
        if let Some(path) = review_note_path_from_heading(heading_line) {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            current = Some((path, line.to_string()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
        } else {
            preamble.push_str(line);
        }
    }
    if let Some(section) = current {
        sections.push(section);
    }

    (preamble, sections)
}

pub(super) fn push_review_note_chunk(out: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(chunk);
}

pub(super) fn write_review_notes_atomic(path: &Path, content: &str) -> Result<()> {
    use anyhow::Context as _;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to stage {}", path.display()))?;
    staged
        .write_all(content.as_bytes())
        .with_context(|| format!("failed to stage {}", path.display()))?;
    staged
        .as_file()
        .sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))?;
    staged
        .persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

/// Keep the latest note for each of the `keep` most recently documented files
/// in the live document. Older duplicates are archived too: `parse_review_notes`
/// already exposes only the newest section for a path, so retaining superseded
/// copies live adds read cost without changing the reviewer-visible result. Any
/// preamble (content before the first heading) always stays in the live
/// document — it's never itself a candidate for archiving.
pub(super) fn split_overflow_review_notes(content: &str, keep: usize) -> (String, Option<String>) {
    let (preamble, sections) = review_note_sections(content);
    let mut seen = std::collections::HashSet::new();
    let mut live_indices = std::collections::HashSet::new();

    for (index, (path, _)) in sections.iter().enumerate().rev() {
        if seen.insert(path.as_str()) && live_indices.len() < keep {
            live_indices.insert(index);
        }
    }

    if live_indices.len() == sections.len() {
        return (content.to_string(), None);
    }

    let mut live = preamble;
    let mut overflow = String::new();
    for (index, (_, section)) in sections.iter().enumerate() {
        if live_indices.contains(&index) {
            push_review_note_chunk(&mut live, section);
        } else {
            push_review_note_chunk(&mut overflow, section);
        }
    }

    let overflow = (!overflow.trim().is_empty()).then_some(overflow);
    (live, overflow)
}

/// Bound `.claude/review-notes.md`, moving older/superseded sections into
/// `.claude/review-notes-archive.md`. The archive is written first so a failed
/// archive write never removes history from the live file.
pub(crate) fn archive_review_notes(workdir: &Path) -> Result<usize> {
    use anyhow::Context as _;
    use std::io::ErrorKind;

    let claude_dir = workdir.join(".claude");
    let live_path = claude_dir.join("review-notes.md");
    let content = match std::fs::read_to_string(&live_path) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", live_path.display()));
        }
    };
    let (live, Some(overflow)) = split_overflow_review_notes(&content, MAX_LIVE_REVIEW_NOTE_FILES)
    else {
        return Ok(0);
    };
    let archived_count = review_note_sections(&overflow).1.len();

    let archive_path = claude_dir.join("review-notes-archive.md");
    let mut archive = match std::fs::read_to_string(&archive_path) {
        Ok(archive) => archive,
        Err(err) if err.kind() == ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", archive_path.display()));
        }
    };
    if archive.is_empty() {
        archive.push_str(REVIEW_NOTES_ARCHIVE_TITLE);
    } else if !archive.ends_with('\n') {
        archive.push('\n');
    }
    push_review_note_chunk(&mut archive, &overflow);

    write_review_notes_atomic(&archive_path, &archive)?;
    write_review_notes_atomic(&live_path, &live)?;
    Ok(archived_count)
}

/// Load all developer notes for AMF's review surfaces. The feature agent
/// blind-appends to the live file and never reads it back; AMF folds in
/// archived sections here so older files keep their walkthrough context. Live
/// notes win over archived notes for the same path.
pub(crate) fn load_review_notes(workdir: &Path) -> std::collections::HashMap<String, String> {
    let claude_dir = workdir.join(".claude");
    let mut notes = std::fs::read_to_string(claude_dir.join("review-notes-archive.md"))
        .map(|content| parse_review_notes(&content))
        .unwrap_or_default();
    if let Ok(content) = std::fs::read_to_string(claude_dir.join("review-notes.md")) {
        notes.extend(parse_review_notes(&content));
    }
    notes
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
        if let Some(path) = review_note_path_from_heading(line) {
            flush(&mut current, &mut map);
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
