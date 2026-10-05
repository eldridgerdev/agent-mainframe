//! GUI presentation and lifecycle guards for the existing review AI engines.
use serde::Serialize;

use super::*;
use crate::app::App;
use crate::gui_plans::PrecallView;
use crate::project::AgentKind;

#[derive(Debug, Serialize)]
pub struct ReviewQuestionView {
    pub question: String,
    pub answer: Option<String>,
    pub error: Option<String>,
    pub focus: String,
    pub path: Option<String>,
    pub start: Option<DiffLineLocation>,
    pub end: Option<DiffLineLocation>,
    pub harness: AgentKind,
}

#[derive(Debug, Serialize)]
pub struct ReviewAiView {
    pub precall: Option<PrecallView>,
    pub running: bool,
    pub walkthrough_path: Option<String>,
    pub co_review_path: Option<String>,
    pub overview_running: bool,
    pub overview: Option<String>,
    pub question_running: bool,
    pub questions: Vec<ReviewQuestionView>,
    pub question_error: Option<String>,
    pub harnesses: Vec<AgentKind>,
    pub message: Option<String>,
}

pub(super) fn state(mode: &AppMode) -> GuiResult<&DiffViewerState> {
    match mode {
        AppMode::DiffViewer(s) => Ok(s),
        AppMode::PromptPrecall(p) => state(&p.prior_mode),
        _ => Err(GuiError::conflict("Final Review is no longer open")),
    }
}

fn running(s: &DiffViewerState) -> bool {
    s.walkthrough_child.is_some()
        || s.co_review_child.is_some()
        || s.co_review_bg.is_some()
        || s.changeset_overview_child.is_some()
        || s.questions.request.is_some()
}

fn harnesses(app: &App) -> GuiResult<Vec<AgentKind>> {
    let workdir = &state(&app.mode)?.workdir;
    let repo = app
        .store
        .projects
        .iter()
        .find(|p| p.features.iter().any(|f| &f.workdir == workdir))
        .map(|p| &p.repo)
        .unwrap_or(workdir);
    Ok(app.allowed_agents_for_repo(repo))
}

pub(super) fn view(app: &App) -> GuiResult<ReviewAiView> {
    let s = state(&app.mode)?;
    Ok(ReviewAiView {
        precall: match &app.mode {
            AppMode::PromptPrecall(p) => Some(PrecallView {
                title: p.prompt_id.spec().title.into(),
                harness: p.harness.display_name().into(),
                preview: p.preview.clone(),
                viewing: p.viewing,
            }),
            _ => None,
        },
        running: running(s),
        walkthrough_path: s.walkthrough_file.clone(),
        co_review_path: s.co_review_file.clone(),
        overview_running: s.changeset_overview_child.is_some(),
        overview: s.changeset_overview.clone(),
        question_running: s.questions.request.is_some(),
        questions: s
            .questions
            .turns
            .iter()
            .map(|t| ReviewQuestionView {
                question: t.question.clone(),
                answer: t.answer.clone(),
                error: t.error.clone(),
                focus: t.context.focus.clone(),
                path: t.context.path.clone(),
                start: t.context.anchor.as_ref().map(|a| a.start),
                end: t.context.anchor.as_ref().map(|a| a.end),
                harness: t.harness.clone(),
            })
            .collect(),
        question_error: s.questions.error.clone(),
        harnesses: harnesses(app)?,
        message: app.message.clone(),
    })
}

/// Dropping leased children terminates their process groups. The shared
/// batched worker has no cancellation handle: drop its receiver so any late
/// findings are discarded, matching the TUI's close behavior.
pub(super) fn cancel(app: &mut App) {
    app.precall_cancel();
    app.cancel_review_question();
    if let AppMode::DiffViewer(s) = &mut app.mode {
        s.walkthrough_child = None;
        s.walkthrough_file = None;
        s.co_review_child = None;
        s.co_review_bg = None;
        s.co_review_file = None;
        s.changeset_overview_child = None;
    }
}

