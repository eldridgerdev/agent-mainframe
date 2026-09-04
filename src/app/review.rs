use std::collections::HashSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::*;
use crate::app::pr_review::FixTarget;
use crate::extension::merge_project_extension_config;

/// Label (and de-facto identity) of the dedicated final-review agent session.
/// Found-or-created by this label so re-running the review reuses the same
/// window. Kept distinct from PR Triage's "PR Triage" session so the two
/// dedicated targets never collide on one feature.
pub(crate) const FINAL_REVIEW_SESSION_LABEL: &str = "Final Review";

/// The prompt dispatched to the agent after a review finishes, asking it to act
/// on the feedback file's most recent round.
const REVIEW_FEEDBACK_PROMPT: &str = "A reviewer left feedback on these changes in \
     .claude/final-review-feedback.md. Read that file and address every item in the most recent \
     review round (the first \"## Review\" section); earlier sections are prior rounds kept for \
     history. Each item is tagged with a severity in brackets: [blocker] must be fixed, \
     [suggestion] and [nit] are improvements worth making, [question] wants an answer (not \
     necessarily a code change), and [praise] needs no action. Prioritize the blockers. \
     After you address an item, append a reply directly under it in that same file on its own \
     line, starting with \"**Agent:** \" — say what you changed (e.g. \"fixed in src/foo.rs\") or, \
     if you disagree or are answering a [question], why. Keep each reply to a sentence or two. \
     These replies are shown to the reviewer beside your changes on the next review round.";

/// Outcome of the project's optional `final_review_check_command` (a
/// build/test gate), run in the background when finishing a review. `None`
/// throughout `complete_final_review` whenever no command is configured.
struct CheckOutcome {
    command: String,
    passed: bool,
    output: String,
}

#[derive(Debug, Default)]
struct SuggestionApplyReport {
    applied: Vec<String>,
    failures: Vec<String>,
}

#[derive(Debug)]
struct PlannedSuggestion {
    comment_index: usize,
    anchor: String,
    start_line: usize,
    end_line: usize,
    replacement: String,
}

#[derive(Debug, Default)]
struct FileSuggestionApplyReport {
    applied: Vec<(usize, String)>,
    failures: Vec<String>,
}

/// Cap on how much of a check command's combined stdout/stderr is kept, so a
/// noisy build/test failure can't blow up the feedback file or the agent
/// prompt built from it.
const CHECK_OUTPUT_MAX_CHARS: usize = 4000;

/// Cap on how many files' notes stay in `.claude/review-notes.md`. In Review
/// Mode the feature agent blind-appends sections and never reads the file back
/// (`ensure_review_claude_md`); AMF collapses it to the newest note per file
/// after every agent turn, keeping only the `MAX_LIVE_REVIEW_NOTE_FILES` most
/// recently documented. Older and superseded sections remain available to AMF
/// in the archive.
const MAX_LIVE_REVIEW_NOTE_FILES: usize = 50;
const REVIEW_NOTES_ARCHIVE_TITLE: &str = "# Review Notes Archive\n\n";

fn truncate_check_output(output: &str) -> String {
    if output.chars().count() <= CHECK_OUTPUT_MAX_CHARS {
        output.to_string()
    } else {
        let truncated: String = output.chars().take(CHECK_OUTPUT_MAX_CHARS).collect();
        format!("{truncated}\n… (truncated)")
    }
}

