//! Explicitly confirmed review completion and feedback handoff over the TUI's
//! finish engine: the saved suggestion opt-in, the configured check (rerun,
//! like the TUI, rather than trusting an earlier transient result), and
//! `record_final_review_round`. Only the agent handoff differs: it targets the
//! reviewed feature's first agent session, as the TUI's default destination
//! does, and is delivered only when the reviewer chose it.
use serde::Serialize;

use super::*;
use crate::app::pr_review::{FixTarget, fix_session_index};
use crate::app::review::{
    CheckOutcome, FINAL_REVIEW_SESSION_LABEL, REVIEW_FEEDBACK_PROMPT, ReviewCheckRun,
};
use crate::gui_contract::SessionTarget;

/// The agent session an actionable round's prompt goes to.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewHandoffTarget {
    pub session_id: String,
    pub label: String,
    /// The session (or its feature) is not running, so a submitted prompt
    /// cannot be delivered; it is kept as an unsent draft instead.
    pub stopped: bool,
}

/// What completing would record and do, for the explicit confirmation.
#[derive(Debug, Serialize)]
pub struct ReviewFinishView {
    pub approved: usize,
    pub needs_work: usize,
    pub skipped: usize,
    pub file_comments: usize,
    pub line_comments: usize,
    pub general_feedback: bool,
    /// Open suggestions the review's saved apply-on-finish opt-in will write
    /// to source before the round is recorded (0 without the opt-in).
    pub apply_suggestions: usize,
    pub post_to_pr: bool,
    /// The TUI setting deciding whether a handed-off prompt is submitted
    /// (Enter) or left unsent for the reviewer to edit.
    pub submit_prompt: bool,
    pub handoff: Option<ReviewHandoffTarget>,
    /// The configured check is running as part of a confirmed completion.
    pub completing: bool,
}

/// The result of a completed review, taken once by the interface.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewCompletion {
    pub workflow_id: String,
    pub message: String,
    pub handoff: Option<ReviewHandoff>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewHandoff {
    pub target: SessionTarget,
    /// Unsent composer text. `None` when the prompt was submitted.
    pub draft_prompt: Option<String>,
}

/// A confirmed completion waiting for its configured check.
pub(super) struct PendingCompletion {
    deliver: bool,
    handoff_session: Option<String>,
}

struct Handoff {
    view: ReviewHandoffTarget,
    target: SessionTarget,
    tmux_session: String,
    window: String,
}

/// Resolve the TUI's default destination by stable id against the current
/// store: the reviewed feature's first agent session.
fn handoff(gui: &mut GuiHandle) -> Option<Handoff> {
    let target = gui.review_context.as_ref()?.target.clone();
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)?;
    let feature = &app.store.projects[pi].features[fi];
    let si = fix_session_index(feature, FixTarget::ExistingLive, FINAL_REVIEW_SESSION_LABEL)?;
    let session = &feature.sessions[si];
    Some(Handoff {
        view: ReviewHandoffTarget {
            session_id: session.id.clone(),
            label: session.label.clone(),
            stopped: session.stopped || feature.status == crate::project::ProjectStatus::Stopped,
        },
        target: SessionTarget {
            project_id: target.project_id,
            feature_id: target.feature_id,
            session_id: session.id.clone(),
        },
        tmux_session: feature.tmux_session.clone(),
        window: session.tmux_window.clone(),
    })
}