pub(super) fn fresh(s: &DiffViewerState) -> GuiResult<()> {
    let mut current = crate::diff::load_snapshot(
        &s.workdir,
        s.override_base_ref.as_deref(),
        s.ignore_whitespace,
    )?;
    current
        .files
        .retain(|file| !crate::app::review::is_review_bookkeeping_path(&file.path));
    if current.base_commit != s.base_commit
        || current.files.len() != s.files.len()
        || !current.files.iter().zip(&s.files).all(|(a, b)| {
            a.path == b.path
                && a.old_path == b.old_path
                && a.patch == b.patch
                && a.status == b.status
                && a.old_content == b.old_content
                && a.new_content == b.new_content
        })
    {
        return Err(GuiError::conflict(
            "Review changes changed; refresh changes before running AI",
        ));
    }
    Ok(())
}

pub(super) fn start_guard(app: &App) -> GuiResult<()> {
    let s = state(&app.mode)?;
    if running(s) {
        return Err(GuiError::conflict(
            "Wait for the current AI request or cancel it first",
        ));
    }
    fresh(s)
}

fn claude_guard(app: &App) -> GuiResult<()> {
    start_guard(app)?;
    if !harnesses(app)?.contains(&AgentKind::Claude) {
        return Err(GuiError::conflict(
            "This review tool requires Claude; it is unavailable or disabled for this project",
        ));
    }
    Ok(())
}

