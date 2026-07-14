use std::io::Write;
use std::path::{Path, PathBuf};

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

/// Cap on how much of a check command's combined stdout/stderr is kept, so a
/// noisy build/test failure can't blow up the feedback file or the agent
/// prompt built from it.
const CHECK_OUTPUT_MAX_CHARS: usize = 4000;

fn truncate_check_output(output: &str) -> String {
    if output.chars().count() <= CHECK_OUTPUT_MAX_CHARS {
        output.to_string()
    } else {
        let truncated: String = output.chars().take(CHECK_OUTPUT_MAX_CHARS).collect();
        format!("{truncated}\n… (truncated)")
    }
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
    /// file path -> the kept (non-draft) line comments the round finished with,
    /// each tagged `carried` and keeping its `resolved` flag. Restored as live
    /// threads when the next round opens, so a conversation survives the agent
    /// addressing it — the cross-round half of the re-anchor machinery.
    /// Defaulted so snapshots written before this field load unchanged.
    #[serde(default)]
    threads: std::collections::HashMap<String, Vec<LineComment>>,
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
            || !state.general_feedback.is_empty()
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
        // `line_comments` alone can't gate this: `restore_review_progress` just
        // seeded it with threads carried in from the last round, which is
        // exactly the state a fresh re-review opens with — so only comments
        // authored *this* session count as work in progress.
        let pristine = state.decisions.is_empty()
            && state.has_only_carried_comments()
            && state.general_feedback.is_empty()
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
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
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
        state.changeset_overview_open = true;
        if state.changeset_overview.is_none() && state.changeset_overview_child.is_none() {
            self.generate_changeset_overview();
        }
    }

    /// Spawn (or re-spawn) a headless whole-changeset overview pass. Unlike
    /// `open_changeset_overview` this always starts a fresh generation when
    /// none is already in flight — the explicit "regenerate" action once the
    /// modal is open.
    pub fn generate_changeset_overview(&mut self) {
        let (workdir, prompt) = {
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
                build_changeset_overview_prompt(&state.files),
            )
        };

        match crate::claude::ClaudeLauncher::spawn_headless(&workdir, &prompt) {
            Ok(child) => {
                self.message = Some("Changeset overview running…".to_string());
                if let AppMode::DiffViewer(state) = &mut self.mode {
                    state.changeset_overview_child = Some(child);
                }
            }
            Err(err) => {
                self.message = Some(format!("Changeset overview unavailable: {err}"));
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
            state.feedback_editor = crate::editor::TextEditor::new(text);
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
            let severity = state.comment_severity;
            if let Some(file) = state.files.get(state.selected_file) {
                state.decisions.insert(
                    file.path.clone(),
                    ReviewDecision::Reject { feedback, severity },
                );
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

    /// Finish the review. If the project has a `final_review_check_command`
    /// configured (a build/test gate), spawn it in the background and return
    /// immediately — `poll_final_review_check` picks up the result once the
    /// process exits and actually completes the review. Otherwise (the
    /// default: no command configured) completes immediately, unchanged from
    /// before this gate existed.
    pub fn finish_final_review(&mut self) -> Result<()> {
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

    /// Write `.claude/final-review-feedback.md` for any rejected files and
    /// return to the feature view with a summary message, folding in the
    /// optional build/test-gate `check` outcome.
    fn complete_final_review(&mut self, check: Option<CheckOutcome>) -> Result<()> {
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
            self.save_review_snapshot(&workdir, &files, &decisions, &line_comments);
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

        if !check_failed
            && rejected.is_empty()
            && general_feedback.is_empty()
            && line_comment_sections.is_empty()
        {
            self.message = Some(if total == 0 {
                "Final review: no changes against the base branch".to_string()
            } else {
                let check_note = match &check {
                    Some(c) => format!(" (check `{}` passed)", c.command),
                    None => String::new(),
                };
                format!(
                    "Final review complete: all {approved} reviewed file(s) approved{}{check_note}",
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

            if let Some(c) = &check {
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
            let check_note = match &check {
                Some(c) if c.passed => format!(", check `{}` passed", c.command),
                Some(c) => format!(", check `{}` FAILED", c.command),
                None => String::new(),
            };
            let summary = format!(
                "Final review: {approved} approved, {} need work, {skipped} skipped\
                 {comment_note}{check_note} — feedback saved to .claude/final-review-feedback.md",
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
        line_comment_sections: &[(String, Vec<LineComment>)],
        general_feedback: &str,
    ) -> String {
        use crate::github::{GhCli, PrResolution};

        let (body, comments, file_comments) =
            build_pr_review(rejected, line_comment_sections, general_feedback);
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
        let event = resolve_review_event(workdir, pr.number, rejected, line_comment_sections);
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

/// Indices into `file.addressable_lines()` whose text contains `query`
/// (case-insensitive substring), ascending. Empty for a blank query. Matches
/// against `addressable_line_texts()` (diff prefix stripped) so a query hits the
/// same content regardless of whether the line was added, removed or context.
fn compute_search_matches(file: &crate::diff::DiffFile, query: &str) -> Vec<usize> {
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
        let end_ok = locs.contains(&comment.location);
        let start_ok = comment.start.is_none_or(|start| locs.contains(&start));
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
    line_comment_sections: &[(String, Vec<LineComment>)],
) -> &'static str {
    let escalated = severity_review_event(rejected, line_comment_sections);
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
    line_comment_sections: &[(String, Vec<LineComment>)],
) -> &'static str {
    let has_blocker = rejected.iter().any(|(_, _, s)| s.is_blocker())
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

/// Assemble a GitHub PR review from a finished final review. Line comments
/// become inline review comments — anchored to the current file line
/// (`RIGHT`) or, for a deletion-only line, the base file line (`LEFT`).
/// Whole-file rejections have no single line to anchor to either, but *do*
/// have a file — they become `subject_type: file` comments instead of being
/// dumped into the summary body (GitHub's batch review endpoint can't carry
/// those, so the caller posts them as separate `create_file_comment` calls).
/// The summary body carries only the general feedback. Returns `(body,
/// comments, file_comments)`.
fn build_pr_review(
    rejected: &[(String, String, Severity)],
    line_comment_sections: &[(String, Vec<LineComment>)],
    general_feedback: &str,
) -> (
    String,
    Vec<crate::github::PrReviewComment>,
    Vec<crate::github::PrFileComment>,
) {
    let mut comments = Vec::new();
    for (path, file_comments) in line_comment_sections {
        for comment in file_comments {
            // A comment we couldn't re-anchor holds a stale line number; posting
            // it inline would pin it to the wrong line. Omit it — the local
            // feedback file still carries it, flagged "anchor lost".
            if comment.anchor_lost {
                continue;
            }
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
    let file_comments = rejected
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

/// Build the prompt for an on-demand overview / risk summary of the **whole**
/// changeset (reviewer-triggered via `O`, never automatic). Bounded on two
/// axes so a large changeset can't produce an unbounded headless request: the
/// file list is capped at `MAX_FILES`, and each included file's patch gets a
/// much smaller budget than the single-file walkthrough's, since this prompt
/// aggregates many files at once.
fn build_changeset_overview_prompt(files: &[crate::diff::DiffFile]) -> String {
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

    format!(
        "You are helping a reviewer triage a full changeset before a final review. \
         Given the per-file diffs below, write a short markdown overview: a couple \
         of sentences on what the change does overall, then a bulleted list of the \
         areas it touches, then a short \"Risk factors\" list flagging anything that \
         deserves extra attention (large surface area, files with no obvious test \
         coverage, cross-cutting or structural changes, anything that looks \
         unusually risky). Be concise — this is a triage aid, not a full review.\n\n\
         {body}"
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
        CHECK_OUTPUT_MAX_CHARS, anchor_file_path, build_pr_review, build_walkthrough_prompt,
        comment_anchor_label, compose_feedback_log, compute_search_matches, parse_agent_responses,
        parse_co_review_output, parse_review_notes, reanchor_file_comments, severity_review_event,
        truncate_check_output,
    };
    use crate::app::state::DiffViewerState;
    use crate::app::{CommentAnchorContext, LineComment, Severity};
    use crate::diff::{DiffFile, DiffLineLocation, parse_unified_diff};

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
        let (body, comments, file_comments) =
            build_pr_review(&rejected, &line_comments, "overall LGTM-ish");

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
        let (body, comments, file_comments) = build_pr_review(&[], &line_comments, "");
        assert!(body.is_empty());
        assert_eq!(comments.len(), 1);
        assert!(file_comments.is_empty());
    }

    #[test]
    fn pr_review_emits_start_line_for_a_ranged_comment() {
        let line_comments = vec![(
            "src/c.rs".to_string(),
            vec![ranged_comment(10, 14, "this whole block")],
        )];
        let (_, comments, _) = build_pr_review(&[], &line_comments, "");
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
        let (_, comments, _) = build_pr_review(&[], &line_comments, "");
        assert_eq!(comments[0].start_line, None);
        assert_eq!(comments[0].start_side, None);
    }

    #[test]
    fn pr_review_appends_suggestion_block_to_comment_body() {
        let mut comment = line_comment(Some(5), None, "use a guard");
        comment.suggestion = Some("let x = y?;".to_string());
        let line_comments = vec![("src/c.rs".to_string(), vec![comment])];
        let (_, comments, _) = build_pr_review(&[], &line_comments, "");
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
        let (_, comments, _) = build_pr_review(&[], &line_comments, "");
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
        assert_eq!(severity_review_event(&rejected, &[]), "REQUEST_CHANGES");
        // A blocker line comment escalates even with no rejection.
        let sections = vec![("a.rs".to_string(), vec![blocker_comment(3, "must fix")])];
        assert_eq!(severity_review_event(&[], &sections), "REQUEST_CHANGES");
    }

    #[test]
    fn severity_event_approves_with_no_rejections_else_comments() {
        // No rejection, only non-blocking notes → an approving review.
        let sections = vec![("a.rs".to_string(), vec![line_comment(Some(3), None, "nit")])];
        assert_eq!(severity_review_event(&[], &sections), "APPROVE");
        assert_eq!(severity_review_event(&[], &[]), "APPROVE");
        // A non-blocking rejection is a plain comment review.
        let rejected = vec![("a.rs".to_string(), "meh".to_string(), Severity::Suggestion)];
        assert_eq!(severity_review_event(&rejected, &[]), "COMMENT");
    }

    #[test]
    fn feedback_file_comment_tags_whole_file_rejection_with_severity() {
        // A whole-file rejection's file-level comment carries its severity tag.
        let rejected = vec![("a.rs".to_string(), "fix".to_string(), Severity::Blocker)];
        let (_, _, file_comments) = build_pr_review(&rejected, &[], "");
        assert_eq!(file_comments.len(), 1);
        assert_eq!(file_comments[0].path, "a.rs");
        assert_eq!(file_comments[0].body, "**[blocker]** fix");
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
        let (body, comments, _) = build_pr_review(&[], &sections, "");
        assert!(comments.is_empty(), "lost anchor must not post inline");
        assert!(!body.contains("src/foo.rs:42"));
    }
}