pub(super) fn preview(gui: &mut GuiHandle) -> Option<ReviewFinishView> {
    let completing = gui.review_context.as_ref()?.completion.is_some();
    let handoff = handoff(gui).map(|h| h.view);
    let app = gui.app_for_workflow();
    let s = ai::state(&app.mode).ok()?;
    if !s.summary_open {
        return None;
    }
    let verdict = |approve: bool| {
        s.files
            .iter()
            .filter(|f| match s.decisions.get(&f.path) {
                Some(ReviewDecision::Approve) => approve,
                Some(ReviewDecision::Reject { .. }) => !approve,
                None => false,
            })
            .count()
    };
    let (approved, needs_work) = (verdict(true), verdict(false));
    Some(ReviewFinishView {
        approved,
        needs_work,
        skipped: s.files.len() - approved - needs_work,
        file_comments: s
            .files
            .iter()
            .filter(|f| {
                s.file_comments
                    .get(&f.path)
                    .is_some_and(|c| c.is_open_thread())
            })
            .count(),
        line_comments: s
            .files
            .iter()
            .filter_map(|f| s.line_comments.get(&f.path))
            .flatten()
            .filter(|c| c.is_open_thread())
            .count(),
        general_feedback: !s.general_feedback.trim().is_empty(),
        apply_suggestions: if s.apply_suggestions_on_finish {
            s.pending_suggestion_count()
        } else {
            0
        },
        post_to_pr: app.config.final_review_post_to_pr,
        submit_prompt: app.config.final_review_submit_prompt,
        handoff,
        completing,
    })
}

pub(super) enum Started {
    /// The configured check is running; completion follows on its result.
    Checking,
    /// Nothing was recorded; the open review explains why.
    Stopped,
    Completed,
}

/// Start a confirmed completion. Every expectation the confirmation showed
/// (check command, suggestion batch, handoff session) is rechecked so a
/// changed project, review or target is refused before anything is written.
pub(super) fn start(
    gui: &mut GuiHandle,
    check_command: Option<String>,
    apply_suggestions: usize,
    handoff_session: Option<String>,
    deliver: bool,
) -> GuiResult<Started> {
    let context = gui.review_context.as_ref().unwrap();
    if context.save_error.is_some() {
        return Err(GuiError::conflict(
            "Save review progress successfully before completing",
        ));
    }
    if checks::command(gui) != check_command {
        return Err(GuiError::conflict(
            "Project check command changed; review the completion again",
        ));
    }
    let current = handoff(gui).map(|h| h.view.session_id);
    if current != handoff_session {
        return Err(GuiError::conflict(
            "Feedback target changed; review the handoff again",
        ));
    }
    if deliver && current.is_none() {
        return Err(GuiError::conflict(
            "This feature has no agent session to hand feedback to",
        ));
    }
    let app = gui.app_for_workflow();
    let s = ai::state(&app.mode)?;
    if !s.summary_open || s.review_history.is_some() {
        return Err(GuiError::conflict(
            "Open the pre-finish summary before completing the review",
        ));
    }
    if ai::view(app)?.running || s.questions.draft.is_some() {
        return Err(GuiError::conflict(
            "Wait for AI work and transfer or discard generated drafts first",
        ));
    }
    let pending = if s.apply_suggestions_on_finish {
        s.pending_suggestion_count()
    } else {
        0
    };
    if pending != apply_suggestions {
        return Err(GuiError::conflict(
            "Suggestions to apply on finish changed; review the completion again",
        ));
    }
    ai::fresh(s)?;
    // The TUI's finish order: the saved opt-in batch, then the check, then
    // the round. The batch consumes the opt-in, so a retry never reapplies.
    if s.apply_suggestions_on_finish {
        app.defer_review_progress_persist = true;
        app.prepare_final_review_suggestions();
        app.defer_review_progress_persist = false;
        app.open_review_summary();
        save(gui);
        if gui.review_context.as_ref().unwrap().check.is_some() {
            checks::cancel(
                gui,
                ReviewCheckStatus::Stale,
                "Suggestions were applied; the completion check runs again".into(),
            );
        }
        let reload_failed = ai::state(&gui.app_for_workflow().mode)?.error.is_some();
        if reload_failed || gui.review_context.as_ref().unwrap().save_error.is_some() {
            // The source writes are kept and reported; nothing is recorded
            // until the reviewer refreshes or retries the save.
            return Ok(Started::Stopped);
        }
    }
    let Some(command) = check_command else {
        return if finish(gui, None, deliver) {
            Ok(Started::Completed)
        } else {
            Err(GuiError::conflict(
                "Could not save final review feedback; the review remains open",
            ))
        };
    };
    let workdir = ai::state(&gui.app_for_workflow().mode)?.workdir.clone();
    match ReviewCheckRun::spawn(&workdir, &command) {
        Ok(run) => {
            let context = gui.review_context.as_mut().unwrap();
            context.check = Some(ReviewCheckView {
                command,
                status: ReviewCheckStatus::Running,
                output: String::new(),
            });
            context.check_run = Some(run);
            context.completion = Some(PendingCompletion {
                deliver,
                handoff_session,
            });
            Ok(Started::Checking)
        }
        // Like the TUI, an environment problem does not block finishing: the
        // round records the check as failed.
        Err(e) => {
            if !finish(
                gui,
                Some(CheckOutcome {
                    command,
                    passed: false,
                    output: format!("failed to start: {e}"),
                }),
                deliver,
            ) {
                return Err(GuiError::conflict(
                    "Could not save final review feedback; the review remains open",
                ));
            }
            Ok(Started::Completed)
        }
    }
}

