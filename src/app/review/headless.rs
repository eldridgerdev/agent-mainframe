use super::comments::{comment_anchor_label, local_suggestion_summary};
use super::preparation::{
    FINAL_REVIEW_SESSION_LABEL, MAX_LIVE_ROUNDS, REVIEW_FEEDBACK_PROMPT, compose_feedback_log,
    review_round_preamble, split_overflow_rounds,
};
use crate::app::pr_review::FixTarget;
use crate::app::{
    App, AppMode, AwaitingReviewFix, FileComment, HashMap, LineComment, ReviewAction,
    ReviewDecision, ReviewHarnessPickState, Severity, StartIntent, ViewState,
};
use crate::extension::merge_project_extension_config;
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;

/// Outcome of the project's optional `final_review_check_command` (a
/// build/test gate), run in the background when finishing a review. `None`
/// throughout `complete_final_review` whenever no command is configured.
pub(super) struct CheckOutcome {
    pub(super) command: String,
    pub(super) passed: bool,
    pub(super) output: String,
}

/// Cap on how much of a check command's combined stdout/stderr is kept, so a
/// noisy build/test failure can't blow up the feedback file or the agent
/// prompt built from it.
pub(super) const CHECK_OUTPUT_MAX_CHARS: usize = 4000;

pub(super) fn truncate_check_output(output: &str) -> String {
    if output.chars().count() <= CHECK_OUTPUT_MAX_CHARS {
        output.to_string()
    } else {
        let truncated: String = output.chars().take(CHECK_OUTPUT_MAX_CHARS).collect();
        format!("{truncated}\n… (truncated)")
    }
}

impl App {
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
        let (workdir, file) = {
            let AppMode::DiffViewer(state) = &self.mode else {
                return;
            };
            if !state.review || state.co_review_child.is_some() || state.co_review_bg.is_some() {
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
            (state.workdir.clone(), file.clone())
        };
        let path = file.path.clone();

        let repo = crate::worktree::WorktreeManager::repo_root(&workdir)
            .unwrap_or_else(|_| workdir.clone());
        let prompt = self.resolve_headless_prompt(
            crate::prompts::PromptId::ReviewCoReview,
            &crate::project::AgentKind::Claude,
            &repo,
            &workdir,
            &co_review_context(&file),
        );
        if !self.precall_gate(
            crate::app::precall::PrecallAction::ReviewCoReview,
            &crate::project::AgentKind::Claude,
            &prompt,
        ) {
            return;
        }
        let model = self.config.review_model_for(ReviewAction::CoReview);

        // Oversized file: review it hunk-slice by hunk-slice on a worker thread
        // rather than sending the single truncated prompt. `review_prompt_budget`
        // of `0` disables pre-send splitting entirely (the documented opt-out,
        // matching the `W` path) — the file then falls through to the single
        // pass, where `co_review_context` bounds the body with a visible
        // "diff truncated" marker.
        let full_body_len = co_review_annotated_body(&file.hunks).len();
        let co_review_budget = self.review_prompt_budget(&repo, &crate::project::AgentKind::Claude);
        if co_review_budget != 0
            && (full_body_len > CO_REVIEW_MAX_BODY
                || crate::headless::will_overflow_with_budget(&prompt, co_review_budget))
        {
            let (template, _) = self.resolve_headless_template(
                crate::prompts::PromptId::ReviewCoReview,
                &crate::project::AgentKind::Claude,
                &repo,
                &workdir,
            );
            let (tx, rx) = std::sync::mpsc::channel();
            let (thread_workdir, thread_model) = (workdir.clone(), model.clone());
            std::thread::spawn(move || {
                let _ = tx.send(run_batched_co_review(
                    &thread_workdir,
                    &file,
                    &template,
                    thread_model.as_deref(),
                ));
            });
            if let AppMode::DiffViewer(state) = &mut self.mode {
                state.co_review_bg = Some(rx);
                state.co_review_file = Some(path.clone());
            }
            self.message = Some(format!("AI co-review (batched) running on {path}…"));
            return;
        }

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
    /// `poll_review_walkthrough`. Handles both the single `spawn_headless` pass
    /// and the batched worker-thread pass for an oversized file.
    pub fn poll_co_review(&mut self) -> Result<()> {
        // Batched (worker-thread) pass.
        let bg_msg = match &self.mode {
            AppMode::DiffViewer(state) => state
                .co_review_bg
                .as_ref()
                .map(std::sync::mpsc::Receiver::try_recv),
            _ => None,
        };
        if let Some(recv) = bg_msg {
            match recv {
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if let AppMode::DiffViewer(state) = &mut self.mode {
                        state.co_review_bg = None;
                        state.co_review_file = None;
                    }
                    self.message = Some("AI co-review failed: worker exited".to_string());
                    return Ok(());
                }
                Ok(result) => {
                    let path = match &mut self.mode {
                        AppMode::DiffViewer(state) => {
                            state.co_review_bg = None;
                            state.co_review_file.take()
                        }
                        _ => None,
                    };
                    let Some(path) = path else { return Ok(()) };
                    match result {
                        Ok((text, unreviewed)) => {
                            self.apply_co_review_text(&path, &text);
                            if unreviewed > 0 {
                                let tail =
                                    format!("{unreviewed} hunk group(s) could not be reviewed");
                                self.message = Some(match self.message.take() {
                                    Some(base) if !base.is_empty() => format!("{base} · {tail}"),
                                    _ => format!("AI co-review: {tail}"),
                                });
                            }
                        }
                        Err(msg) => self.message = Some(format!("AI co-review failed: {msg}")),
                    }
                    return Ok(());
                }
            }
        }

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
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        self.apply_co_review_text(&path, &text);
        Ok(())
    }