pub(super) fn apply(app: &mut App, action: &ReviewAction) -> GuiResult<bool> {
    match action {
        ReviewAction::Walkthrough { .. } => {
            claude_guard(app)?;
            let s = state(&app.mode)?;
            let f = &s.files[s.selected_file];
            if f.is_binary {
                return Err(GuiError::conflict("Binary files have no walkthrough"));
            }
            app.message = None;
            app.generate_review_walkthrough();
        }
        ReviewAction::CoReview { .. } => {
            claude_guard(app)?;
            let s = state(&app.mode)?;
            let f = &s.files[s.selected_file];
            if f.is_binary || f.hunks.is_empty() {
                return Err(GuiError::conflict(
                    "Select a file with textual changes for AI co-review",
                ));
            }
            app.message = None;
            app.generate_co_review();
        }
        ReviewAction::Overview => {
            claude_guard(app)?;
            app.message = None;
            app.open_changeset_overview();
        }
        ReviewAction::Ask {
            question,
            harness,
            start,
            end,
            ..
        } => {
            start_guard(app)?;
            if question.trim().is_empty() {
                return Err(GuiError::conflict("Enter a question first"));
            }
            if !harnesses(app)?.contains(harness) {
                return Err(GuiError::conflict(
                    "This harness is unavailable or disabled for this project",
                ));
            }
            if start.is_some() != end.is_some() {
                return Err(GuiError::conflict(
                    "Select both ends of the question's range",
                ));
            }
            if let Some(end) = end {
                let s = state(&app.mode)?;
                let lines = s.files[s.selected_file].addressable_lines();
                let range = s.comment_anchor.unwrap()..=s.comment_cursor.unwrap();
                if range.into_iter().any(|i| {
                    if end.new_line.is_some() {
                        lines[i].new_line.is_none()
                    } else {
                        lines[i].old_line.is_none()
                    }
                }) {
                    return Err(GuiError::conflict(
                        "Select a question range entirely on one side of the diff",
                    ));
                }
            }
            app.open_review_questions();
            if let AppMode::DiffViewer(s) = &mut app.mode {
                if start.is_none() {
                    s.comment_cursor = None;
                    s.comment_anchor = None;
                }
                s.questions.editor = crate::editor::TextEditor::new(question.clone());
                s.questions.harness = harness.clone();
            }
            app.message = None;
            app.submit_review_question();
            if let AppMode::DiffViewer(s) = &app.mode
                && let Some(error) = &s.questions.error
            {
                return Err(GuiError::conflict(error.clone()));
            }
        }
        ReviewAction::AcceptDraft { start, end, path }
        | ReviewAction::DismissDraft { start, end, path } => {
            let s = state(&app.mode)?;
            if !s.line_comments.get(path).is_some_and(|comments| {
                let lines = s.files[s.selected_file].addressable_lines();
                let cursor = s.comment_cursor.unwrap();
                let overlapping = comments
                    .iter()
                    .filter(|c| {
                        c.draft
                            && c.covered_indices(&lines)
                                .is_some_and(|range| range.contains(&cursor))
                    })
                    .collect::<Vec<_>>();
                overlapping.len() == 1
                    && overlapping.iter().all(|c| {
                        c.draft
                            && !c.anchor_lost
                            && c.start.unwrap_or(c.location) == *start
                            && c.location == *end
                    })
            }) {
                return Err(GuiError::conflict("AI draft is no longer at this anchor"));
            }
            if matches!(action, ReviewAction::AcceptDraft { .. }) {
                app.diff_review_accept_draft_under_cursor();
            } else {
                app.diff_review_dismiss_draft_under_cursor();
            }
        }
        ReviewAction::PrecallConfirm => {
            let AppMode::PromptPrecall(pending) = &app.mode else {
                return Err(GuiError::conflict("There is no pending AI call to confirm"));
            };
            if !harnesses(app)?.contains(&pending.harness) {
                return Err(GuiError::conflict(
                    "This harness is now unavailable or disabled; cancel the AI call",
                ));
            }
            fresh(state(&app.mode)?)?;
            app.precall_confirm()?;
        }
        ReviewAction::PrecallCancel => app.precall_cancel(),
        ReviewAction::PrecallToggleView => app.precall_toggle_view(),
        ReviewAction::CancelAi => {
            cancel(app);
            app.message = Some("AI request cancelled; late results will be discarded".into());
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// A read poll may finish work but never launches a paid request. Revision
/// changes only for a completion/cancellation, preserving local GUI drafts.
/// Like the TUI, it never times a run out on its own: long repository-aware
/// runs are legitimate, the reviewer cancels explicitly, and a question's
/// own deadline lives in the shared question worker.
pub fn poll(gui: &mut GuiHandle, workflow_id: &str) -> GuiResult<ReviewView> {
    let context = gui
        .review_context
        .as_ref()
        .filter(|c| c.id == workflow_id)
        .ok_or_else(|| GuiError::conflict("Final Review is no longer open"))?;
    let target = context.target.clone();
    let progress = context.progress.clone();
    gui.refresh_snapshot()?;
    let app = gui.app_for_workflow();
    if matches!(app.mode, AppMode::PromptPrecall(_)) || !running(state(&app.mode)?) {
        return snapshot(gui);
    }
    let s = state(&app.mode)?;
    let valid = (|| -> GuiResult<()> {
        let (pi, fi) = app
            .store
            .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
            .ok_or_else(|| GuiError::not_found("Feature was deleted"))?;
        if app.store.projects[pi].features[fi].workdir != s.workdir {
            return Err(GuiError::conflict("Feature checkout changed"));
        }
        if progress_bytes(&s.workdir)? != progress {
            return Err(GuiError::conflict(
                "Saved review changed in another interface; reload the saved review",
            ));
        }
        fresh(s)
    })();
    if let Err(error) = valid {
        cancel(app);
        app.message = Some(format!("AI results discarded: {}", error.message));
    } else {
        let before = serde_json::to_vec(&view(app)?).map_err(anyhow::Error::from)?;
        let overview_running = state(&app.mode)?.changeset_overview_child.is_some();
        app.defer_review_progress_persist = true;
        let result = (|| -> GuiResult<()> {
            app.poll_review_walkthrough()?;
            app.poll_changeset_overview()?;
            app.poll_co_review()?;
            app.poll_review_questions();
            Ok(())
        })();
        app.defer_review_progress_persist = false;
        if let Err(error) = result {
            cancel(app);
            app.message = Some(format!("AI request failed: {}", error.message));
        }
        if overview_running
            && state(&app.mode)?.changeset_overview_child.is_none()
            && app.message.as_deref() == Some("Changeset overview running…")
        {
            app.message = None;
        }
        let after = serde_json::to_vec(&view(app)?).map_err(anyhow::Error::from)?;
        if before == after {
            return snapshot(gui);
        }
        super::save(gui);
    }
    let context = gui.review_context.as_mut().unwrap();
    context.revision += 1;
    snapshot(gui)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_diff::tests::fixture;
    use crate::traits::MockWorktreeOps;
    use std::time::{Duration, Instant};

    fn act(gui: &mut GuiHandle, view: &ReviewView, action: ReviewAction) -> ReviewView {
        super::super::act(gui, &view.workflow_id, view.revision, action)
            .unwrap()
            .unwrap()
    }

    fn opened() -> (tempfile::TempDir, GuiHandle, ReviewView) {
        let (dir, mut gui, target) = fixture();
        let mut worktree = MockWorktreeOps::new();
        worktree
            .expect_repo_root()
            .returning(|path| Ok(path.to_path_buf()));
        gui.app_for_workflow().worktree = Box::new(worktree);
        let view = begin(&mut gui, target).unwrap();
        (dir, gui, view)
    }

    fn ask(harness: AgentKind) -> ReviewAction {
        ReviewAction::Ask {
            path: "code.txt".into(),
            start: None,
            end: None,
            question: "Why this change?".into(),
            harness,
        }
    }

    fn pending_co_review(
        gui: &mut GuiHandle,
    ) -> std::sync::mpsc::Sender<Result<(String, usize), String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let AppMode::DiffViewer(s) = &mut gui.app_for_workflow().mode else {
            panic!("review");
        };
        s.co_review_file = Some("code.txt".into());
        s.co_review_bg = Some(rx);
        tx
    }

    fn drain(gui: &mut GuiHandle, view: &ReviewView) -> ReviewView {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let current = poll(gui, &view.workflow_id).unwrap();
            if !current.ai.running {
                return current;
            }
            assert!(Instant::now() < until, "review worker did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn answer(
        input: &crate::app::review_questions::test_support::RunInput,
    ) -> anyhow::Result<String> {
        assert!(input.discovery);
        assert!(input.prompt.contains("Why this change?"));
        assert!(input.prompt.contains("code.txt"));
        Ok("Repository-backed explanation".into())
    }

    #[test]
    fn ai_tools_require_a_precall_notice_and_cancel_restores_the_same_review() {
        for action in [
            ReviewAction::Walkthrough {
                path: "code.txt".into(),
            },
            ReviewAction::CoReview {
                path: "code.txt".into(),
            },
            ReviewAction::Overview,
            ask(AgentKind::Codex),
        ] {
            let (_dir, mut gui, view) = opened();
            let pending = act(&mut gui, &view, action);
            assert!(
                pending
                    .ai
                    .precall
                    .as_ref()
                    .is_some_and(|p| !p.preview.is_empty())
            );
            assert!(!pending.ai.running);
            assert!(gui.app_for_workflow().review_question_work.job.is_none());
            assert!(
                super::super::act(
                    &mut gui,
                    &pending.workflow_id,
                    pending.revision,
                    ReviewAction::Approve {
                        path: "code.txt".into()
                    }
                )
                .is_err()
            );
            let expanded = act(&mut gui, &pending, ReviewAction::PrecallToggleView);
            assert!(expanded.ai.precall.as_ref().unwrap().viewing);
            let cancelled = act(&mut gui, &expanded, ReviewAction::PrecallCancel);
            assert!(cancelled.ai.precall.is_none());
            assert_eq!(cancelled.workflow_id, view.workflow_id);
            assert_eq!(cancelled.files.len(), view.files.len());
            assert!(!cancelled.ai.running);
            assert_eq!(
                poll(&mut gui, &view.workflow_id).unwrap().revision,
                cancelled.revision
            );
        }
    }

    #[test]
    fn all_four_question_harnesses_use_the_shared_worker_and_reject_duplicate_requests() {
        for harness in AgentKind::ALL {
            let (_dir, mut gui, view) = opened();
            gui.app_for_workflow().review_question_work.runner = answer;
            let pending = act(&mut gui, &view, ask(harness.clone()));
            assert_eq!(
                pending.ai.precall.as_ref().unwrap().harness,
                harness.display_name()
            );
            let running = act(&mut gui, &pending, ReviewAction::PrecallConfirm);
            assert!(running.ai.question_running);
            assert!(
                super::super::act(
                    &mut gui,
                    &pending.workflow_id,
                    pending.revision,
                    ReviewAction::PrecallConfirm
                )
                .is_err()
            );
            assert!(
                super::super::act(
                    &mut gui,
                    &running.workflow_id,
                    running.revision,
                    ask(harness.clone())
                )
                .is_err()
            );
            let completed = drain(&mut gui, &running);
            assert_eq!(completed.ai.questions.len(), 1);
            assert_eq!(
                completed.ai.questions[0].answer.as_deref(),
                Some("Repository-backed explanation"),
                "question failed: {:?}; {:?}",
                completed.ai.question_error,
                completed.ai.message
            );
            assert!(completed.revision > running.revision);
            assert!(completed.save_error.is_none());
            let next = act(&mut gui, &completed, ask(harness.clone()));
            let next = act(&mut gui, &next, ReviewAction::PrecallConfirm);
            assert_eq!(drain(&mut gui, &next).ai.questions.len(), 2);
        }
    }

    #[test]
    fn saving_review_progress_while_a_question_runs_does_not_invalidate_its_repository_context() {
        fn wait_for_save(
            input: &crate::app::review_questions::test_support::RunInput,
        ) -> anyhow::Result<String> {
            let until = Instant::now() + Duration::from_secs(5);
            let ready = input.context.workdir.parent().unwrap().join("ready");
            while !ready.exists() {
                anyhow::ensure!(Instant::now() < until, "fixture save did not arrive");
                std::thread::sleep(Duration::from_millis(5));
            }
            answer(input)
        }
        let (dir, mut gui, view) = opened();
        gui.app_for_workflow().review_question_work.runner = wait_for_save;
        let pending = act(&mut gui, &view, ask(AgentKind::Claude));
        let running = act(&mut gui, &pending, ReviewAction::PrecallConfirm);
        let saved = act(
            &mut gui,
            &running,
            ReviewAction::General {
                text: "Saved while waiting".into(),
            },
        );
        std::fs::write(dir.path().join("ready"), "ready").unwrap();
        let completed = drain(&mut gui, &saved);
        assert_eq!(
            completed.ai.questions[0].answer.as_deref(),
            Some("Repository-backed explanation")
        );
        assert_eq!(completed.general_feedback, "Saved while waiting");
    }

    #[test]
    fn pause_with_an_unsaved_review_cancels_a_pending_notice_instead_of_panicking() {
        let (dir, mut gui, view) = opened();
        // Saving fails without the blocker showing up as a reviewed change.
        std::fs::write(dir.path().join("repo/.git/info/exclude"), ".claude\n").unwrap();
        let claude = dir.path().join("repo/.claude");
        std::fs::write(&claude, "parent is a file").unwrap();
        let view = act(
            &mut gui,
            &view,
            ReviewAction::General {
                text: "Keep me".into(),
            },
        );
        assert!(view.save_error.is_some());
        let pending = act(
            &mut gui,
            &view,
            ReviewAction::Walkthrough {
                path: "code.txt".into(),
            },
        );
        assert!(pending.ai.precall.is_some());
        let paused = act(&mut gui, &pending, ReviewAction::Pause);
        assert!(paused.ai.precall.is_none());
        assert!(paused.save_error.is_some());
        assert_eq!(paused.general_feedback, "Keep me");
        std::fs::remove_file(claude).unwrap();
        let view = act(&mut gui, &paused, ReviewAction::RetrySave);
        assert!(
            super::super::act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::Pause
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn changed_patch_is_refused_again_at_precall_confirmation() {
        let (dir, mut gui, view) = opened();
        let pending = act(&mut gui, &view, ask(AgentKind::Claude));
        std::fs::write(dir.path().join("repo/code.txt"), "changed externally\n").unwrap();
        assert!(
            super::super::act(
                &mut gui,
                &pending.workflow_id,
                pending.revision,
                ReviewAction::PrecallConfirm
            )
            .is_err()
        );
        assert!(gui.app_for_workflow().review_question_work.job.is_none());
        let restored = act(&mut gui, &pending, ReviewAction::PrecallCancel);
        assert!(restored.ai.precall.is_none());
        assert_eq!(
            gui.app_for_workflow()
                .review_questions()
                .unwrap()
                .editor
                .text(),
            "Why this change?"
        );
    }

    #[test]
    fn co_review_drafts_follow_their_file_and_need_acceptance_before_rejecting_it() {
        let (dir, mut gui, view) = opened();
        std::fs::write(dir.path().join("repo/another.txt"), "second file\n").unwrap();
        let view = act(&mut gui, &view, ReviewAction::Refresh);
        let tx = pending_co_review(&mut gui);
        let view = act(
            &mut gui,
            &view,
            ReviewAction::Select {
                path: "another.txt".into(),
            },
        );
        tx.send(Ok(("8|[blocker] Check the helper".into(), 0)))
            .unwrap();
        let completed = drain(&mut gui, &view);
        assert_eq!(completed.selected_path.as_deref(), Some("another.txt"));
        let file = completed
            .files
            .iter()
            .find(|f| f.diff.path == "code.txt")
            .unwrap();
        assert_eq!(file.verdict, "undecided");
        assert_eq!(file.line_comments.len(), 1);
        let comment = &file.line_comments[0];
        assert!(comment.draft);
        let start = comment.start;
        let end = comment.end;
        let accepted = act(
            &mut gui,
            &completed,
            ReviewAction::AcceptDraft {
                path: "code.txt".into(),
                start,
                end,
            },
        );
        let file = accepted
            .files
            .iter()
            .find(|f| f.diff.path == "code.txt")
            .unwrap();
        assert!(!file.line_comments[0].draft);
        assert_eq!(file.verdict, "rejected");
        assert!(
            super::super::act(
                &mut gui,
                &accepted.workflow_id,
                accepted.revision,
                ReviewAction::AcceptDraft {
                    path: "code.txt".into(),
                    start,
                    end
                }
            )
            .is_err()
        );
        super::super::act(
            &mut gui,
            &accepted.workflow_id,
            accepted.revision,
            ReviewAction::Pause,
        )
        .unwrap();
        let reopened = begin(&mut gui, accepted.target).unwrap();
        assert!(
            !reopened
                .files
                .iter()
                .find(|f| f.diff.path == "code.txt")
                .unwrap()
                .line_comments[0]
                .draft
        );
    }

    #[test]
    fn draft_dismissal_deletes_the_exact_thread_without_a_verdict() {
        let (_dir, mut gui, view) = opened();
        let tx = pending_co_review(&mut gui);
        tx.send(Ok(("8|Investigate this".into(), 0))).unwrap();
        let completed = drain(&mut gui, &view);
        let c = &completed.files[0].line_comments[0];
        let dismissed = act(
            &mut gui,
            &completed,
            ReviewAction::DismissDraft {
                path: "code.txt".into(),
                start: c.start,
                end: c.end,
            },
        );
        assert!(dismissed.files[0].line_comments.is_empty());
        assert_eq!(dismissed.files[0].verdict, "undecided");
    }

    #[test]
    fn overlapping_saved_drafts_cannot_accept_or_dismiss_a_neighbouring_thread() {
        let (_dir, mut gui, view) = opened();
        let tx = pending_co_review(&mut gui);
        tx.send(Ok(("8|First finding".into(), 0))).unwrap();
        let completed = drain(&mut gui, &view);
        let c = &completed.files[0].line_comments[0];
        let start = c.start;
        let end = c.end;
        if let AppMode::DiffViewer(s) = &mut gui.app_for_workflow().mode {
            let mut overlapping = s.line_comments["code.txt"][0].clone();
            overlapping.start = s.files[0]
                .addressable_lines()
                .into_iter()
                .find(|l| l.new_line == Some(7));
            s.line_comments
                .get_mut("code.txt")
                .unwrap()
                .insert(0, overlapping);
        }
        for action in [
            ReviewAction::AcceptDraft {
                path: "code.txt".into(),
                start,
                end,
            },
            ReviewAction::DismissDraft {
                path: "code.txt".into(),
                start,
                end,
            },
        ] {
            assert!(
                super::super::act(&mut gui, &completed.workflow_id, completed.revision, action)
                    .is_err()
            );
            let current = snapshot(&mut gui).unwrap();
            assert_eq!(current.files[0].line_comments.len(), 2);
            assert!(current.files[0].line_comments.iter().all(|c| c.draft));
        }
    }

    #[test]
    fn stale_completions_from_patch_progress_checkout_or_deleted_target_are_discarded() {
        for cause in 0..4 {
            let (dir, mut gui, view) = opened();
            let tx = pending_co_review(&mut gui);
            tx.send(Ok(("8|Obsolete finding".into(), 0))).unwrap();
            match cause {
                0 => std::fs::write(dir.path().join("repo/code.txt"), "changed\n").unwrap(),
                1 => {
                    std::fs::create_dir_all(dir.path().join("repo/.claude")).unwrap();
                    std::fs::write(
                        crate::app::review::review_progress_path(&dir.path().join("repo")),
                        "{}\n",
                    )
                    .unwrap();
                }
                _ => {
                    let app = gui.app_for_workflow();
                    if cause == 2 {
                        app.store.projects[0].features[0].workdir = dir.path().join("other");
                    } else {
                        app.store.projects[0].features.clear();
                    }
                    app.db.as_ref().unwrap().save_store(&app.store).unwrap();
                }
            }
            let completed = poll(&mut gui, &view.workflow_id).unwrap();
            assert!(!completed.ai.running);
            assert!(completed.ai.message.as_ref().unwrap().contains("discarded"));
            assert!(completed.files.iter().all(|f| f.line_comments.is_empty()));
            assert_eq!(completed.revision, view.revision + 1);
        }
    }

    #[test]
    fn cancellation_and_pause_drop_the_receiver_and_reopening_has_a_new_identity() {
        for pause in [false, true] {
            let (_dir, mut gui, view) = opened();
            let tx = pending_co_review(&mut gui);
            let next = super::super::act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                if pause {
                    ReviewAction::Pause
                } else {
                    ReviewAction::CancelAi
                },
            )
            .unwrap();
            assert!(tx.send(Ok(("8|Late finding".into(), 0))).is_err());
            if pause {
                assert!(next.is_none());
                let reopened = begin(&mut gui, view.target).unwrap();
                assert_ne!(reopened.workflow_id, view.workflow_id);
                assert!(poll(&mut gui, &view.workflow_id).is_err());
            } else {
                assert!(!next.unwrap().ai.running);
            }
        }
    }

    #[test]
    fn poll_reports_worker_failure_and_refresh_discards_cached_ai_notes() {
        let (_dir, mut gui, view) = opened();
        let tx = pending_co_review(&mut gui);
        tx.send(Err("fixture failure".into())).unwrap();
        let failed = drain(&mut gui, &view);
        assert!(
            failed
                .ai
                .message
                .as_ref()
                .unwrap()
                .contains("fixture failure")
        );
        if let AppMode::DiffViewer(s) = &mut gui.app_for_workflow().mode {
            s.generated_notes
                .insert("code.txt".into(), "Old walkthrough".into());
            s.changeset_overview = Some("Old overview".into());
        }
        let refreshed = act(&mut gui, &failed, ReviewAction::Refresh);
        assert!(refreshed.ai.overview.is_none());
        assert!(refreshed.files.iter().all(|f| f.walkthrough.is_none()));
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use crate::gui_diff::tests::fixture;
    use std::process::{Command, Stdio};

    #[test]
    fn finished_walkthrough_and_overview_outputs_are_cached_without_another_call() {
        for (shell, expected) in [
            ("printf 'Fixture explanation'", "Fixture explanation"),
            ("true", "empty"),
            (
                "printf 'fixture failed' >&2; exit 1",
                "unavailable: fixture failed",
            ),
        ] {
            let (_dir, mut gui, target) = fixture();
            let view = begin(&mut gui, target).unwrap();
            for overview in [false, true] {
                let mut child = Command::new("sh")
                    .args(["-c", shell])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child.wait().unwrap();
                let AppMode::DiffViewer(s) = &mut gui.app_for_workflow().mode else {
                    panic!("review");
                };
                if overview {
                    s.changeset_overview_child = Some(crate::headless::LeasedChild::new(child));
                } else {
                    s.walkthrough_child = Some(crate::headless::LeasedChild::new(child));
                    s.walkthrough_file = Some("code.txt".into());
                }
                if overview {
                    gui.app_for_workflow().message = Some("Changeset overview running…".into());
                }
                let completed = poll(&mut gui, &view.workflow_id).unwrap();
                assert_ne!(
                    completed.ai.message.as_deref(),
                    Some("Changeset overview running…")
                );
                let text = if overview {
                    completed.ai.overview.as_deref()
                } else {
                    completed.files[0].walkthrough.as_deref()
                };
                assert!(text.unwrap().contains(expected));
                let action = if overview {
                    ReviewAction::Overview
                } else {
                    ReviewAction::Walkthrough {
                        path: "code.txt".into(),
                    }
                };
                let cached =
                    super::super::act(&mut gui, &completed.workflow_id, completed.revision, action)
                        .unwrap()
                        .unwrap();
                assert!(cached.ai.precall.is_none());
                assert!(!cached.ai.running);
            }
        }
    }
}