/// The completion's check finished and its result passed the shared
/// freshness checks. Returns an error, without recording, when the handoff
/// target changed while it ran.
pub(super) fn after_check(gui: &mut GuiHandle, outcome: CheckOutcome) -> GuiResult<bool> {
    let expected = gui
        .review_context
        .as_ref()
        .and_then(|c| c.completion.as_ref())
        .expect("completion pending")
        .handoff_session
        .clone();
    if handoff(gui).map(|h| h.view.session_id) != expected {
        return Err(GuiError::conflict(
            "the feedback target changed while it ran",
        ));
    }
    let pending = gui.review_context.as_mut().unwrap().completion.take();
    Ok(finish(
        gui,
        Some(outcome),
        pending.is_some_and(|p| p.deliver),
    ))
}

/// Record the round with the shared engine, then hand an actionable round's
/// prompt to the target when the reviewer chose to. Closes the review.
fn finish(gui: &mut GuiHandle, check: Option<CheckOutcome>, deliver: bool) -> bool {
    let workflow_id = gui.review_context.as_ref().unwrap().id.clone();
    let handoff = handoff(gui);
    let app = gui.app_for_workflow();
    app.message = None;
    let dispatch = app.record_final_review_round(check);
    if matches!(app.mode, AppMode::DiffViewer(_)) {
        let message = app
            .message
            .take()
            .unwrap_or_else(|| "Could not save final review feedback".into());
        if let Some(check) = &mut gui.review_context.as_mut().unwrap().check {
            check.status = ReviewCheckStatus::Failed;
            check.output = format!("Review not completed: {message}");
        }
        return false;
    }
    let (message, handoff) = match (dispatch, handoff) {
        (None, _) => (
            app.message
                .take()
                .unwrap_or_else(|| "Final review complete".into()),
            None,
        ),
        (Some(d), Some(h)) if deliver => {
            let draft = || ReviewHandoff {
                target: h.target.clone(),
                draft_prompt: Some(REVIEW_FEEDBACK_PROMPT.to_string()),
            };
            if !app.config.final_review_submit_prompt {
                (
                    format!(
                        "{} — the feedback prompt is an unsent draft in {}",
                        d.summary, h.view.label
                    ),
                    Some(draft()),
                )
            } else if h.view.stopped {
                (
                    format!(
                        "{} — {} is stopped; the feedback prompt is waiting as an unsent draft",
                        d.summary, h.view.label
                    ),
                    Some(draft()),
                )
            } else {
                match app.deliver_review_prompt(&h.tmux_session, &h.window, true) {
                    Ok(_) => (
                        format!("{} — sent to {}", d.summary, h.view.label),
                        Some(ReviewHandoff {
                            target: h.target.clone(),
                            draft_prompt: None,
                        }),
                    ),
                    Err(e) => (
                        format!(
                            "{} (couldn't prompt agent: {e}; the prompt is waiting as an unsent draft)",
                            d.summary
                        ),
                        Some(draft()),
                    ),
                }
            }
        }
        (Some(d), Some(_)) => (
            format!("{} (feedback saved; no agent prompted)", d.summary),
            None,
        ),
        (Some(d), None) => (
            format!("{} (feedback saved; no agent session to prompt)", d.summary),
            None,
        ),
    };
    app.mode = AppMode::Normal;
    gui.review_context = None;
    gui.review_completion = Some(ReviewCompletion {
        workflow_id,
        message,
        handoff,
    });
    true
}