    /// Parse `<line>|<comment>` co-review output into non-overlapping draft
    /// line comments for `path`, sort them, and set the status message. Shared
    /// by the single and batched co-review polls.
    fn apply_co_review_text(&mut self, path: &str, text: &str) {
        let added = if let AppMode::DiffViewer(state) = &mut self.mode {
            let Some(file) = state.files.iter().find(|f| f.path == path) else {
                return;
            };
            let locs = file.addressable_lines();
            let drafts = parse_co_review_output(text, &locs);
            let existing = state.line_comments.entry(path.to_string()).or_default();
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
            return;
        };

        self.message = Some(if added == 0 {
            format!("AI co-review: no new findings for {path}")
        } else {
            format!("AI co-review added {added} draft comment(s) — a accept · d dismiss")
        });
        self.persist_review_progress();
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
    pub(super) fn persist_final_review_round(
        &mut self,
        workdir: &Path,
        round: &str,
    ) -> std::io::Result<()> {
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
    pub(super) fn complete_final_review(&mut self, check: Option<CheckOutcome>) -> Result<()> {
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
    pub(super) fn dispatch_review_feedback(
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
    pub(super) fn paste_review_prompt(&mut self, session: &str, window: &str) -> String {
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
    pub(super) fn post_final_review_to_pr(
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

/// Map a diff-line location to a GitHub `(line, side)` pair: the current-file
/// line (`RIGHT`) when present, else the base-file line (`LEFT`). `None` for an
/// unanchored location.
pub(super) fn pr_line_side(loc: &crate::diff::DiffLineLocation) -> Option<(usize, &'static str)> {
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
pub(super) fn resolve_review_event(
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
pub(super) fn severity_review_event(
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
pub(super) fn pr_postable_lines(
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
pub(super) fn build_pr_review(
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

/// The `{{file_path}}` / `{{patch}}` context for `PromptId::ReviewWalkthrough`.
/// Large patches are truncated here to keep the headless request bounded.
pub(super) fn walkthrough_context(file: &crate::diff::DiffFile) -> crate::prompts::PromptContext {
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
/// Rough per-slice cap on the annotated co-review body. Beyond this the file is
/// reviewed hunk-slice by hunk-slice ([`App::spawn_batched_co_review`]) instead
/// of being silently truncated — unless `review_prompt_budget_tokens` is `0`,
/// which opts out of all pre-send splitting and lets the single pass truncate
/// the body with a visible marker.
const CO_REVIEW_MAX_BODY: usize = 8000;

/// The line-numbered co-review body for a run of hunks. No length cap — callers
/// that need one apply it (the single-shot [`co_review_context`]) or split the
/// hunks first (the batched path).
fn co_review_annotated_body(hunks: &[crate::diff::DiffHunk]) -> String {
    use crate::diff::DiffLineKind;
    let mut body = String::new();
    for hunk in hunks {
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
    body
}

/// `{{token}}` context for a `review.co_review` pass over `hunks` of `path`.
fn co_review_slice_context(
    path: &str,
    hunks: &[crate::diff::DiffHunk],
) -> crate::prompts::PromptContext {
    crate::prompts::PromptContext::new()
        .with("file_path", path.to_string())
        .with("annotated_body", co_review_annotated_body(hunks))
}

pub(super) fn co_review_context(file: &crate::diff::DiffFile) -> crate::prompts::PromptContext {
    let mut body = co_review_annotated_body(&file.hunks);
    if body.len() > CO_REVIEW_MAX_BODY {
        body.truncate(CO_REVIEW_MAX_BODY);
        body.push_str("\n… (diff truncated)");
    }
    crate::prompts::PromptContext::new()
        .with("file_path", file.path.clone())
        .with("annotated_body", body)
}

/// Worker-thread body of a batched co-review: split `file` into hunk slices
/// each under [`CO_REVIEW_MAX_BODY`], run `review.co_review` (`template`) over
/// each, and concatenate the `<line>|<comment>` outputs. `Err` only when every
/// slice failed; a partial failure drops the failed slice and keeps going.
fn run_batched_co_review(
    workdir: &Path,
    file: &crate::diff::DiffFile,
    template: &str,
    model: Option<&str>,
) -> std::result::Result<(String, usize), String> {
    let Some(section) = crate::diff_split::SplitDiff::parse(&file.patch)
        .files
        .into_iter()
        .next()
    else {
        return Err("could not parse the file diff for batched co-review".to_string());
    };
    // ~CO_REVIEW_MAX_BODY bytes ≈ that many / 4 tokens per slice.
    let split = crate::diff_split::split_file_by_hunk(&section, CO_REVIEW_MAX_BODY / 4);

    let mut combined = String::new();
    let mut attempted = 0usize;
    let mut failed = 0usize;
    let mut last_err = String::new();
    for sub in &split.subunits {
        let hunks: &[crate::diff::DiffHunk] = match sub.hunk_span {
            Some((a, b)) => file.hunks.get(a..=b).unwrap_or(&[]),
            None => &[],
        };
        if hunks.is_empty() {
            continue;
        }
        attempted += 1;
        let prompt =
            crate::prompts::render_template(template, &co_review_slice_context(&file.path, hunks));
        match crate::headless::HeadlessRunner::run(
            &crate::project::AgentKind::Claude,
            workdir,
            &prompt,
            model,
            false,
        ) {
            Ok(text) => {
                let text = text.trim();
                if !text.is_empty() {
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str(text);
                }
            }
            Err(err) => {
                failed += 1;
                last_err = err.to_string();
            }
        }
    }

    if attempted > 0 && failed == attempted {
        return Err(format!("all {attempted} slice(s) failed: {last_err}"));
    }
    Ok((combined, failed))
}

/// The `{{files_block}}` context for `PromptId::ReviewChangesetOverview`.
/// Bounded on two axes so a large changeset can't produce an unbounded
/// headless request: the file list is capped at `MAX_FILES`, and each
/// included file's patch gets a much smaller budget than the single-file
/// walkthrough's, since this prompt aggregates many files at once.
pub(super) fn changeset_overview_context(
    files: &[crate::diff::DiffFile],
) -> crate::prompts::PromptContext {
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
pub(super) fn parse_co_review_output(
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