fn local_suggestion_summary(applied: &[String], failures: &[String]) -> String {
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
    file_comments: std::collections::HashMap<String, FileComment>,
    #[serde(default)]
    general_feedback: String,
    #[serde(default)]
    apply_suggestions_on_finish: bool,
    #[serde(default)]
    applied_suggestions: Vec<String>,
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
    /// file path -> the kept (non-draft) line comments the round finished with,
    /// each tagged `carried` and keeping its `resolved` flag. Restored as live
    /// threads when the next round opens, so a conversation survives the agent
    /// addressing it — the cross-round half of the re-anchor machinery.
    /// Defaulted so snapshots written before this field load unchanged.
    #[serde(default)]
    threads: std::collections::HashMap<String, Vec<LineComment>>,
    /// Whole-file threads carried across finished review rounds.
    #[serde(default)]
    file_threads: std::collections::HashMap<String, FileComment>,
    /// file path -> the file's `new_content` as it stood when this round
    /// finished. Used to compute an on-demand "since last review" interdiff
    /// for a changed file (`open_interdiff`) without re-reading history from
    /// git. Absent for binary files and deletions (no content to diff from).
    /// Defaulted so snapshots written before this field load unchanged —
    /// interdiff simply has nothing to diff against until the next round
    /// refreshes the snapshot.
    #[serde(default)]
    content: std::collections::HashMap<String, String>,
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

/// Compute the diff between two arbitrary content strings as an in-memory
/// `DiffFile`, by materializing each to a temp file and reusing
/// `crate::diff::load_review_file` — the same plumbing the config-wizard
/// confirm dialog (`build_config_confirm_diff`) and the Claude-hook diff-
/// review prompt already use to diff two blobs that aren't necessarily
/// checked into git history.
fn build_interdiff(
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
    fn recapture_anchor_contexts(&mut self) {
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
    fn interdiff_max_scroll(state: &DiffViewerState) -> usize {
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
    fn diff_search_recompute_and_jump(&mut self) {
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
    fn diff_review_sync_auto_reject(&mut self, path: &str) {
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
    fn apply_review_suggestion_jobs(
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

    /// Generate a walkthrough for the current file when it has no developer
    /// note. Spawns a headless Claude explanation of the file's diff; the result
    /// is collected by `poll_review_walkthrough` and cached in `generated_notes`
    /// so the developer-notes panel is never empty.
    pub fn generate_review_walkthrough(&mut self) {
        let (workdir, path, ctx) = {
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
            (state.workdir.clone(), path, walkthrough_context(file))
        };

        let repo = crate::worktree::WorktreeManager::repo_root(&workdir)
            .unwrap_or_else(|_| workdir.clone());
        let prompt = self.resolve_headless_prompt(
            crate::prompts::PromptId::ReviewWalkthrough,
            &crate::project::AgentKind::Claude,
            &repo,
            &workdir,
            &ctx,
        );
        if !self.precall_gate(
            crate::app::precall::PrecallAction::ReviewWalkthrough,
            &crate::project::AgentKind::Claude,
            &prompt,
        ) {
            return;
        }
        let model = self.config.review_model_for(ReviewAction::Walkthrough);
        match crate::claude::ClaudeLauncher::spawn_headless(&workdir, &prompt, model.as_deref()) {
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
        let (workdir, path, ctx) = {
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
                co_review_context(file),
            )
        };

        let repo = crate::worktree::WorktreeManager::repo_root(&workdir)
            .unwrap_or_else(|_| workdir.clone());
        let prompt = self.resolve_headless_prompt(
            crate::prompts::PromptId::ReviewCoReview,
            &crate::project::AgentKind::Claude,
            &repo,
            &workdir,
            &ctx,
        );
        if !self.precall_gate(
            crate::app::precall::PrecallAction::ReviewCoReview,
            &crate::project::AgentKind::Claude,
            &prompt,
        ) {
            return;
        }
        let model = self.config.review_model_for(ReviewAction::CoReview);
        match crate::claude::ClaudeLauncher::spawn_headless(&workdir, &prompt, model.as_deref()) {
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

    /// Open the changeset-overview modal (reviewer-triggered, `O`). Reuses a
    /// cached overview for free; only spawns a headless pass when nothing is
    /// cached and nothing is already generating, so simply reopening the modal
    /// never re-triggers a headless request on its own — the plan's "manual
    /// only, never automatic" requirement is about *generation*, not viewing.
    pub fn open_changeset_overview(&mut self) {
        let AppMode::DiffViewer(state) = &mut self.mode else {
            return;
        };
        if !state.review {
            return;
        }
        if state.changeset_overview.is_some() || state.changeset_overview_child.is_some() {
            // Cached or already running: just show it.
            state.changeset_overview_open = true;
            return;
        }
        // A fresh pass: `generate_changeset_overview` opens the modal only once
        // the run actually starts, so a cancelled pre-call notice leaves the
        // viewer with no half-open "generating…" modal.
        self.generate_changeset_overview();
    }

    /// Spawn (or re-spawn) a headless whole-changeset overview pass. Unlike
    /// `open_changeset_overview` this always starts a fresh generation when
    /// none is already in flight — the explicit "regenerate" action once the
    /// modal is open.
    pub fn generate_changeset_overview(&mut self) {
        let (workdir, ctx) = {
            let AppMode::DiffViewer(state) = &self.mode else {
                return;
            };
            if !state.review || state.changeset_overview_child.is_some() {
                return;
            }
            if state.files.is_empty() {
                return;
            }
            (
                state.workdir.clone(),
                changeset_overview_context(&state.files),
            )
        };

        let repo = crate::worktree::WorktreeManager::repo_root(&workdir)
            .unwrap_or_else(|_| workdir.clone());
        let prompt = self.resolve_headless_prompt(
            crate::prompts::PromptId::ReviewChangesetOverview,
            &crate::project::AgentKind::Claude,
            &repo,
            &workdir,
            &ctx,
        );
        if !self.precall_gate(
            crate::app::precall::PrecallAction::ReviewChangesetOverview,
            &crate::project::AgentKind::Claude,
            &prompt,
        ) {
            return;
        }
        let model = self
            .config
            .review_model_for(ReviewAction::ChangesetOverview);
        // The pre-call gate has been cleared, so the user has committed to this
        // pass: open the modal either way. On success it shows "generating…";
        // on a spawn failure it shows the error, rather than the viewer just
        // swallowing the keypress. (A *cancelled* pre-call returns above,
        // before this, so it still leaves no half-open modal.)
        match crate::claude::ClaudeLauncher::spawn_headless(&workdir, &prompt, model.as_deref()) {
            Ok(child) => {
                self.message = Some("Changeset overview running…".to_string());
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.changeset_overview_child = Some(child);
                    state.changeset_overview_open = true;
                }
            }
            Err(err) => {
                self.message = Some(format!("Changeset overview unavailable: {err}"));
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.changeset_overview =
                        Some(format!("Changeset overview unavailable: {err}"));
                    state.changeset_overview_open = true;
                }
            }
        }
    }

    /// Poll an in-flight changeset-overview generation; on completion cache the
    /// result and reset the modal's scroll. Mirrors `poll_review_walkthrough`.
    pub fn poll_changeset_overview(&mut self) -> Result<()> {
        let finished = match &mut self.mode {
            AppMode::DiffViewer(state) => match state.changeset_overview_child.as_mut() {
                Some(child) => child.try_wait()?,
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        let Some(status) = finished else {
            return Ok(());
        };

        let child = match &mut self.mode {
            AppMode::DiffViewer(state) => state.changeset_overview_child.take(),
            _ => None,
        };
        let Some(child) = child else {
            return Ok(());
        };

        let output = child.wait_with_output()?;
        let overview = if status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if text.is_empty() {
                "Changeset overview was empty.".to_string()
            } else {
                text
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            format!("Changeset overview unavailable: {stderr}")
        };
        if let AppMode::DiffViewer(state) = &mut self.mode {
            state.changeset_overview = Some(overview);
            state.changeset_overview_scroll = 0;
        }
        Ok(())
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
    fn changeset_overview_max_scroll(state: &DiffViewerState) -> usize {
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
    fn review_help_max_scroll(state: &DiffViewerState) -> usize {
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
    fn review_comment_stops(state: &DiffViewerState) -> (Vec<(usize, usize)>, usize) {
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
    fn load_review_history_archive(&mut self) {
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

    fn review_history_max_scroll(state: &DiffViewerState) -> usize {
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

    /// Finish the review. If the project has a `final_review_check_command`
    /// configured (a build/test gate), spawn it in the background and return
    /// immediately — `poll_final_review_check` picks up the result once the
    /// process exits and actually completes the review. Otherwise (the
    /// default: no command configured) completes immediately, unchanged from
    /// before this gate existed.
    pub fn finish_final_review(&mut self) -> Result<()> {
        let apply_on_finish = matches!(&self.mode, AppMode::DiffViewer(state) if state.review && state.apply_suggestions_on_finish);
        if apply_on_finish {
            if let AppMode::DiffViewer(state) = &mut self.mode {
                // Consume the opt-in before doing any work so a repeated finish
                // attempt (for example after an async check) never applies twice.
                state.apply_suggestions_on_finish = false;
                state.suggestion_apply_failures.clear();
            }
            let report = self.apply_review_suggestion_jobs(None);
            if let AppMode::DiffViewer(state) = &mut self.mode {
                state.suggestion_apply_failures = report.failures;
            }
            self.persist_review_progress();
        }

        let spawn_info = match &self.mode {
            AppMode::DiffViewer(state)
                if state.review
                    && !state.files.is_empty()
                    && state.finish_check_child.is_none() =>
            {
                Some((state.workdir.clone(), state.from_view.project_name.clone()))
            }
            _ => None,
        };

        let Some((workdir, project_name)) = spawn_info else {
            return self.complete_final_review(None);
        };

        let command = self
            .store
            .projects
            .iter()
            .find(|p| p.name == project_name)
            .map(|p| merge_project_extension_config(&self.config.extension, &p.repo))
            .and_then(|ext| ext.final_review_check_command)
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());

        let Some(command) = command else {
            return self.complete_final_review(None);
        };

        match std::process::Command::new("bash")
            .arg("-c")
            .arg(&command)
            .current_dir(&workdir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => {
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.finish_check_command = Some(command.clone());
                    state.finish_check_child = Some(child);
                }
                self.message = Some(format!("Running check before finishing: {command} …"));
                Ok(())
            }
            // Don't let an environment problem (bad shell, missing dir, …)
            // block finishing the review — complete it, but report the check
            // as a failure so it isn't silently dropped.
            Err(e) => self.complete_final_review(Some(CheckOutcome {
                command,
                passed: false,
                output: format!("failed to start: {e}"),
            })),
        }
    }

    /// Poll the in-flight final-review check process (spawned by
    /// `finish_final_review`); once it exits, actually finish the review with
    /// its outcome folded in. Mirrors `poll_changeset_overview`.
    pub fn poll_final_review_check(&mut self) -> Result<()> {
        let finished = match &mut self.mode {
            AppMode::DiffViewer(state) => match state.finish_check_child.as_mut() {
                Some(child) => child.try_wait()?,
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        let Some(status) = finished else {
            return Ok(());
        };

        let (child, command) = match &mut self.mode {
            AppMode::DiffViewer(state) => (
                state.finish_check_child.take(),
                state.finish_check_command.take(),
            ),
            _ => (None, None),
        };
        let (Some(child), Some(command)) = (child, command) else {
            return Ok(());
        };

        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = if !stdout.trim().is_empty() && !stderr.trim().is_empty() {
            format!("{}\n{}", stdout.trim(), stderr.trim())
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };

        self.complete_final_review(Some(CheckOutcome {
            command,
            passed: status.success(),
            output: truncate_check_output(&combined),
        }))
    }

    /// Persist one self-contained round into the bounded live feedback log,
    /// archiving overflow before replacing the live file. The archive remains
    /// outside the fixing agent's read path and is consumed lazily by the
    /// history browser.
    fn persist_final_review_round(&mut self, workdir: &Path, round: &str) -> std::io::Result<()> {
        let path = workdir.join(".claude").join("final-review-feedback.md");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let out = compose_feedback_log(std::fs::read_to_string(&path).ok().as_deref(), round);
        let (live, overflow) = split_overflow_rounds(&out, MAX_LIVE_ROUNDS);

        if let Some(overflow) = overflow {
            let archive_path = workdir
                .join(".claude")
                .join("final-review-feedback-archive.md");
            let mut archive = std::fs::read_to_string(&archive_path).unwrap_or_default();
            if archive.is_empty() {
                archive.push_str("# Final Review Feedback Archive\n\n");
            }
            archive.push_str(&overflow);
            if let Err(e) = std::fs::write(&archive_path, archive) {
                // Preserve the existing behavior: an archive failure is
                // visible in the debug log but does not prevent the latest
                // actionable round from reaching the agent.
                self.log_warn(
                    "review",
                    format!("failed to archive prior review rounds: {e}"),
                );
            }
        }
        std::fs::write(path, live)
    }

    /// Write `.claude/final-review-feedback.md` for any rejected files and
    /// return to the feature view with a summary message, folding in the
    /// optional build/test-gate `check` outcome.
    fn complete_final_review(&mut self, check: Option<CheckOutcome>) -> Result<()> {
        let (
            workdir,
            files,
            decisions,
            line_comments,
            file_comments,
            general_feedback,
            from_view,
            fix_target,
            fix_target_feature_id,
            review_harness,
            applied_suggestions,
            suggestion_apply_failures,
        ) = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffViewer(state) => (
                state.workdir,
                state.files,
                state.decisions,
                state.line_comments,
                state.file_comments,
                state.general_feedback,
                state.from_view,
                state.fix_target,
                state.fix_target_feature_id,
                state.review_harness,
                state.applied_suggestions,
                state.suggestion_apply_failures,
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
            self.save_review_snapshot(&workdir, &files, &decisions, &line_comments, &file_comments);
        }

        let total = files.len();
        let mut approved = 0usize;
        let mut rejected: Vec<(String, String, Severity)> = Vec::new();
        for file in &files {
            match decisions.get(&file.path) {
                Some(ReviewDecision::Approve) => approved += 1,
                Some(ReviewDecision::Reject { feedback, severity }) => {
                    rejected.push((file.path.clone(), feedback.clone(), *severity));
                }
                None => {}
            }
        }

        let file_comment_sections: Vec<(String, FileComment)> = files
            .iter()
            .filter_map(|file| {
                file_comments
                    .get(&file.path)
                    .filter(|comment| comment.is_open_thread())
                    .cloned()
                    .map(|comment| (file.path.clone(), comment))
            })
            .collect();
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
                // Unaccepted AI drafts and resolved threads never reach the
                // feedback file or the PR review — only open threads the human
                // kept and hasn't settled.
                let kept: Vec<LineComment> = comments
                    .iter()
                    .filter(|c| c.is_open_thread())
                    .cloned()
                    .collect();
                if !kept.is_empty() {
                    line_comment_count += kept.len();
                    line_comment_sections.push((file.path.clone(), kept));
                }
            }
        }

        // A failed check gate must never be swallowed by the "all approved"
        // fast path below — it's the whole point of the gate.
        let check_failed = matches!(&check, Some(c) if !c.passed);

        let no_actionable_feedback = !check_failed
            && rejected.is_empty()
            && general_feedback.is_empty()
            && line_comment_sections.is_empty()
            && file_comment_sections.is_empty();
        if no_actionable_feedback {
            // Successful/all-approved rounds still belong in the review
            // timeline. They are persisted but never dispatched to an agent.
            let round = review_round_preamble(
                total,
                approved,
                rejected.len(),
                skipped,
                file_comment_sections.len(),
                line_comment_count,
                check.as_ref(),
                &applied_suggestions,
                &suggestion_apply_failures,
            );
            let history_error = self
                .persist_final_review_round(&workdir, &round)
                .err()
                .map(|e| format!(" (history not saved: {e})"))
                .unwrap_or_default();
            self.message = Some(
                if total == 0 {
                    "Final review: no changes against the base branch".to_string()
                } else {
                    let check_note = match &check {
                        Some(c) => format!(" (check `{}` passed)", c.command),
                        None => String::new(),
                    };
                    let local_note =
                        local_suggestion_summary(&applied_suggestions, &suggestion_apply_failures);
                    format!(
                        "Final review complete: all {approved} reviewed file(s) approved{}{check_note}{local_note}",
                        if skipped > 0 {
                            format!(", {skipped} skipped")
                        } else {
                            String::new()
                        }
                    )
                } + &history_error,
            );
            self.mode = AppMode::Viewing(from_view);
            return Ok(());
        }
        {
            // Build this round as a self-contained section. Rounds are
            // prepended under a single title (see `compose_feedback_log`) so
            // every review is preserved as a trail rather than overwritten.
            let mut round = review_round_preamble(
                total,
                approved,
                rejected.len(),
                skipped,
                file_comment_sections.len(),
                line_comment_count,
                check.as_ref(),
                &applied_suggestions,
                &suggestion_apply_failures,
            );

            if !general_feedback.is_empty() {
                round.push_str("### General Feedback\n\n");
                round.push_str(&general_feedback);
                round.push_str("\n\n");
            }

            if !rejected.is_empty() {
                round.push_str("### Files Needing Revision\n\n");
                for (file, feedback, severity) in &rejected {
                    round.push_str(&format!("#### {file} — [{}]\n\n", severity.label()));
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

            if !file_comment_sections.is_empty() {
                round.push_str("### File Comments\n\n");
                for (file, comment) in &file_comment_sections {
                    let carried_tag = if comment.carried {
                        " (unresolved from a previous round)"
                    } else {
                        ""
                    };
                    round.push_str(&format!(
                        "#### {file} — [{}]{carried_tag}\n\n{}\n\n",
                        comment.severity.label(),
                        comment.text
                    ));
                }
            }

            if !line_comment_sections.is_empty() {
                round.push_str("### Line Comments\n\n");
                for (file, comments) in &line_comment_sections {
                    for comment in comments {
                        let anchor = comment_anchor_label(file, comment);
                        // Only unresolved threads reach this point (`kept` above
                        // filters on `is_open_thread`), so a carried comment here
                        // is always still open — flag it as such.
                        let carried_tag = if comment.carried {
                            " (unresolved from a previous round)"
                        } else {
                            ""
                        };
                        round.push_str(&format!(
                            "#### {anchor} — [{}]{carried_tag}\n\n",
                            comment.severity.label()
                        ));
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

            if let Err(e) = self.persist_final_review_round(&workdir, &round) {
                self.message = Some(format!("Final review: failed to write feedback file: {e}"));
                self.mode = AppMode::Viewing(from_view);
                return Ok(());
            }

            let comment_note = if line_comment_count > 0 {
                format!(", {line_comment_count} line comment(s)")
            } else {
                String::new()
            };
            let file_comment_note = if file_comment_sections.is_empty() {
                String::new()
            } else {
                format!(", {} file comment(s)", file_comment_sections.len())
            };
            let check_note = match &check {
                Some(c) if c.passed => format!(", check `{}` passed", c.command),
                Some(c) => format!(", check `{}` FAILED", c.command),
                None => String::new(),
            };
            let local_note =
                local_suggestion_summary(&applied_suggestions, &suggestion_apply_failures);
            let summary = format!(
                "Final review: {approved} approved, {} need work, {skipped} skipped\
                 {file_comment_note}{comment_note}{check_note}{local_note} — feedback saved to .claude/final-review-feedback.md",
                rejected.len()
            );
            // Optionally mirror the feedback onto the branch's GitHub PR as a
            // review (best-effort; the local file is the source of truth either
            // way).
            let pr_note = if post_to_pr {
                let postable = pr_postable_lines(&files);
                self.post_final_review_to_pr(
                    &workdir,
                    &rejected,
                    &file_comment_sections,
                    &line_comment_sections,
                    &general_feedback,
                    &postable,
                )
            } else {
                String::new()
            };
            // Dispatch the "address the feedback" prompt to the chosen target.
            // This sets `self.message` and `self.mode` (it may open the harness
            // picker when a fresh dedicated session is needed).
            self.dispatch_review_feedback(
                from_view,
                format!("{summary}{pr_note}"),
                fix_target,
                fix_target_feature_id,
                review_harness,
            );
        }
        Ok(())
    }

    /// Dispatch a finished review's "address the feedback" prompt to the agent
    /// session chosen by `fix_target`, then return to the feature view.
    ///
    /// - `ExistingLive` pastes into the reviewed feature's first agent session
    ///   (the shipped behaviour).
    /// - `DedicatedReview` reuses an existing "Final Review" session in the
    ///   reviewed feature, or — when none exists — opens the harness picker so
    ///   the reviewer chooses which harness runs the fixes.
    /// - `ExistingFeature` pastes into the first agent session of the feature
    ///   the destination picker resolved (`fix_target_feature_id`).
    /// - `NewFeature` targets the companion feature the picker created: its
    ///   "Final Review" session normally already exists; if it's gone, it is
    ///   recreated on `review_harness` (the harness the setup overlay chose).
    ///   If the companion feature itself no longer resolves, the feedback is
    ///   reported as saved and nothing is dispatched — it is never redirected
    ///   into the reviewed feature (that would defeat the isolation guarantee).
    ///
    /// Sets `self.message` and `self.mode`.
    fn dispatch_review_feedback(
        &mut self,
        from_view: ViewState,
        summary: String,
        fix_target: FixTarget,
        fix_target_feature_id: Option<String>,
        review_harness: Option<crate::project::AgentKind>,
    ) {
        let source_indices = self.store.projects.iter().enumerate().find_map(|(pi, p)| {
            if p.name != from_view.project_name {
                return None;
            }
            p.features
                .iter()
                .position(|f| f.name == from_view.feature_name)
                .map(|fi| (pi, fi))
        });

        // `ExistingFeature` / `NewFeature` route into the feature the picker
        // resolved by id; the other two stay in the reviewed feature.
        let resolved_by_id = match fix_target {
            FixTarget::ExistingFeature | FixTarget::NewFeature => fix_target_feature_id
                .as_deref()
                .and_then(|id| self.feature_indices_by_id(id)),
            FixTarget::ExistingLive | FixTarget::DedicatedReview => None,
        };

        // `NewFeature` dispatches into an *isolated* companion feature. If its
        // id no longer resolves — companion deleted, or it failed to persist,
        // between picking the destination and finishing the review — there is
        // nothing safe to fall back to: pasting into (or spinning up a "Final
        // Review" session inside) the *reviewed* feature would break the very
        // isolation guarantee the companion flow advertises. Report and park;
        // the feedback file is already written.
        if fix_target == FixTarget::NewFeature && resolved_by_id.is_none() {
            self.message = Some(format!(
                "{summary} (feedback saved; the companion review feature is gone — nothing dispatched)"
            ));
            self.mode = AppMode::Viewing(from_view);
            return;
        }

        // A stale `ExistingFeature` id (the picked feature was deleted between
        // picking and finishing) falls back to the reviewed feature so the
        // feedback still reaches an agent.
        let indices = match fix_target {
            FixTarget::ExistingFeature | FixTarget::NewFeature => resolved_by_id.or(source_indices),
            FixTarget::ExistingLive | FixTarget::DedicatedReview => source_indices,
        };
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
            FixTarget::ExistingLive | FixTarget::ExistingFeature => {
                self.message = Some(summary);
                self.mode = AppMode::Viewing(from_view);
            }
            // The companion feature resolved (the guard above returned early if
            // it hadn't), but its "Final Review" window is gone (or never came
            // up) — recreate it on the harness the setup overlay chose and
            // paste there. The feedback file is already written, so warn rather
            // than park.
            FixTarget::NewFeature => {
                match self.create_dedicated_review_session(
                    pi,
                    fi,
                    FINAL_REVIEW_SESSION_LABEL,
                    review_harness,
                    StartIntent::Warn("the review agent"),
                ) {
                    Ok(si) => {
                        let (session, window) = {
                            let feature = &self.store.projects[pi].features[fi];
                            (
                                feature.tmux_session.clone(),
                                feature.sessions[si].tmux_window.clone(),
                            )
                        };
                        let suffix = self.paste_review_prompt(&session, &window);
                        self.message =
                            Some(format!("{summary}{suffix} (companion review feature)"));
                    }
                    Err(e) => {
                        self.show_error(e);
                        self.message = Some(format!(
                            "{summary} (feedback saved; couldn't start the companion review session)"
                        ));
                    }
                }
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
    /// for the finish message. On a successful *submitted* paste, starts
    /// watching `session` (the feature's tmux session) via the thinking-status
    /// sync so a later idle transition raises a "fixes ready — re-review?"
    /// notification (see `AwaitingReviewFix` / `sync_thinking_status`). Not
    /// watched when the prompt is only pasted, not submitted — the reviewer
    /// hasn't sent it yet, so there's nothing to watch for finishing.
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
            Ok(()) if submit => {
                self.awaiting_review_fixes.insert(
                    session.to_string(),
                    AwaitingReviewFix {
                        started_thinking: false,
                    },
                );
                " — sent to agent".to_string()
            }
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
            // The feedback file is already written and the harness already
            // picked; warn rather than park, which would drop both.
            StartIntent::Warn("the review agent"),
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
        rejected: &[(String, String, Severity)],
        file_comment_sections: &[(String, FileComment)],
        line_comment_sections: &[(String, Vec<LineComment>)],
        general_feedback: &str,
        postable: &HashMap<String, HashSet<crate::diff::DiffLineLocation>>,
    ) -> String {
        use crate::github::{GhCli, PrResolution};

        let (body, comments, file_comments) = build_pr_review(
            rejected,
            file_comment_sections,
            line_comment_sections,
            general_feedback,
            postable,
        );
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
        // Map the review's severities onto a GitHub review event, but only
        // escalate past COMMENT when we can confirm the reviewer isn't the PR
        // author (GitHub rejects self approve / request-changes).
        let event = resolve_review_event(
            workdir,
            pr.number,
            rejected,
            file_comment_sections,
            line_comment_sections,
        );
        if event != "COMMENT" {
            self.log_info(
                "review",
                format!("PR review event: {event} (PR #{})", pr.number),
            );
        }
        if let Err(err) = GhCli::create_review(workdir, &pr, &body, event, &comments) {
            self.log_warn("review", format!("PR review post failed: {err}"));
            return format!(" — couldn't post to PR #{}: {err}", pr.number);
        }
        let what = if comments.is_empty() {
            "review summary".to_string()
        } else {
            format!("{} comment(s)", comments.len())
        };
        let mut suffix = format!(" — posted {what} to PR #{}", pr.number);
        // Whole-file rejections can't ride along in the batch review above
        // (GitHub's create-review endpoint has no file-level comment
        // support), so post each as its own `subject_type: file` comment.
        // Only attempted once the review itself is confirmed posted, and
        // best-effort per file so one failure doesn't drop the rest.
        if !file_comments.is_empty() {
            let mut posted = 0usize;
            let mut failed = 0usize;
            for fc in &file_comments {
                match GhCli::create_file_comment(workdir, &pr, &fc.path, &fc.body) {
                    Ok(()) => posted += 1,
                    Err(err) => {
                        failed += 1;
                        self.log_warn(
                            "review",
                            format!("PR file comment post failed ({}): {err}", fc.path),
                        );
                    }
                }
            }
            if failed == 0 {
                suffix.push_str(&format!(", {posted} file comment(s)"));
            } else {
                suffix.push_str(&format!(", {posted} file comment(s) ({failed} failed)"));
            }
        }
        suffix
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

/// Byte bounds of an inclusive, one-based line span. The returned end includes
/// the final line ending when one exists, which lets a replacement preserve the
/// file's existing EOF/newline shape.
fn content_line_bounds(
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

fn line_text_without_ending(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

/// Validate that a suggestion still points at a contiguous current-side span
/// and that the diff text under the anchor exactly matches the reviewed file
/// content. Deletion-side and mixed old/new ranges are intentionally refused:
/// they do not describe an unambiguous replacement in the worktree file.
fn plan_local_suggestion(
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

fn replacement_with_preserved_line_endings(replacement: &str, replaced: &str) -> String {
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

fn replace_content_line_span(
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

fn guarded_worktree_file(workdir: &Path, relative: &str) -> std::result::Result<PathBuf, String> {
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
fn editor_target_line(file: &crate::diff::DiffFile, cursor: Option<usize>) -> Option<usize> {
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
fn editor_needs_wait(stem: &str) -> bool {
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
fn editor_invocation(
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
fn apply_suggestions_to_file(
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

/// Re-locate one file's line comments against its freshly-loaded diff, in place.
/// Returns `(moved, lost)`: how many comments were re-anchored to a new line,
/// and how many newly lost their anchor.
///
/// A comment whose anchors still resolve exactly is left alone (and un-flagged).
/// Otherwise its captured context snippet is fuzzy-matched against the new diff.
/// A range whose `start` can no longer be found degrades to a single-line comment
/// on the re-found end rather than inventing a span. A comment with no snippet,
/// or whose snippet matches nowhere / ambiguously, is flagged `anchor_lost`.
fn reanchor_file_comments(
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
fn comment_anchor_label(file: &str, comment: &LineComment) -> String {
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

/// Map a finished review's severities onto a GitHub review event. Any
/// `Blocker` (rejection or line comment) → `REQUEST_CHANGES`; otherwise, when no
/// file was rejected, `APPROVE` (an approving review, possibly with non-blocking
/// notes); else `COMMENT`. Escalating past `COMMENT` requires confirming the
/// reviewer is **not** the PR author — GitHub 422s a self approve /
/// request-changes — so a self-review, or an inconclusive author check, stays
/// `COMMENT` (always valid). Best-effort by design: the local feedback file is
/// the source of truth regardless.
fn resolve_review_event(
    workdir: &Path,
    pr_number: u32,
    rejected: &[(String, String, Severity)],
    file_comment_sections: &[(String, FileComment)],
    line_comment_sections: &[(String, Vec<LineComment>)],
) -> &'static str {
    let escalated = severity_review_event(rejected, file_comment_sections, line_comment_sections);
    if escalated == "COMMENT" {
        return "COMMENT";
    }
    match crate::github::GhCli::is_self_review(workdir, pr_number) {
        Ok(false) => escalated,
        // Self-review, or we couldn't tell → the always-safe COMMENT.
        _ => "COMMENT",
    }
}

/// The GitHub review event a finished review's severities *want*, before the
/// self-review guard: any `Blocker` → `REQUEST_CHANGES`; else, with no file
/// rejected, `APPROVE`; else `COMMENT`.
fn severity_review_event(
    rejected: &[(String, String, Severity)],
    file_comment_sections: &[(String, FileComment)],
    line_comment_sections: &[(String, Vec<LineComment>)],
) -> &'static str {
    let has_blocker = rejected.iter().any(|(_, _, s)| s.is_blocker())
        || file_comment_sections
            .iter()
            .any(|(_, comment)| comment.severity.is_blocker())
        || line_comment_sections
            .iter()
            .flat_map(|(_, cs)| cs)
            .any(|c| c.severity.is_blocker());
    if has_blocker {
        "REQUEST_CHANGES"
    } else if rejected.is_empty() {
        "APPROVE"
    } else {
        "COMMENT"
    }
}

/// The diff lines GitHub will accept an inline review comment on, per file:
/// the ones in the diff git actually produced. `DiffFile::patch` preserves that
/// verbatim no matter how far the reviewer expanded the *rendered* context
/// (`hunks_with_context` rewrites `hunks` only), so re-parsing it recovers the
/// real boundary. This matters because `create_review` posts every inline
/// comment in one batch — a single anchor outside the PR's diff would reject
/// the whole review, losing the comments that were postable.
///
/// A file whose patch can't be re-parsed is left out of the map entirely, which
/// callers read as "unrestricted" — the pre-expansion behaviour.
fn pr_postable_lines(
    files: &[crate::diff::DiffFile],
) -> HashMap<String, HashSet<crate::diff::DiffLineLocation>> {
    let mut out = HashMap::new();
    for file in files {
        let Ok(parsed) = crate::diff::parse_unified_diff(&file.patch) else {
            continue;
        };
        let Some(original) = parsed.first() else {
            continue;
        };
        out.insert(
            file.path.clone(),
            original.addressable_lines().into_iter().collect(),
        );
    }
    out
}

/// Assemble a GitHub PR review from a finished final review. Line comments
/// become inline review comments — anchored to the current file line
/// (`RIGHT`) or, for a deletion-only line, the base file line (`LEFT`).
/// Whole-file rejections have no single line to anchor to either, but *do*
/// have a file — they become `subject_type: file` comments instead of being
/// dumped into the summary body (GitHub's batch review endpoint can't carry
/// those, so the caller posts them as separate `create_file_comment` calls).
/// The summary body carries only the general feedback. `postable` bounds which
/// lines may be commented on inline (see `pr_postable_lines`); a path with no
/// entry is unrestricted. Returns `(body, comments, file_comments)`.
fn build_pr_review(
    rejected: &[(String, String, Severity)],
    file_comment_sections: &[(String, FileComment)],
    line_comment_sections: &[(String, Vec<LineComment>)],
    general_feedback: &str,
    postable: &HashMap<String, HashSet<crate::diff::DiffLineLocation>>,
) -> (
    String,
    Vec<crate::github::PrReviewComment>,
    Vec<crate::github::PrFileComment>,
) {
    let mut comments = Vec::new();
    for (path, file_comments) in line_comment_sections {
        let allowed = postable.get(path);
        let in_diff = |loc: &crate::diff::DiffLineLocation| {
            allowed.is_none_or(|allowed| allowed.contains(loc))
        };
        for comment in file_comments {
            // A comment we couldn't re-anchor holds a stale line number; posting
            // it inline would pin it to the wrong line. Omit it — the local
            // feedback file still carries it, flagged "anchor lost".
            if comment.anchor_lost {
                continue;
            }
            // Likewise for a line the reviewer only reached by expanding the
            // rendered context: it's valid local feedback, but GitHub rejects
            // an inline comment outside the PR's own diff.
            if !in_diff(&comment.location) {
                continue;
            }
            let Some((line, side)) = pr_line_side(&comment.location) else {
                continue;
            };
            // For a span, anchor the start with GitHub's start_line/start_side.
            // Drop the range (post as a single-line comment at the end) if the
            // start can't be mapped to a line/side — or falls outside the diff.
            let (start_line, start_side) = comment
                .start
                .filter(|s| in_diff(s))
                .and_then(|s| pr_line_side(&s))
                .map(|(l, sd)| (Some(l), Some(sd)))
                .unwrap_or((None, None));
            // Lead with the conventional-comments severity tag so the priority is
            // visible on the PR (GitHub has no native severity field).
            let mut body = format!("**[{}]**", comment.severity.label());
            if !comment.text.is_empty() {
                body.push(' ');
                body.push_str(&comment.text);
            }
            // A suggested change posts as a GitHub fenced ```suggestion block so
            // it's one-click-appliable on the PR. Append it to any prose.
            if let Some(suggestion) = &comment.suggestion {
                body.push_str(&format!("\n\n```suggestion\n{suggestion}\n```"));
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

    let body = general_feedback.trim().to_string();

    // Whole-file rejections carry the same conventional-comments severity
    // tag as line comments, with a filler line when the reviewer left no
    // feedback text (mirrors the old body-dump's bare "needs revision").
    let mut file_comments: Vec<crate::github::PrFileComment> = rejected
        .iter()
        .map(|(file, feedback, severity)| {
            let feedback = feedback.trim();
            let tag = severity.label();
            let body = if feedback.is_empty() {
                format!("**[{tag}]** Needs revision.")
            } else {
                format!("**[{tag}]** {feedback}")
            };
            crate::github::PrFileComment {
                path: file.clone(),
                body,
            }
        })
        .collect();
    file_comments.extend(file_comment_sections.iter().map(|(file, comment)| {
        crate::github::PrFileComment {
            path: file.clone(),
            body: format!("**[{}]** {}", comment.severity.label(), comment.text.trim()),
        }
    }));

    (body, comments, file_comments)
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

/// Start a persisted review round with the metadata shared by both actionable
/// feedback rounds and all-approved rounds. Keeping successful rounds too is
/// what lets the history browser represent the complete review conversation
/// instead of silently omitting its positive outcomes.
#[allow(clippy::too_many_arguments)]
fn review_round_preamble(
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
const MAX_LIVE_ROUNDS: usize = 2;

/// Split a feedback log's body (title already stripped) into per-round
/// chunks, each starting at a `## Review` heading line and running up to (not
/// including) the next one.
fn split_rounds(body: &str) -> Vec<String> {
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
fn parse_review_history_rounds(content: &str, document_title: &str) -> Vec<ReviewHistoryRound> {
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
fn split_overflow_rounds(content: &str, keep: usize) -> (String, Option<String>) {
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
fn anchor_file_path(anchor: &str) -> &str {
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
fn parse_agent_responses(
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

/// The `{{file_path}}` / `{{patch}}` context for `PromptId::ReviewWalkthrough`.
/// Large patches are truncated here to keep the headless request bounded.
fn walkthrough_context(file: &crate::diff::DiffFile) -> crate::prompts::PromptContext {
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
    crate::prompts::PromptContext::new()
        .with("file_path", file.path.clone())
        .with("patch", patch)
}

/// The `{{file_path}}` / `{{annotated_body}}` context for
/// `PromptId::ReviewCoReview`. The diff is rendered with each current-side
/// line tagged by its **new** line number so the model can anchor findings
/// precisely, and bounded like the walkthrough so a large file can't blow up
/// token cost.
fn co_review_context(file: &crate::diff::DiffFile) -> crate::prompts::PromptContext {
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

    crate::prompts::PromptContext::new()
        .with("file_path", file.path.clone())
        .with("annotated_body", body)
}

/// The `{{files_block}}` context for `PromptId::ReviewChangesetOverview`.
/// Bounded on two axes so a large changeset can't produce an unbounded
/// headless request: the file list is capped at `MAX_FILES`, and each
/// included file's patch gets a much smaller budget than the single-file
/// walkthrough's, since this prompt aggregates many files at once.
fn changeset_overview_context(files: &[crate::diff::DiffFile]) -> crate::prompts::PromptContext {
    const MAX_FILES: usize = 30;
    const MAX_PATCH_PER_FILE: usize = 400;
    const MAX_TOTAL: usize = 16000;

    let mut body = String::new();
    let included = files.iter().filter(|f| !f.is_binary).take(MAX_FILES);
    let included_count = included.clone().count();
    for file in included {
        let mut patch = if file.patch.trim().is_empty() {
            file.new_content.clone().unwrap_or_default()
        } else {
            file.patch.clone()
        };
        if patch.len() > MAX_PATCH_PER_FILE {
            patch.truncate(MAX_PATCH_PER_FILE);
            patch.push_str("\n… (truncated)");
        }
        body.push_str(&format!(
            "### {} (+{} -{})\n```diff\n{}\n```\n\n",
            file.path, file.additions, file.deletions, patch
        ));
        if body.len() > MAX_TOTAL {
            body.truncate(MAX_TOTAL);
            body.push_str("\n… (changeset truncated)\n\n");
            break;
        }
    }
    let remaining = files.len().saturating_sub(included_count);
    if remaining > 0 {
        body.push_str(&format!("… and {remaining} more file(s) not shown.\n"));
    }

    crate::prompts::PromptContext::new().with("files_block", body)
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
            severity: crate::app::Severity::default(),
            anchor_context: None,
            start_anchor_context: None,
            anchor_lost: false,
            // A draft is not a thread until the human accepts it.
            resolved: false,
            carried: false,
        });
    }
    out
}

fn review_note_path_from_heading(line: &str) -> Option<String> {
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
fn review_note_sections(content: &str) -> (String, Vec<(String, String)>) {
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

fn push_review_note_chunk(out: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(chunk);
}

fn write_review_notes_atomic(path: &Path, content: &str) -> Result<()> {
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
fn split_overflow_review_notes(content: &str, keep: usize) -> (String, Option<String>) {
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

#[cfg(test)]
mod tests {
    use super::{
        CHECK_OUTPUT_MAX_CHARS, anchor_file_path, apply_suggestions_to_file, archive_review_notes,
        build_pr_review, comment_anchor_label, compose_feedback_log, compute_search_matches,
        editor_invocation, editor_target_line, load_review_notes, parse_agent_responses,
        parse_co_review_output, parse_review_history_rounds, parse_review_notes, pr_postable_lines,
        reanchor_file_comments, severity_review_event, split_overflow_review_notes,
        split_overflow_rounds, truncate_check_output, walkthrough_context,
    };
    use std::collections::{HashMap, HashSet};

    use crate::app::state::DiffViewerState;
    use crate::app::{CommentAnchorContext, FileComment, LineComment, Severity};
    use crate::diff::{
        DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind, DiffLineLocation,
        parse_unified_diff,
    };

    fn line_comment(new_line: Option<usize>, old_line: Option<usize>, text: &str) -> LineComment {
        LineComment {
            location: DiffLineLocation { old_line, new_line },
            start: None,
            text: text.to_string(),
            draft: false,
            suggestion: None,
            severity: Severity::default(),
            anchor_context: None,
            start_anchor_context: None,
            anchor_lost: false,
            resolved: false,
            carried: false,
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
            severity: Severity::default(),
            anchor_context: None,
            start_anchor_context: None,
            anchor_lost: false,
            resolved: false,
            carried: false,
        }
    }

    fn local_apply_file(content: &str) -> DiffFile {
        DiffFile {
            old_path: Some("src/example.rs".to_string()),
            path: "src/example.rs".to_string(),
            status: DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: Some("old\ncontent\n".to_string()),
            new_content: Some(content.to_string()),
            patch: String::new(),
            hunks: vec![DiffHunk {
                header: "@@ -1,3 +1,3 @@".to_string(),
                old_start: 1,
                old_lines: 3,
                new_start: 1,
                new_lines: 3,
                lines: vec![
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: " one".to_string(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "+two".to_string(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: " three".to_string(),
                    },
                ],
            }],
        }
    }

    #[test]
    fn local_suggestion_replaces_range_and_preserves_crlf() {
        let workdir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(workdir.path().join("src")).unwrap();
        let content = "one\r\ntwo\r\nthree\r\n";
        std::fs::write(workdir.path().join("src/example.rs"), content).unwrap();
        let file = local_apply_file(content);
        let locations = file.addressable_lines();
        let mut comment = line_comment(Some(3), Some(3), "replace both");
        comment.start = Some(locations[1]);
        comment.location = locations[2];
        comment.suggestion = Some("TWO\nTHREE".to_string());

        let report = apply_suggestions_to_file(workdir.path(), &file, &[(0, comment)]);

        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(report.applied.len(), 1);
        assert_eq!(
            std::fs::read_to_string(workdir.path().join("src/example.rs")).unwrap(),
            "one\r\nTWO\r\nTHREE\r\n"
        );
    }

    #[test]
    fn local_suggestion_refuses_a_file_changed_since_diff_load() {
        let workdir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(workdir.path().join("src")).unwrap();
        let reviewed = "one\ntwo\nthree\n";
        let live = "one\nchanged elsewhere\nthree\n";
        std::fs::write(workdir.path().join("src/example.rs"), live).unwrap();
        let file = local_apply_file(reviewed);
        let mut comment = line_comment(Some(2), None, "replace it");
        comment.suggestion = Some("TWO".to_string());

        let report = apply_suggestions_to_file(workdir.path(), &file, &[(0, comment)]);

        assert!(report.applied.is_empty());
        assert!(report.failures[0].contains("changed since the diff was loaded"));
        assert_eq!(
            std::fs::read_to_string(workdir.path().join("src/example.rs")).unwrap(),
            live
        );
    }

    #[test]
    fn local_suggestion_refuses_deletion_side_anchor() {
        let workdir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(workdir.path().join("src")).unwrap();
        std::fs::write(workdir.path().join("src/example.rs"), "one\n").unwrap();
        let mut file = local_apply_file("one\n");
        file.hunks = vec![DiffHunk {
            header: "@@ -1,2 +1 @@".to_string(),
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 1,
            lines: vec![DiffLine {
                kind: DiffLineKind::Removed,
                text: "-gone".to_string(),
            }],
        }];
        let mut comment = line_comment(None, Some(1), "replace deletion");
        comment.suggestion = Some("replacement".to_string());

        let report = apply_suggestions_to_file(workdir.path(), &file, &[(0, comment)]);

        assert!(report.applied.is_empty());
        assert!(report.failures[0].contains("deletion-only line"));
        assert_eq!(
            std::fs::read_to_string(workdir.path().join("src/example.rs")).unwrap(),
            "one\n"
        );
    }

    #[test]
    fn local_suggestion_batch_applies_bottom_up_when_line_counts_change() {
        let workdir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(workdir.path().join("src")).unwrap();
        let content = "one\ntwo\nthree\n";
        std::fs::write(workdir.path().join("src/example.rs"), content).unwrap();
        let file = local_apply_file(content);
        let locations = file.addressable_lines();
        let mut second = line_comment(Some(2), None, "expand two");
        second.location = locations[1];
        second.suggestion = Some("TWO-A\nTWO-B".to_string());
        let mut third = line_comment(Some(3), Some(3), "replace three");
        third.location = locations[2];
        third.suggestion = Some("THREE".to_string());

        let report = apply_suggestions_to_file(workdir.path(), &file, &[(0, second), (1, third)]);

        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(report.applied.len(), 2);
        assert_eq!(
            std::fs::read_to_string(workdir.path().join("src/example.rs")).unwrap(),
            "one\nTWO-A\nTWO-B\nTHREE\n"
        );
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
    fn pr_review_maps_lines_inline_and_files_to_file_comments() {
        let rejected = vec![
            (
                "src/a.rs".to_string(),
                "tighten this up".to_string(),
                Severity::Suggestion,
            ),
            ("src/b.rs".to_string(), String::new(), Severity::Blocker),
        ];
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![
                line_comment(Some(42), None, "off-by-one"),
                line_comment(None, Some(7), "why delete this?"),
            ],
        )];
        let (body, comments, file_comments) = build_pr_review(
            &rejected,
            &[],
            &line_comments,
            "overall LGTM-ish",
            &HashMap::new(),
        );

        // Inline comments: an added/current line posts on RIGHT, a deletion-only
        // line on LEFT.
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].path, "src/c.rs");
        assert_eq!(comments[0].line, 42);
        assert_eq!(comments[0].side, "RIGHT");
        assert_eq!(comments[1].line, 7);
        assert_eq!(comments[1].side, "LEFT");

        // Body carries only the general feedback now — whole-file rejections
        // post as their own file-level comments instead of being dumped here.
        assert_eq!(body, "overall LGTM-ish");
        assert!(!body.contains("Files needing revision"));

        // Whole-file rejections: one `PrFileComment` per file, tagged with its
        // severity, with a filler line when no feedback text was given.
        assert_eq!(file_comments.len(), 2);
        assert_eq!(file_comments[0].path, "src/a.rs");
        assert_eq!(file_comments[0].body, "**[suggestion]** tighten this up");
        assert_eq!(file_comments[1].path, "src/b.rs");
        assert_eq!(file_comments[1].body, "**[blocker]** Needs revision.");
    }

    #[test]
    fn pr_review_body_empty_when_only_line_comments() {
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![line_comment(Some(3), None, "nit")],
        )];
        let (body, comments, file_comments) =
            build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
        assert!(body.is_empty());
        assert_eq!(comments.len(), 1);
        assert!(file_comments.is_empty());
    }

    #[test]
    fn pr_review_skips_a_comment_on_a_line_outside_the_prs_diff() {
        // Expanding the rendered context lets the reviewer comment on a line
        // git's own diff never emitted. GitHub rejects an inline comment there,
        // and `create_review` posts the batch atomically — so it must be
        // dropped rather than allowed to sink the whole review.
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![
                line_comment(Some(3), None, "in the diff"),
                line_comment(Some(90), None, "only visible after expanding"),
            ],
        )];
        let postable: HashMap<String, HashSet<crate::diff::DiffLineLocation>> = [(
            "src/c.rs".to_string(),
            HashSet::from([crate::diff::DiffLineLocation {
                old_line: None,
                new_line: Some(3),
            }]),
        )]
        .into_iter()
        .collect();

        let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &postable);

        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].line, 3);
        // With no entry for the path the map imposes no restriction, which is
        // the pre-expansion behaviour.
        let (_, unrestricted, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
        assert_eq!(unrestricted.len(), 2);
    }

    #[test]
    fn pr_review_drops_a_range_whose_start_is_outside_the_prs_diff() {
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![ranged_comment(10, 14, "this whole block")],
        )];
        // The end is in the diff but the start was only revealed by expansion:
        // post it as a single-line comment rather than an invalid span.
        let postable: HashMap<String, HashSet<crate::diff::DiffLineLocation>> = [(
            "src/c.rs".to_string(),
            HashSet::from([crate::diff::DiffLineLocation {
                old_line: None,
                new_line: Some(14),
            }]),
        )]
        .into_iter()
        .collect();

        let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &postable);

        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].line, 14);
        assert_eq!(comments[0].start_line, None);
        assert_eq!(comments[0].start_side, None);
    }

    #[test]
    fn pr_postable_lines_tracks_the_original_patch_not_the_expanded_hunks() {
        let old: String = (1..=30).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l15\n", "l15 changed\n");
        let mut file = crate::diff::parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -12,7 +12,7 @@
 l12
 l13
 l14
-l15
+l15 changed
 l16
 l17
 l18
",
        )
        .unwrap()
        .pop()
        .unwrap();
        file.old_content = Some(old);
        file.new_content = Some(new);

        // Expansion rewrites `hunks` but deliberately leaves `patch` alone, so
        // the postable set still describes git's real diff.
        file.hunks = file.hunks_with_context(usize::MAX).unwrap();
        assert_eq!(file.addressable_lines().len(), 31);

        let postable = pr_postable_lines(std::slice::from_ref(&file));
        let allowed = &postable["a.rs"];
        assert_eq!(allowed.len(), 8);
        assert!(allowed.contains(&crate::diff::DiffLineLocation {
            old_line: Some(12),
            new_line: Some(12)
        }));
        assert!(!allowed.contains(&crate::diff::DiffLineLocation {
            old_line: Some(1),
            new_line: Some(1)
        }));
    }

    #[test]
    fn pr_review_emits_start_line_for_a_ranged_comment() {
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![ranged_comment(10, 14, "this whole block")],
        )];
        let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
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
        let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
        assert_eq!(comments[0].start_line, None);
        assert_eq!(comments[0].start_side, None);
    }

    #[test]
    fn pr_review_appends_suggestion_block_to_comment_body() {
        let mut comment = line_comment(Some(5), None, "use a guard");
        comment.suggestion = Some("let x = y?;".to_string());
        let line_comments = vec![("src/c.rs".to_string(), vec![comment])];
        let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
        assert_eq!(comments.len(), 1);
        // The comment body leads with the conventional-comments severity tag.
        assert_eq!(
            comments[0].body,
            "**[suggestion]** use a guard\n\n```suggestion\nlet x = y?;\n```"
        );
    }

    #[test]
    fn pr_review_suggestion_only_comment_is_just_the_block() {
        let mut comment = line_comment(Some(5), None, "");
        comment.suggestion = Some("let x = y?;".to_string());
        let line_comments = vec![("src/c.rs".to_string(), vec![comment])];
        let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
        // Even a suggestion-only comment carries its severity tag.
        assert_eq!(
            comments[0].body,
            "**[suggestion]**\n\n```suggestion\nlet x = y?;\n```"
        );
    }

    fn blocker_comment(new_line: usize, text: &str) -> LineComment {
        let mut c = line_comment(Some(new_line), None, text);
        c.severity = Severity::Blocker;
        c
    }

    #[test]
    fn severity_event_requests_changes_on_a_blocker() {
        // A blocker rejection escalates.
        let rejected = vec![("a.rs".to_string(), "no".to_string(), Severity::Blocker)];
        assert_eq!(
            severity_review_event(&rejected, &[], &[]),
            "REQUEST_CHANGES"
        );
        // A blocker line comment escalates even with no rejection.
        let sections = vec![("a.rs".to_string(), vec![blocker_comment(3, "must fix")])];
        assert_eq!(
            severity_review_event(&[], &[], &sections),
            "REQUEST_CHANGES"
        );
    }

    #[test]
    fn severity_event_approves_with_no_rejections_else_comments() {
        // No rejection, only non-blocking notes → an approving review.
        let sections = vec![("a.rs".to_string(), vec![line_comment(Some(3), None, "nit")])];
        assert_eq!(severity_review_event(&[], &[], &sections), "APPROVE");
        assert_eq!(severity_review_event(&[], &[], &[]), "APPROVE");
        // A non-blocking rejection is a plain comment review.
        let rejected = vec![("a.rs".to_string(), "meh".to_string(), Severity::Suggestion)];
        assert_eq!(severity_review_event(&rejected, &[], &[]), "COMMENT");
    }

    #[test]
    fn feedback_file_comment_tags_whole_file_rejection_with_severity() {
        // A whole-file rejection's file-level comment carries its severity tag.
        let rejected = vec![("a.rs".to_string(), "fix".to_string(), Severity::Blocker)];
        let (_, _, file_comments) = build_pr_review(&rejected, &[], &[], "", &HashMap::new());
        assert_eq!(file_comments.len(), 1);
        assert_eq!(file_comments[0].path, "a.rs");
        assert_eq!(file_comments[0].body, "**[blocker]** fix");
    }

    #[test]
    fn verdict_free_file_comment_posts_and_can_escalate() {
        let comments = vec![(
            "src/module.rs".to_string(),
            FileComment {
                text: "This module should be split.".to_string(),
                severity: Severity::Blocker,
                resolved: false,
                carried: false,
            },
        )];
        let (body, inline, file_comments) =
            build_pr_review(&[], &comments, &[], "", &HashMap::new());
        assert!(body.is_empty());
        assert!(inline.is_empty());
        assert_eq!(file_comments.len(), 1);
        assert_eq!(file_comments[0].path, "src/module.rs");
        assert_eq!(
            file_comments[0].body,
            "**[blocker]** This module should be split."
        );
        assert_eq!(
            severity_review_event(&[], &comments, &[]),
            "REQUEST_CHANGES"
        );
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
    fn split_overflow_rounds_keeps_everything_under_the_cap() {
        let content = compose_feedback_log(
            Some("# Final Review Feedback\n\n## Review — r1\n\nold.\n\n"),
            "## Review — r2\n\nnew.\n\n",
        );
        let (live, overflow) = split_overflow_rounds(&content, 2);
        assert_eq!(live, content);
        assert!(overflow.is_none());
    }

    #[test]
    fn split_overflow_rounds_moves_rounds_past_the_cap_to_the_archive() {
        let existing = compose_feedback_log(
            Some("# Final Review Feedback\n\n## Review — r1\n\noldest.\n\n"),
            "## Review — r2\n\nmiddle.\n\n",
        );
        let content = compose_feedback_log(Some(&existing), "## Review — r3\n\nnewest.\n\n");
        let (live, overflow) = split_overflow_rounds(&content, 2);

        // The two newest rounds stay live, still under a single title.
        assert!(live.starts_with("# Final Review Feedback\n\n## Review — r3"));
        assert!(live.contains("newest."));
        assert!(live.contains("## Review — r2"));
        assert!(live.contains("middle."));
        assert!(!live.contains("oldest."));

        // The oldest round is pushed out for the caller to archive.
        let overflow = overflow.expect("oldest round should overflow");
        assert!(overflow.contains("## Review — r1"));
        assert!(overflow.contains("oldest."));
    }

    #[test]
    fn history_round_parser_preserves_markdown_and_carried_thread_count() {
        let content = "\
# Final Review Feedback

## Review — 2026-07-24T12:00:00Z

**Files reviewed:** 2

#### src/a.rs:7 — [blocker] (unresolved from a previous round)

Fix this.

**Agent:** Fixed in src/a.rs.

## Review — 2026-07-23T12:00:00Z

**Check:** `cargo test` — passed
";
        let rounds = parse_review_history_rounds(content, super::FEEDBACK_TITLE);
        assert_eq!(rounds.len(), 2);
        assert_eq!(rounds[0].title, "Review — 2026-07-24T12:00:00Z");
        assert_eq!(rounds[0].carried_unresolved, 1);
        assert!(rounds[0].markdown.contains("**Agent:** Fixed"));
        assert!(rounds[1].markdown.contains("cargo test"));
    }

    #[test]
    fn truncate_check_output_passes_short_output_through() {
        assert_eq!(truncate_check_output("all good"), "all good");
    }

    #[test]
    fn truncate_check_output_caps_long_output() {
        let long = "x".repeat(CHECK_OUTPUT_MAX_CHARS + 500);
        let out = truncate_check_output(&long);
        assert!(out.starts_with(&"x".repeat(CHECK_OUTPUT_MAX_CHARS)));
        assert!(out.ends_with("… (truncated)"));
        assert_eq!(
            out.chars().count(),
            CHECK_OUTPUT_MAX_CHARS + "\n… (truncated)".chars().count()
        );
    }

    #[test]
    fn anchor_file_path_strips_line_suffix() {
        assert_eq!(anchor_file_path("src/foo.rs"), "src/foo.rs");
        assert_eq!(anchor_file_path("src/foo.rs:42"), "src/foo.rs");
        assert_eq!(anchor_file_path("src/foo.rs:42-48"), "src/foo.rs");
        assert_eq!(anchor_file_path("src/foo.rs:42 (base)"), "src/foo.rs");
        // A colon not followed by a digit is part of the path, not a suffix.
        assert_eq!(anchor_file_path("weird:name.rs"), "weird:name.rs");
    }

    #[test]
    fn parse_agent_responses_groups_replies_by_file() {
        let feedback = "\
# Final Review Feedback

## Review — 2026-07-02T00:00:00Z

### Files Needing Revision

#### src/foo.rs — [blocker]

Needs error handling.

**Agent:** fixed in src/foo.rs — added a match on the Result.

### Line Comments

#### src/foo.rs:42 — [suggestion]

Rename this variable.

**Agent:** done, renamed to `count`.

#### src/bar.rs:10-12 — [question]

Why the loop?

**Agent:** disagree — the loop is needed for the retry.
";
        let out = parse_agent_responses(feedback);
        let foo = out.get("src/foo.rs").expect("foo replies");
        assert_eq!(foo.len(), 2);
        assert_eq!(foo[0].anchor, "src/foo.rs");
        assert!(foo[0].response.contains("added a match"));
        assert_eq!(foo[1].anchor, "src/foo.rs:42");
        assert!(foo[1].response.contains("renamed to"));
        let bar = out.get("src/bar.rs").expect("bar replies");
        assert_eq!(bar[0].anchor, "src/bar.rs:10-12");
        assert!(bar[0].response.contains("disagree"));
    }

    #[test]
    fn parse_agent_responses_only_reads_latest_round() {
        // Older rounds (below the first `## Review`) must not leak in.
        let feedback = "\
# Final Review Feedback

## Review — 2026-07-02T00:00:00Z

### Line Comments

#### src/foo.rs:5 — [nit]

New item.

## Review — 2026-07-01T00:00:00Z

### Line Comments

#### src/old.rs:9 — [blocker]

Old item.

**Agent:** addressed last round.
";
        let out = parse_agent_responses(feedback);
        // The newest round's item has no reply; the old round's reply is ignored.
        assert!(out.is_empty(), "only the latest round is parsed, {out:?}");
    }

    #[test]
    fn parse_agent_responses_skips_item_text_without_reply() {
        let feedback = "\
# Final Review Feedback

## Review — 2026-07-02T00:00:00Z

### Line Comments

#### src/foo.rs:5 — [nit]

Just a comment, no agent reply yet.
";
        assert!(parse_agent_responses(feedback).is_empty());
    }

    #[test]
    fn parse_agent_responses_captures_multiline_reply() {
        let feedback = "\
# Final Review Feedback

## Review — 2026-07-02T00:00:00Z

### Line Comments

#### src/foo.rs:5 — [suggestion]

Do the thing.

**Agent:** first line of reply.
second line of reply.
";
        let out = parse_agent_responses(feedback);
        let reply = &out.get("src/foo.rs").unwrap()[0].response;
        assert!(reply.contains("first line"));
        assert!(reply.contains("second line"));
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
        let prompt = crate::prompts::render_template(
            crate::prompts::PromptId::ReviewWalkthrough
                .spec()
                .default_template,
            &walkthrough_context(&file),
        );
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
        let prompt = crate::prompts::render_template(
            crate::prompts::PromptId::ReviewWalkthrough
                .spec()
                .default_template,
            &walkthrough_context(&file),
        );
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

    #[test]
    fn review_notes_cap_keeps_latest_unique_files_and_archives_superseded_notes() {
        let content = "\
# Optional preamble

## src/old.rs — first

Old note.

---

## src/keep.rs — first

Superseded note.

---

## src/new.rs — current

Newest file.

---

## src/keep.rs — updated

Current note.

---
";
        let (live, overflow) = split_overflow_review_notes(content, 2);

        assert!(live.starts_with("# Optional preamble"));
        assert!(live.contains("## src/new.rs — current"));
        assert!(live.contains("## src/keep.rs — updated"));
        assert!(!live.contains("Superseded note."));
        assert!(!live.contains("Old note."));

        let overflow = overflow.expect("old and superseded notes should overflow");
        assert!(!overflow.contains("# Optional preamble"));
        assert!(overflow.contains("## src/old.rs — first"));
        assert!(overflow.contains("## src/keep.rs — first"));
        assert!(!overflow.contains("## src/keep.rs — updated"));
    }

    #[test]
    fn review_notes_cap_is_noop_when_preamble_present_but_unique_notes_fit() {
        let content =
            "# Optional preamble\n\n## src/a.rs — a\n\nA.\n\n---\n\n## src/b.rs — b\n\nB.\n";
        let (live, overflow) = split_overflow_review_notes(content, 2);
        assert_eq!(live, content);
        assert!(overflow.is_none());
    }

    #[test]
    fn review_notes_cap_is_noop_when_unique_notes_fit() {
        let content = "## src/a.rs — a\n\nA.\n\n---\n\n## src/b.rs — b\n\nB.\n";
        let (live, overflow) = split_overflow_review_notes(content, 2);
        assert_eq!(live, content);
        assert!(overflow.is_none());
    }

    #[test]
    fn blind_appended_duplicates_collapse_below_the_cap() {
        // Option F: the agent appends notes without reading the file, so within
        // one session it can append several sections for the same file. The
        // per-turn archive pass must collapse them to the newest even when the
        // unique-file count is nowhere near MAX_LIVE_REVIEW_NOTE_FILES — i.e.
        // with no cap pressure at all, only supersession.
        let content = "\
## src/main.rs — first pass

Sketched the entry point.

---

## src/lib.rs — helper

Added a helper.

---

## src/main.rs — second pass

Wired the entry point to the helper.

---
";
        let (live, overflow) = split_overflow_review_notes(content, 50);

        assert!(live.contains("## src/lib.rs — helper"));
        assert!(live.contains("## src/main.rs — second pass"));
        assert!(!live.contains("Sketched the entry point."));

        let overflow = overflow.expect("the superseded src/main.rs note should archive");
        assert!(overflow.contains("Sketched the entry point."));
        assert!(!overflow.contains("second pass"));
    }

    #[test]
    fn archived_review_notes_remain_visible_but_live_note_wins() {
        let dir = tempfile::TempDir::new().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("review-notes-archive.md"),
            "# Review Notes Archive\n\n\
             ## src/archived.rs — old\n\nArchived context.\n\n---\n\n\
             ## src/live.rs — stale\n\nStale context.\n",
        )
        .unwrap();
        std::fs::write(
            claude.join("review-notes.md"),
            "## src/live.rs — current\n\nCurrent context.\n",
        )
        .unwrap();

        let notes = load_review_notes(dir.path());
        assert_eq!(
            notes.get("src/archived.rs").map(String::as_str),
            Some("Archived context.")
        );
        assert_eq!(
            notes.get("src/live.rs").map(String::as_str),
            Some("Current context.")
        );
    }

    #[test]
    fn archive_review_notes_moves_overflow_and_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let mut content = String::new();
        for index in 0..=super::MAX_LIVE_REVIEW_NOTE_FILES {
            content.push_str(&format!(
                "## src/file-{index}.rs — note\n\nNote {index}.\n\n---\n\n"
            ));
        }
        std::fs::write(claude.join("review-notes.md"), content).unwrap();

        assert_eq!(archive_review_notes(dir.path()).unwrap(), 1);
        assert_eq!(archive_review_notes(dir.path()).unwrap(), 0);

        let live = std::fs::read_to_string(claude.join("review-notes.md")).unwrap();
        assert!(!live.contains("## src/file-0.rs"));
        assert!(live.contains(&format!(
            "## src/file-{}.rs",
            super::MAX_LIVE_REVIEW_NOTE_FILES
        )));
        let archive = std::fs::read_to_string(claude.join("review-notes-archive.md")).unwrap();
        assert_eq!(archive.matches("# Review Notes Archive").count(), 1);
        assert!(archive.contains("## src/file-0.rs"));
    }

    #[test]
    fn archive_review_notes_collapses_blind_appended_duplicates_on_disk() {
        // End-to-end P0 for Option F: a blind-append turn that re-notes the same
        // file must leave only the newest section live, with the stale copy in
        // the archive and load_review_notes returning the current note. No cap
        // is hit here — two sections, one path.
        let dir = tempfile::TempDir::new().unwrap();
        let claude = dir.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("review-notes.md"),
            "## src/a.rs — first\n\nEarly note.\n\n---\n\n\
             ## src/a.rs — refined\n\nLater, fuller note.\n\n---\n",
        )
        .unwrap();

        assert_eq!(archive_review_notes(dir.path()).unwrap(), 1);
        assert_eq!(archive_review_notes(dir.path()).unwrap(), 0);

        let live = std::fs::read_to_string(claude.join("review-notes.md")).unwrap();
        assert!(live.contains("Later, fuller note."));
        assert!(!live.contains("Early note."));

        let notes = load_review_notes(dir.path());
        assert_eq!(
            notes.get("src/a.rs").map(String::as_str),
            Some("Later, fuller note.")
        );
    }

    const SEARCH_PATCH: &str = "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,4 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
+fn gamma_alpha() {}
 fn delta() {}
";

    #[test]
    fn compute_search_matches_finds_case_insensitive_substrings() {
        let files = parse_unified_diff(SEARCH_PATCH).unwrap();
        let file = &files[0];
        // Addressable lines: 0 alpha (ctx), 1 beta (del), 2 beta_two (add),
        // 3 gamma_alpha (add), 4 delta (ctx).
        assert_eq!(compute_search_matches(file, "alpha"), vec![0, 3]);
        assert_eq!(compute_search_matches(file, "beta"), vec![1, 2]);
        // Case-insensitive.
        assert_eq!(compute_search_matches(file, "ALPHA"), vec![0, 3]);
    }

    #[test]
    fn compute_search_matches_empty_query_or_no_hit_is_empty() {
        let files = parse_unified_diff(SEARCH_PATCH).unwrap();
        let file = &files[0];
        assert!(compute_search_matches(file, "").is_empty());
        assert!(compute_search_matches(file, "   ").is_empty());
        assert!(compute_search_matches(file, "nonexistent").is_empty());
    }

    #[test]
    fn on_file_changed_clears_active_search() {
        let mut state = DiffViewerState::new(
            crate::app::state::ViewState::new(
                "p".into(),
                "f".into(),
                "s".into(),
                "w".into(),
                "Claude".into(),
                crate::project::SessionKind::Claude,
                crate::project::VibeMode::Vibeless,
                true,
            ),
            std::path::PathBuf::from("/tmp"),
        );
        state.search_query = "alpha".into();
        state.search_matches = vec![0, 3];
        state.search_match_pos = Some(1);
        state.editing_search = true;

        state.on_file_changed();

        assert!(state.search_query.is_empty());
        assert!(state.search_matches.is_empty());
        assert_eq!(state.search_match_pos, None);
        assert!(!state.editing_search);
    }

    // ---- re-anchoring line comments across edits -------------------------

    fn texts(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    /// The same file, before and after an edit shifted its line numbers. The
    /// hunk contents are identical; only the hunk header moves, so every
    /// comment's exact `DiffLineLocation` goes stale while its context snippet
    /// still matches uniquely.
    fn shifted_diff_pair() -> (DiffFile, DiffFile) {
        let body = "\
 fn main() {
-    let x = 1;
+    let x = 2;
+    let y = 3;
 }
";
        let build = |header: &str| {
            let raw = format!(
                "diff --git a/a.rs b/a.rs\n\
                 index 1111111..2222222 100644\n\
                 --- a/a.rs\n\
                 +++ b/a.rs\n\
                 {header}\n{body}"
            );
            parse_unified_diff(&raw).unwrap().remove(0)
        };
        (build("@@ -1,3 +1,4 @@"), build("@@ -10,3 +20,4 @@"))
    }

    /// A single-line comment anchored to `idx`, with its context snippet captured
    /// from `file` exactly as `recapture_anchor_contexts` would.
    fn anchored_comment(file: &DiffFile, idx: usize, text: &str) -> LineComment {
        LineComment {
            location: file.addressable_lines()[idx],
            start: None,
            text: text.to_string(),
            draft: false,
            suggestion: None,
            severity: Severity::default(),
            anchor_context: CommentAnchorContext::capture(&file.addressable_line_texts(), idx),
            start_anchor_context: None,
            anchor_lost: false,
            resolved: false,
            carried: false,
        }
    }

    #[test]
    fn anchor_context_capture_bounds_at_file_edges() {
        let lines = texts(&["l0", "l1", "l2", "l3", "l4"]);

        let mid = CommentAnchorContext::capture(&lines, 2).unwrap();
        assert_eq!(mid.line, "l2");
        assert_eq!(mid.before, texts(&["l0", "l1"]));
        assert_eq!(mid.after, texts(&["l3", "l4"]));

        let first = CommentAnchorContext::capture(&lines, 0).unwrap();
        assert!(first.before.is_empty());
        assert_eq!(first.after, texts(&["l1", "l2"]));

        let last = CommentAnchorContext::capture(&lines, 4).unwrap();
        assert_eq!(last.before, texts(&["l2", "l3"]));
        assert!(last.after.is_empty());

        assert!(CommentAnchorContext::capture(&lines, 5).is_none());
    }

    #[test]
    fn anchor_context_best_match_disambiguates_repeated_lines_by_neighbours() {
        let ctx = CommentAnchorContext {
            line: "    x += 1;".to_string(),
            before: texts(&["fn a() {"]),
            after: texts(&["}"]),
        };
        // "    x += 1;" appears twice; only the second sits between fn a() and }.
        let haystack = texts(&[
            "fn b() {",
            "    x += 1;",
            "    more();",
            "fn a() {",
            "    x += 1;",
            "}",
        ]);
        assert_eq!(ctx.best_match(&haystack), Some(4));
    }

    #[test]
    fn anchor_context_best_match_tolerates_reindentation() {
        let ctx = CommentAnchorContext {
            line: "let x = 1;".to_string(),
            before: texts(&["fn a() {"]),
            after: Vec::new(),
        };
        let haystack = texts(&["fn a() {", "        let x = 1;"]);
        assert_eq!(ctx.best_match(&haystack), Some(1));
    }

    #[test]
    fn anchor_context_best_match_gives_up_when_ambiguous_or_absent() {
        // Two identical candidates with identical neighbours: a tie, so refuse to
        // guess rather than re-anchor onto the wrong one.
        let ambiguous = CommentAnchorContext {
            line: "x".to_string(),
            before: texts(&["a"]),
            after: texts(&["b"]),
        };
        let tie = texts(&["a", "x", "b", "a", "x", "b"]);
        assert_eq!(ambiguous.best_match(&tie), None);

        // The line is simply gone.
        assert_eq!(ambiguous.best_match(&texts(&["p", "q"])), None);

        // A blank anchor line carries no signal.
        let blank = CommentAnchorContext {
            line: "   ".to_string(),
            before: texts(&["a"]),
            after: texts(&["b"]),
        };
        assert_eq!(blank.best_match(&texts(&["a", "", "b"])), None);
    }

    #[test]
    fn reanchor_leaves_still_resolving_comments_untouched() {
        let (before, _) = shifted_diff_pair();
        let mut comments = vec![anchored_comment(&before, 2, "why 2?")];
        let original = comments[0].location;

        let (moved, lost) = reanchor_file_comments(&before, &mut comments);
        assert_eq!((moved, lost), (0, 0));
        assert_eq!(comments[0].location, original);
        assert!(!comments[0].anchor_lost);
    }

    #[test]
    fn reanchor_relocates_a_comment_whose_line_moved() {
        let (before, after) = shifted_diff_pair();
        let mut comments = vec![anchored_comment(&before, 2, "why 2?")];
        // The added `let x = 2;` was new_line 2; after the shift it is new_line 21.
        assert_eq!(comments[0].location.new_line, Some(2));

        let (moved, lost) = reanchor_file_comments(&after, &mut comments);
        assert_eq!((moved, lost), (1, 0));
        assert_eq!(comments[0].location, after.addressable_lines()[2]);
        assert_eq!(comments[0].location.new_line, Some(21));
        assert!(!comments[0].anchor_lost);
    }

    #[test]
    fn reanchor_does_not_trust_a_reused_line_number_with_different_text() {
        let before = local_apply_file("one\ntwo\nthree\n");
        let mut after = local_apply_file("one\ninserted\ntwo\nthree\n");
        after.hunks[0].new_lines = 4;
        after.hunks[0].lines = vec![
            DiffLine {
                kind: DiffLineKind::Context,
                text: " one".to_string(),
            },
            DiffLine {
                kind: DiffLineKind::Added,
                text: "+inserted".to_string(),
            },
            DiffLine {
                kind: DiffLineKind::Added,
                text: "+two".to_string(),
            },
            DiffLine {
                kind: DiffLineKind::Context,
                text: " three".to_string(),
            },
        ];
        let mut comment = line_comment(Some(2), None, "track two");
        comment.location = before.addressable_lines()[1];
        comment.anchor_context = CommentAnchorContext::capture(&before.addressable_line_texts(), 1);

        let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));

        assert_eq!((moved, lost), (1, 0));
        assert_eq!(comment.location.new_line, Some(3));
    }

    #[test]
    fn reanchor_flags_a_comment_whose_line_is_gone() {
        let (_, after) = shifted_diff_pair();
        let mut comment = LineComment {
            location: DiffLineLocation {
                old_line: None,
                new_line: Some(2),
            },
            start: None,
            text: "stale".to_string(),
            draft: false,
            suggestion: None,
            severity: Severity::default(),
            anchor_context: Some(CommentAnchorContext {
                line: "    let deleted = 9;".to_string(),
                before: Vec::new(),
                after: Vec::new(),
            }),
            start_anchor_context: None,
            anchor_lost: false,
            resolved: false,
            carried: false,
        };
        let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
        assert_eq!((moved, lost), (0, 1));
        assert!(comment.anchor_lost);

        // Already-lost comments are not re-counted on a subsequent refresh.
        let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
        assert_eq!((moved, lost), (0, 0));
        assert!(comment.anchor_lost);
    }

    #[test]
    fn reanchor_degrades_a_range_whose_start_cannot_be_found() {
        let (before, after) = shifted_diff_pair();
        let mut comment = anchored_comment(&before, 3, "this pair");
        // A range starting at idx 2, but with a start snippet that no longer
        // matches anything in the refreshed diff.
        comment.start = Some(before.addressable_lines()[2]);
        comment.start_anchor_context = Some(CommentAnchorContext {
            line: "    let vanished = 0;".to_string(),
            before: Vec::new(),
            after: Vec::new(),
        });

        let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
        assert_eq!((moved, lost), (1, 0));
        // End re-anchored; the span collapsed rather than inverting or guessing.
        assert_eq!(comment.location, after.addressable_lines()[3]);
        assert_eq!(comment.start, None);
        assert!(!comment.anchor_lost);
    }

    #[test]
    fn reanchor_relocates_both_ends_of_a_range() {
        let (before, after) = shifted_diff_pair();
        let mut comment = anchored_comment(&before, 3, "this pair");
        comment.start = Some(before.addressable_lines()[2]);
        comment.start_anchor_context =
            CommentAnchorContext::capture(&before.addressable_line_texts(), 2);

        let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
        assert_eq!((moved, lost), (1, 0));
        assert_eq!(comment.location, after.addressable_lines()[3]);
        assert_eq!(comment.start, Some(after.addressable_lines()[2]));
    }

    #[test]
    fn reanchor_cannot_relocate_a_comment_with_no_context_snippet() {
        // Comments loaded from a progress file written before re-anchoring existed.
        let (_, after) = shifted_diff_pair();
        let mut comment = LineComment {
            location: DiffLineLocation {
                old_line: None,
                new_line: Some(2),
            },
            start: None,
            text: "legacy".to_string(),
            draft: false,
            suggestion: None,
            severity: Severity::default(),
            anchor_context: None,
            start_anchor_context: None,
            anchor_lost: false,
            resolved: false,
            carried: false,
        };
        let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
        assert_eq!((moved, lost), (0, 1));
        assert!(comment.anchor_lost);
    }

    #[test]
    fn progress_files_written_before_reanchoring_still_load() {
        // A minimal old-shape progress file: none of the anchor fields, and none
        // of the other fields added since the first version of the format.
        let json = r#"{
            "line_comments": {
                "a.rs": [{ "location": { "old_line": null, "new_line": 2 }, "text": "old" }]
            }
        }"#;
        let progress: super::ReviewProgress = serde_json::from_str(json).unwrap();
        let comment = &progress.line_comments["a.rs"][0];

        assert_eq!(comment.text, "old");
        assert_eq!(comment.location.new_line, Some(2));
        assert_eq!(comment.anchor_context, None);
        assert_eq!(comment.start_anchor_context, None);
        assert!(!comment.anchor_lost);
    }

    #[test]
    fn lost_anchor_comment_labels_by_file_and_skips_inline_pr_posting() {
        let mut comment = line_comment(Some(42), None, "still worth a look");
        comment.anchor_lost = true;

        assert_eq!(
            comment_anchor_label("src/foo.rs", &comment),
            "src/foo.rs (anchor lost — possibly addressed)"
        );
        // …and that heading still resolves back to its file for agent replies.
        assert_eq!(
            anchor_file_path("src/foo.rs (anchor lost — possibly addressed)"),
            "src/foo.rs"
        );

        // A stale line number must never be posted inline on the PR.
        let sections = vec![("src/foo.rs".to_string(), vec![comment])];
        let (body, comments, _) = build_pr_review(&[], &[], &sections, "", &HashMap::new());
        assert!(comments.is_empty(), "lost anchor must not post inline");
        assert!(!body.contains("src/foo.rs:42"));
    }

    /// Addressable lines: 0 context(new 1), 1 removed(new None), 2 added(new 2),
    /// 3 added(new 3), 4 context(new 4).
    fn editor_line_file() -> DiffFile {
        parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,4 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
+fn gamma_alpha() {}
 fn delta() {}
",
        )
        .unwrap()
        .remove(0)
    }

    #[test]
    fn editor_target_line_uses_the_cursored_lines_current_side_number() {
        let file = editor_line_file();
        assert_eq!(editor_target_line(&file, Some(2)), Some(2));
        assert_eq!(editor_target_line(&file, Some(3)), Some(3));
        assert_eq!(editor_target_line(&file, Some(4)), Some(4));
    }

    #[test]
    fn editor_target_line_falls_back_to_the_nearest_surviving_line() {
        let file = editor_line_file();
        // Index 1 is a removal: it has no line in the file on disk, so the
        // editor should land on the surviving line just above it.
        assert_eq!(editor_target_line(&file, Some(1)), Some(1));
    }

    #[test]
    fn editor_target_line_without_a_cursor_lands_on_the_first_hunk() {
        let file = editor_line_file();
        assert_eq!(editor_target_line(&file, None), Some(1));
    }

    #[test]
    fn editor_target_line_is_none_when_there_is_nothing_addressable() {
        let mut file = editor_line_file();
        file.hunks.clear();
        assert_eq!(editor_target_line(&file, None), None);
        assert_eq!(editor_target_line(&file, Some(3)), None);
    }

    #[test]
    fn editor_invocation_uses_the_plus_flag_for_vi_style_editors() {
        let (program, args) = editor_invocation("nvim", "src/a.rs", Some(42)).unwrap();
        assert_eq!(program, "nvim");
        assert_eq!(args, vec!["+42".to_string(), "src/a.rs".to_string()]);
    }

    #[test]
    fn editor_invocation_keeps_flags_baked_into_the_env_var() {
        let (program, args) = editor_invocation("emacsclient -nw", "src/a.rs", Some(7)).unwrap();
        assert_eq!(program, "emacsclient");
        assert_eq!(args, vec!["-nw", "+7", "src/a.rs"]);
    }

    #[test]
    fn editor_invocation_makes_vscode_block_and_goto() {
        let (program, args) = editor_invocation("code", "src/a.rs", Some(42)).unwrap();
        assert_eq!(program, "code");
        assert_eq!(args, vec!["--wait", "--goto", "src/a.rs:42"]);

        // An explicit --wait must not be duplicated.
        let (_, args) = editor_invocation("code --wait", "src/a.rs", Some(42)).unwrap();
        assert_eq!(args, vec!["--wait", "--goto", "src/a.rs:42"]);
    }

    #[test]
    fn editor_invocation_uses_path_suffix_syntax_where_that_is_the_convention() {
        let (_, args) = editor_invocation("hx", "src/a.rs", Some(42)).unwrap();
        assert_eq!(args, vec!["src/a.rs:42"]);
    }

    #[test]
    fn editor_invocation_opens_an_unknown_editor_at_the_top_rather_than_guessing() {
        // A flag an editor doesn't understand would be read as a second
        // filename, silently opening an empty buffer called `+42`.
        let (program, args) = editor_invocation("my-editor", "src/a.rs", Some(42)).unwrap();
        assert_eq!(program, "my-editor");
        assert_eq!(args, vec!["src/a.rs"]);
    }

    #[test]
    fn editor_invocation_handles_an_absolute_path_and_no_line() {
        let (program, args) = editor_invocation("/usr/bin/vim", "src/a.rs", None).unwrap();
        assert_eq!(program, "/usr/bin/vim");
        assert_eq!(args, vec!["src/a.rs"]);

        assert!(editor_invocation("   ", "src/a.rs", Some(1)).is_none());
    }
}