/// Hand the completion to the interface once. A repeated or unrelated request
/// gets nothing, so a handoff draft cannot be applied twice.
pub fn take_completion(gui: &mut GuiHandle, workflow_id: &str) -> Option<ReviewCompletion> {
    if gui
        .review_completion
        .as_ref()
        .is_some_and(|c| c.workflow_id == workflow_id)
    {
        gui.review_completion.take()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_contract::GuiErrorKind;
    use crate::gui_diff::tests::fixture;
    use crate::project::{ProjectStatus, ProjectStore};
    use crate::traits::MockTmuxOps;
    use std::time::{Duration, Instant};

    fn act(
        gui: &mut GuiHandle,
        view: &ReviewView,
        action: ReviewAction,
    ) -> GuiResult<Option<ReviewView>> {
        super::super::act(gui, &view.workflow_id, view.revision, action)
    }
    fn open(gui: &mut GuiHandle, view: &ReviewView, action: ReviewAction) -> ReviewView {
        act(gui, view, action).unwrap().unwrap()
    }
    fn edit_store(dir: &tempfile::TempDir, edit: impl FnOnce(&mut ProjectStore)) {
        let db = crate::db::AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = db.load_store().unwrap();
        edit(&mut store);
        db.save_store(&store).unwrap();
    }
    fn feedback(dir: &tempfile::TempDir) -> Option<String> {
        std::fs::read_to_string(dir.path().join("repo/.claude/final-review-feedback.md")).ok()
    }
    fn progress(dir: &tempfile::TempDir) -> Option<Vec<u8>> {
        progress_bytes(&dir.path().join("repo")).unwrap()
    }
    /// A review with a rejected file and its summary open, over a feature
    /// whose first agent session is `status`.
    fn rejected(
        status: ProjectStatus,
        check: Option<&str>,
    ) -> (tempfile::TempDir, GuiHandle, ReviewView) {
        let (dir, mut gui, target) = fixture();
        edit_store(&dir, |store| {
            let feature = &mut store.projects[0].features[0];
            feature.add_session(SessionKind::Terminal);
            feature.add_session(SessionKind::Claude);
            feature.status = status;
        });
        let app = gui.app_for_workflow();
        app.config.final_review_submit_prompt = false;
        app.config.extension.final_review_check_command = check.map(Into::into);
        let view = begin(&mut gui, target).unwrap();
        let view = open(
            &mut gui,
            &view,
            ReviewAction::Reject {
                path: "code.txt".into(),
                feedback: "Rename the crab".into(),
                severity: Severity::Blocker,
            },
        );
        let view = open(&mut gui, &view, ReviewAction::SummaryOpen);
        (dir, gui, view)
    }
    fn complete(view: &ReviewView, deliver: bool) -> ReviewAction {
        let finish = view.finish.as_ref().unwrap();
        ReviewAction::Complete {
            check_command: view.check_command.clone(),
            apply_suggestions: finish.apply_suggestions,
            handoff_session: finish.handoff.as_ref().map(|h| h.session_id.clone()),
            deliver,
        }
    }
    fn settle(gui: &mut GuiHandle, workflow_id: &str) -> Option<ReviewView> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match poll_open(gui, workflow_id).unwrap() {
                Some(view)
                    if matches!(
                        view.check.as_ref().map(|c| &c.status),
                        Some(ReviewCheckStatus::Running)
                    ) =>
                {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
                other => return other,
            }
        }
    }

    #[test]
    fn completion_records_the_round_and_hands_an_unsent_draft_to_the_agent_once() {
        let (dir, mut gui, view) = rejected(ProjectStatus::Active, None);
        let finish = view.finish.as_ref().unwrap();
        assert_eq!((finish.approved, finish.needs_work), (0, 1));
        let session = finish.handoff.clone().unwrap();
        assert_eq!(session.label, "Claude 1");
        assert!(progress(&dir).is_some());
        assert!(
            act(&mut gui, &view, complete(&view, true))
                .unwrap()
                .is_none()
        );

        let round = feedback(&dir).unwrap();
        assert!(round.contains("### Files Needing Revision"));
        assert!(round.contains("Rename the crab"));
        assert!(progress(&dir).is_none());
        assert!(
            dir.path()
                .join("repo/.claude/final-review-snapshot.json")
                .exists()
        );
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        let done = take_completion(&mut gui, &view.workflow_id).unwrap();
        let handoff = done.handoff.unwrap();
        assert_eq!(handoff.target.session_id, session.session_id);
        assert_eq!(
            handoff.draft_prompt.as_deref(),
            Some(crate::app::review::REVIEW_FEEDBACK_PROMPT)
        );
        assert!(done.message.contains("unsent draft in Claude 1"));
        // Taken once; a replayed request cannot complete or hand off again.
        assert!(take_completion(&mut gui, &view.workflow_id).is_none());
        assert!(act(&mut gui, &view, complete(&view, true)).is_err());
        assert_eq!(feedback(&dir).unwrap(), round);
    }

    #[test]
    fn feedback_write_failure_keeps_the_review_and_saved_progress() {
        let (dir, mut gui, view) = rejected(ProjectStatus::Active, None);
        std::fs::create_dir(dir.path().join("repo/.claude/final-review-feedback.md")).unwrap();
        let before = progress(&dir);
        let error = act(&mut gui, &view, complete(&view, true)).unwrap_err();
        assert!(
            error
                .message
                .contains("Could not save final review feedback")
        );
        assert!(matches!(
            gui.app_for_workflow().mode,
            AppMode::DiffViewer(_)
        ));
        assert!(gui.review_context.is_some());
        assert_eq!(progress(&dir), before);
        assert!(take_completion(&mut gui, &view.workflow_id).is_none());

        // The all-approved history path must keep the review too.
        let (dir, mut gui, view) = rejected(ProjectStatus::Active, None);
        let view = open(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        let view = open(&mut gui, &view, ReviewAction::SummaryOpen);
        std::fs::create_dir(dir.path().join("repo/.claude/final-review-feedback.md")).unwrap();
        assert!(act(&mut gui, &view, complete(&view, false)).is_err());
        assert!(gui.review_context.is_some());
        assert!(take_completion(&mut gui, &view.workflow_id).is_none());
    }

    #[test]
    fn feedback_write_failure_after_check_returns_an_open_review() {
        let (dir, mut gui, view) = rejected(ProjectStatus::Active, Some("true"));
        std::fs::create_dir(dir.path().join("repo/.claude/final-review-feedback.md")).unwrap();
        let running = open(&mut gui, &view, complete(&view, false));
        let retained = settle(&mut gui, &running.workflow_id).unwrap();
        assert!(retained.revision > running.revision);
        assert!(matches!(
            retained.check.unwrap().status,
            ReviewCheckStatus::Failed
        ));
        assert!(gui.review_context.is_some());
        assert!(progress(&dir).is_some());
        assert!(take_completion(&mut gui, &running.workflow_id).is_none());
    }

    #[test]
    fn submitted_handoff_uses_the_tui_prompt_delivery_and_declined_handoff_prompts_nobody() {
        let (dir, mut gui, _) = rejected(ProjectStatus::Active, None);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_paste_text()
            .withf(|_, window, text| {
                window == "claude" && text == crate::app::review::REVIEW_FEEDBACK_PROMPT
            })
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_send_key_name()
            .withf(|_, _, key| key == "Enter")
            .times(1)
            .returning(|_, _, _| Ok(()));
        let app = gui.app_for_workflow();
        app.tmux = Box::new(tmux);
        app.config.final_review_submit_prompt = true;
        let view = snapshot(&mut gui).unwrap();
        assert!(view.finish.as_ref().unwrap().submit_prompt);
        assert!(
            act(&mut gui, &view, complete(&view, true))
                .unwrap()
                .is_none()
        );
        let done = take_completion(&mut gui, &view.workflow_id).unwrap();
        assert!(done.handoff.unwrap().draft_prompt.is_none());
        assert!(done.message.contains("sent to Claude 1"));
        assert!(feedback(&dir).is_some());

        // Declining delivers nothing (the default mock panics on any call).
        let (dir, mut gui, view) = rejected(ProjectStatus::Active, None);
        gui.app_for_workflow().config.final_review_submit_prompt = true;
        assert!(
            act(&mut gui, &view, complete(&view, false))
                .unwrap()
                .is_none()
        );
        let done = take_completion(&mut gui, &view.workflow_id).unwrap();
        assert!(done.handoff.is_none());
        assert!(done.message.contains("no agent prompted"));
        assert!(feedback(&dir).unwrap().contains("Rename the crab"));
    }

    #[test]
    fn stopped_targets_keep_a_submitted_prompt_as_a_draft_and_approved_rounds_hand_off_nothing() {
        let (_dir, mut gui, _) = rejected(ProjectStatus::Stopped, None);
        gui.app_for_workflow().config.final_review_submit_prompt = true;
        let view = snapshot(&mut gui).unwrap();
        assert!(
            view.finish
                .as_ref()
                .unwrap()
                .handoff
                .as_ref()
                .unwrap()
                .stopped
        );
        act(&mut gui, &view, complete(&view, true)).unwrap();
        let done = take_completion(&mut gui, &view.workflow_id).unwrap();
        assert!(done.handoff.unwrap().draft_prompt.is_some());
        assert!(done.message.contains("is stopped"));

        let (dir, mut gui, view) = rejected(ProjectStatus::Active, None);
        let view = open(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        let view = open(&mut gui, &view, ReviewAction::SummaryOpen);
        act(&mut gui, &view, complete(&view, true)).unwrap();
        let done = take_completion(&mut gui, &view.workflow_id).unwrap();
        assert!(done.handoff.is_none());
        assert!(done.message.contains("approved"), "{}", done.message);
        // Successful rounds still join the review timeline.
        assert!(feedback(&dir).unwrap().contains("## Review"));
    }

    #[test]
    fn stale_or_unconfirmed_completion_is_refused_before_anything_is_written() {
        type Setup = fn(&tempfile::TempDir, &mut GuiHandle, &ReviewView) -> ReviewAction;
        let cases: [(&str, Setup); 8] = [
            ("revision", |_, gui, view| {
                open(gui, view, ReviewAction::SummaryClose);
                complete(view, true)
            }),
            ("patch", |dir, _, view| {
                std::fs::write(dir.path().join("repo/code.txt"), "changed\n").unwrap();
                complete(view, true)
            }),
            ("progress", |dir, _, view| {
                let path = crate::app::review::review_progress_path(&dir.path().join("repo"));
                std::fs::write(path, "{}").unwrap();
                complete(view, true)
            }),
            ("session removed", |dir, _, view| {
                edit_store(dir, |s| {
                    s.projects[0].features[0].sessions.pop().map(drop).unwrap()
                });
                complete(view, true)
            }),
            ("feature deleted", |dir, _, view| {
                edit_store(dir, |s| s.projects[0].features.clear());
                complete(view, false)
            }),
            ("checkout reassigned", |dir, _, view| {
                edit_store(dir, |s| {
                    s.projects[0].features[0].workdir = dir.path().join("x")
                });
                complete(view, false)
            }),
            ("check configured", |_, gui, view| {
                gui.app_for_workflow()
                    .config
                    .extension
                    .final_review_check_command = Some("true".into());
                complete(view, true)
            }),
            ("unsaved", |_, gui, view| {
                gui.review_context.as_mut().unwrap().save_error = Some("disk full".into());
                complete(view, false)
            }),
        ];
        for (name, setup) in cases {
            let (dir, mut gui, view) = rejected(ProjectStatus::Active, None);
            let before = progress(&dir);
            let request = setup(&dir, &mut gui, &view);
            let current = gui.review_context.as_ref().map(|c| c.revision);
            let revision = if name == "revision" {
                view.revision
            } else {
                current.unwrap_or(view.revision)
            };
            let result = super::super::act(&mut gui, &view.workflow_id, revision, request);
            assert!(result.is_err(), "{name}");
            assert!(feedback(&dir).is_none(), "{name}");
            assert!(
                take_completion(&mut gui, &view.workflow_id).is_none(),
                "{name}"
            );
            if !matches!(name, "progress") {
                assert_eq!(progress(&dir), before, "{name}");
            }
            assert!(gui.review_context.is_some(), "{name}");
        }
        // Without the summary, or without a session to receive it.
        let (_dir, mut gui, view) = rejected(ProjectStatus::Active, None);
        let closed = open(&mut gui, &view, ReviewAction::SummaryClose);
        let err = act(&mut gui, &closed, complete(&view, true)).unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::Conflict);
        let (_dir, mut gui, view) = rejected(ProjectStatus::Active, None);
        let mut request = complete(&view, true);
        if let ReviewAction::Complete {
            handoff_session, ..
        } = &mut request
        {
            *handoff_session = None;
        }
        assert!(act(&mut gui, &view, request).is_err());
    }

    #[test]
    fn completion_reruns_the_configured_check_and_records_its_failure_as_feedback() {
        let (dir, mut gui, view) = rejected(ProjectStatus::Active, Some("echo diagnostic; exit 3"));
        // An earlier transient pass is not trusted as the finishing gate.
        let view = open(
            &mut gui,
            &view,
            ReviewAction::RunCheck {
                command: "echo diagnostic; exit 3".into(),
            },
        );
        let view = settle(&mut gui, &view.workflow_id).unwrap();
        let view = open(
            &mut gui,
            &view,
            ReviewAction::Approve {
                path: "code.txt".into(),
            },
        );
        let view = open(&mut gui, &view, ReviewAction::SummaryOpen);
        let running = open(&mut gui, &view, complete(&view, true));
        assert!(running.finish.as_ref().unwrap().completing);
        assert!(feedback(&dir).is_none());
        // Nothing else changes the review while the result decides it.
        for action in [
            ReviewAction::SummaryClose,
            complete(&running, true),
            ReviewAction::General {
                text: "late".into(),
            },
        ] {
            assert!(act(&mut gui, &running, action).is_err());
        }
        assert!(settle(&mut gui, &running.workflow_id).is_none());
        let round = feedback(&dir).unwrap();
        assert!(round.contains("diagnostic"), "{round}");
        assert!(round.contains("FAIL") || round.contains("fail"), "{round}");
        // A failed check is actionable even when every file was approved.
        let done = take_completion(&mut gui, &running.workflow_id).unwrap();
        assert!(done.handoff.unwrap().draft_prompt.is_some());
        assert!(done.message.contains("FAILED"), "{}", done.message);
    }

    #[test]
    fn cancelled_or_stale_completion_checks_record_no_round_and_keep_the_review_open() {
        let (dir, mut gui, view) = rejected(ProjectStatus::Active, Some("sleep 30"));
        let before = progress(&dir);
        let running = open(&mut gui, &view, complete(&view, true));
        let cancelled = open(&mut gui, &running, ReviewAction::CancelCheck);
        let check = cancelled.check.unwrap();
        assert!(matches!(check.status, ReviewCheckStatus::Cancelled));
        assert!(
            check
                .output
                .starts_with("Review not completed; no feedback round was recorded.")
        );
        assert!(!cancelled.finish.unwrap().completing);
        assert!(feedback(&dir).is_none());
        assert_eq!(progress(&dir), before);

        for scenario in ["patch", "progress", "session"] {
            let (dir, mut gui, view) = rejected(
                ProjectStatus::Active,
                Some("while [ ! -e ../go ]; do sleep 0.02; done"),
            );
            let running = open(&mut gui, &view, complete(&view, true));
            match scenario {
                "patch" => std::fs::write(dir.path().join("repo/code.txt"), "changed\n").unwrap(),
                "progress" => {
                    let path = crate::app::review::review_progress_path(&dir.path().join("repo"));
                    std::fs::write(path, "{}").unwrap();
                }
                _ => edit_store(&dir, |s| {
                    s.projects[0].features[0].sessions.pop();
                }),
            }
            std::fs::write(dir.path().join("go"), "").unwrap();
            let stale = settle(&mut gui, &running.workflow_id).expect(scenario);
            let check = stale.check.unwrap();
            assert!(
                matches!(check.status, ReviewCheckStatus::Stale),
                "{scenario}"
            );
            assert!(
                check.output.starts_with("Review not completed"),
                "{scenario}"
            );
            assert!(feedback(&dir).is_none(), "{scenario}");
            assert!(take_completion(&mut gui, &running.workflow_id).is_none());
        }

        // Pausing during the completion check abandons it without recording.
        let (dir, mut gui, view) = rejected(ProjectStatus::Active, Some("sleep 30"));
        let running = open(&mut gui, &view, complete(&view, true));
        assert!(
            act(&mut gui, &running, ReviewAction::Pause)
                .unwrap()
                .is_none()
        );
        assert!(feedback(&dir).is_none());
        assert!(progress(&dir).is_some());
    }

    #[test]
    fn a_saved_apply_on_finish_opt_in_is_confirmed_applied_then_recorded() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let path = crate::app::review::review_progress_path(&repo);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"line_comments":{"code.txt":[{"location":{"old_line":null,"new_line":8},"text":"","suggestion":"crab line","severity":"nit"}]},"apply_suggestions_on_finish":true}"#).unwrap();
        let view = begin(&mut gui, target).unwrap();
        let view = open(&mut gui, &view, ReviewAction::SummaryOpen);
        assert_eq!(view.finish.as_ref().unwrap().apply_suggestions, 1);
        // A confirmation that did not show the batch is refused.
        let mut stale = complete(&view, false);
        if let ReviewAction::Complete {
            apply_suggestions, ..
        } = &mut stale
        {
            *apply_suggestions = 0;
        }
        assert!(act(&mut gui, &view, stale).is_err());
        assert!(
            !std::fs::read_to_string(repo.join("code.txt"))
                .unwrap()
                .contains("crab line")
        );
        act(&mut gui, &view, complete(&view, false)).unwrap();
        assert!(
            std::fs::read_to_string(repo.join("code.txt"))
                .unwrap()
                .contains("crab line")
        );
        let round = feedback(&dir).unwrap();
        assert!(
            round.contains("Applied locally") || round.contains("applied"),
            "{round}"
        );
        assert!(take_completion(&mut gui, &view.workflow_id).is_some());
    }

    #[test]
    fn cancelling_completion_keeps_suggestions_applied_before_the_check() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let path = crate::app::review::review_progress_path(&repo);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"line_comments":{"code.txt":[{"location":{"old_line":null,"new_line":8},"text":"","suggestion":"crab line","severity":"nit"}]},"apply_suggestions_on_finish":true}"#).unwrap();
        gui.app_for_workflow()
            .config
            .extension
            .final_review_check_command = Some("sleep 30".into());
        let view = begin(&mut gui, target).unwrap();
        let view = open(&mut gui, &view, ReviewAction::SummaryOpen);
        let running = open(&mut gui, &view, complete(&view, false));
        let cancelled = open(&mut gui, &running, ReviewAction::CancelCheck);
        assert!(
            std::fs::read_to_string(repo.join("code.txt"))
                .unwrap()
                .contains("crab line")
        );
        assert!(
            cancelled
                .check
                .unwrap()
                .output
                .contains("Suggestions applied before the check remain")
        );
        assert!(feedback(&dir).is_none());
    }
}
