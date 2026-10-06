//! Explicit check execution that leaves the review open for a last look.
use serde::Serialize;

use super::*;
use crate::app::review::ReviewCheckRun;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCheckStatus {
    Running,
    Passed,
    Failed,
    Cancelled,
    Stale,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewCheckView {
    pub command: String,
    pub status: ReviewCheckStatus,
    pub output: String,
}

pub(super) fn command(gui: &mut GuiHandle) -> Option<String> {
    let target = gui.review_context.as_ref()?.target.clone();
    let app = gui.app_for_workflow();
    let (pi, _) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)?;
    crate::extension::merge_project_extension_config(
        &app.config.extension,
        &app.store.projects[pi].repo,
    )
    .final_review_check_command
    .map(|c| c.trim().to_string())
    .filter(|c| !c.is_empty())
}

/// Stop the owned check (if any) and settle its displayed result. A
/// completion waiting for it is abandoned: nothing is recorded.
pub(super) fn cancel(gui: &mut GuiHandle, status: ReviewCheckStatus, output: String) {
    let context = gui.review_context.as_mut().unwrap();
    context.check_run = None;
    let completing = context.completion.take().is_some();
    if let Some(check) = &mut context.check {
        check.status = status;
        check.output = if completing {
            format!(
                "Review not completed; no feedback round was recorded. Suggestions applied before the check remain in source files. {output}"
            )
        } else {
            output
        };
    }
}

pub(super) fn start(gui: &mut GuiHandle, expected: &str) -> GuiResult<()> {
    let actual = command(gui).ok_or_else(|| {
        GuiError::conflict("No final review check is configured for this project")
    })?;
    if actual != expected {
        return Err(GuiError::conflict(
            "Project check command changed; preview it again before running",
        ));
    }
    let context = gui.review_context.as_ref().unwrap();
    if context.check_run.is_some()
        || context.save_error.is_some()
        || context.ready_comment.is_some()
    {
        return Err(GuiError::conflict(
            "Wait for the check and save or discard pending review edits first",
        ));
    }
    let app = gui.app_for_workflow();
    let state = ai::state(&app.mode)?;
    if !state.summary_open || state.review_history.is_some() {
        return Err(GuiError::conflict(
            "Open the pre-finish summary before running checks",
        ));
    }
    if ai::view(app)?.running || state.questions.draft.is_some() {
        return Err(GuiError::conflict(
            "Wait for AI work and transfer or discard generated drafts first",
        ));
    }
    ai::fresh(state)?;
    let run = ReviewCheckRun::spawn(&state.workdir, &actual);
    let context = gui.review_context.as_mut().unwrap();
    context.check = Some(ReviewCheckView {
        command: actual,
        status: if run.is_ok() {
            ReviewCheckStatus::Running
        } else {
            ReviewCheckStatus::Failed
        },
        output: run
            .as_ref()
            .err()
            .map(|e| format!("Failed to start: {e}"))
            .unwrap_or_default(),
    });
    context.check_run = run.ok();
    Ok(())
}

/// Like `ai::fresh`, but only the reviewed set has to be unchanged. A check
/// may legitimately write files the review never covered (coverage reports,
/// `.snap.new` files, a regenerated lockfile, codegen output), and that must
/// not discard its result. Returns those paths so the result can name them.
fn reviewed_files_unchanged(s: &DiffViewerState) -> GuiResult<Vec<String>> {
    let current = crate::diff::load_snapshot(
        &s.workdir,
        s.override_base_ref.as_deref(),
        s.ignore_whitespace,
    )?;
    let stale = || GuiError::conflict("Reviewed changes changed while the check ran");
    if current.base_commit != s.base_commit {
        return Err(stale());
    }
    for reviewed in &s.files {
        match current.files.iter().find(|f| f.path == reviewed.path) {
            Some(file) if ai::same_file(file, reviewed) => {}
            _ => return Err(stale()),
        }
    }
    Ok(current
        .files
        .into_iter()
        .map(|file| file.path)
        .filter(|path| {
            !crate::app::review::is_review_bookkeeping_path(path)
                && !s.files.iter().any(|f| &f.path == path)
        })
        .collect())
}

fn outside_note(paths: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut names = paths[..paths.len().min(SHOWN)].join(", ");
    if paths.len() > SHOWN {
        names.push_str(&format!(" and {} more", paths.len() - SHOWN));
    }
    format!(
        "\n\nThe check created or changed files outside the reviewed set: {names}. \
         Refresh changes to review them."
    )
}

/// Validation a finished check's result must pass before it is shown.
/// Deferred to completion on purpose: re-reading `amf.json` and re-diffing
/// the checkout on every poll is real I/O for a long check, and the `git`
/// calls can take `index.lock` under a check that runs git itself.
fn validate_result(
    gui: &mut GuiHandle,
    expected: &str,
    progress: &Option<Vec<u8>>,
) -> GuiResult<Vec<String>> {
    if command(gui).as_deref() != Some(expected) {
        return Err(GuiError::conflict("Project check command changed"));
    }
    let state = ai::state(&gui.app_for_workflow().mode)?;
    if &progress_bytes(&state.workdir)? != progress {
        return Err(GuiError::conflict(
            "Saved review changed in another interface",
        ));
    }
    reviewed_files_unchanged(state)
}

/// [`poll_open`] for callers that expect the review to stay open.
pub fn poll(gui: &mut GuiHandle, workflow_id: &str) -> GuiResult<ReviewView> {
    poll_open(gui, workflow_id)?.ok_or_else(|| GuiError::conflict("Final Review was completed"))
}

/// Shared poll command routes check completions by stable workflow identity.
/// A stale result is never saved or accepted as a successful gate. Returns
/// `None` once a confirmed completion's check has finished and the review was
/// completed; its result is then available from `take_completion`.
pub fn poll_open(gui: &mut GuiHandle, workflow_id: &str) -> GuiResult<Option<ReviewView>> {
    let context = gui
        .review_context
        .as_ref()
        .filter(|c| c.id == workflow_id)
        .ok_or_else(|| GuiError::conflict("Final Review is no longer open"))?;
    if context.check_run.is_none() {
        return ai::poll(gui, workflow_id).map(Some);
    }
    let target = context.target.clone();
    let progress = context.progress.clone();
    let expected = context.check.as_ref().unwrap().command.clone();
    // Only reloads the store when its version changed, so this stays cheap
    // per poll. A deleted feature or moved checkout is checked while the
    // command runs because it may be removing the directory under it.
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    let state = ai::state(&app.mode)?;
    let target_valid = match app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
    {
        None => Err(GuiError::not_found("Feature was deleted")),
        Some((pi, fi)) if app.store.projects[pi].features[fi].workdir != state.workdir => {
            Err(GuiError::conflict("Feature checkout changed"))
        }
        Some(_) => Ok(()),
    };
    if let Err(error) = target_valid {
        cancel(
            gui,
            ReviewCheckStatus::Stale,
            format!(
                "Check cancelled; result discarded: {}. Refresh or reload before running again.",
                error.message
            ),
        );
    } else {
        let result = gui
            .review_context
            .as_mut()
            .unwrap()
            .check_run
            .as_mut()
            .unwrap()
            .poll();
        match result {
            Ok(None) => return snapshot(gui).map(Some),
            Ok(Some(outcome)) => match validate_result(gui, &expected, &progress) {
                Ok(_) if gui.review_context.as_ref().unwrap().completion.is_some() => {
                    gui.review_context.as_mut().unwrap().check_run = None;
                    match super::complete::after_check(gui, outcome) {
                        Ok(true) => return Ok(None),
                        Ok(false) => {
                            gui.review_context.as_mut().unwrap().revision += 1;
                            return snapshot(gui).map(Some);
                        }
                        Err(error) => cancel(
                            gui,
                            ReviewCheckStatus::Stale,
                            format!("Check finished, but {}.", error.message),
                        ),
                    }
                }
                Ok(outside) => {
                    let mut output = outcome.output;
                    if !outside.is_empty() {
                        output.push_str(&outside_note(&outside));
                    }
                    let context = gui.review_context.as_mut().unwrap();
                    context.check_run = None;
                    context.check = Some(ReviewCheckView {
                        command: outcome.command,
                        status: if outcome.passed {
                            ReviewCheckStatus::Passed
                        } else {
                            ReviewCheckStatus::Failed
                        },
                        output,
                    });
                }
                Err(error) => cancel(
                    gui,
                    ReviewCheckStatus::Stale,
                    format!(
                        "Check finished; result discarded: {}. Refresh or reload before running again.",
                        error.message
                    ),
                ),
            },
            Err(error) => cancel(
                gui,
                ReviewCheckStatus::Failed,
                format!("Check failed: {error}"),
            ),
        }
    }
    gui.review_context.as_mut().unwrap().revision += 1;
    snapshot(gui).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_diff::tests::fixture;
    use std::time::{Duration, Instant};

    fn act(gui: &mut GuiHandle, view: &ReviewView, action: ReviewAction) -> ReviewView {
        super::super::act(gui, &view.workflow_id, view.revision, action)
            .unwrap()
            .unwrap()
    }
    fn opened(command: &str) -> (tempfile::TempDir, GuiHandle, ReviewView) {
        let (dir, mut gui, target) = fixture();
        gui.app_for_workflow()
            .config
            .extension
            .final_review_check_command = Some(command.into());
        let view = begin(&mut gui, target).unwrap();
        let view = act(&mut gui, &view, ReviewAction::SummaryOpen);
        (dir, gui, view)
    }
    fn run(gui: &mut GuiHandle, view: &ReviewView) -> ReviewView {
        act(
            gui,
            view,
            ReviewAction::RunCheck {
                command: view.check_command.clone().unwrap(),
            },
        )
    }
    fn finish(gui: &mut GuiHandle, view: &ReviewView) -> ReviewView {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let view = poll(gui, &view.workflow_id).unwrap();
            if !matches!(
                view.check.as_ref().unwrap().status,
                ReviewCheckStatus::Running
            ) {
                return view;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn checks_preserve_review_progress_and_source_without_finishing_or_dispatching() {
        let (dir, mut gui, view) = opened("printf 'passed 🦀'; printf 'stderr' >&2");
        let before = progress_bytes(&dir.path().join("repo")).unwrap();
        let source = std::fs::read(dir.path().join("repo/code.txt")).unwrap();
        assert!(view.check.is_none());
        let running = run(&mut gui, &view);
        let view = finish(&mut gui, &running);
        let check = view.check.unwrap();
        assert!(matches!(check.status, ReviewCheckStatus::Passed));
        assert_eq!(check.output, "passed 🦀\nstderr");
        assert!(view.summary.is_some());
        assert!(matches!(
            gui.app_for_workflow().mode,
            AppMode::DiffViewer(_)
        ));
        assert_eq!(progress_bytes(&dir.path().join("repo")).unwrap(), before);
        assert_eq!(
            std::fs::read(dir.path().join("repo/code.txt")).unwrap(),
            source
        );
        assert!(
            !dir.path()
                .join("repo/.claude/final-review-feedback.md")
                .exists()
        );
    }

    #[test]
    fn failed_checks_can_be_rerun_and_old_requests_cannot_launch_twice() {
        let (_dir, mut gui, view) = opened("echo diagnostic; exit 9");
        let running = run(&mut gui, &view);
        assert!(
            super::super::act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::RunCheck {
                    command: view.check_command.clone().unwrap()
                }
            )
            .is_err()
        );
        assert!(
            super::super::act(
                &mut gui,
                &running.workflow_id,
                running.revision,
                ReviewAction::RunCheck {
                    command: view.check_command.clone().unwrap()
                }
            )
            .is_err()
        );
        let failed = finish(&mut gui, &running);
        assert!(matches!(
            failed.check.as_ref().unwrap().status,
            ReviewCheckStatus::Failed
        ));
        assert_eq!(failed.check.as_ref().unwrap().output, "diagnostic");
        let rerun = run(&mut gui, &failed);
        assert!(rerun.revision > failed.revision);
        finish(&mut gui, &rerun);
    }

    #[test]
    fn cancellation_after_target_deletion_and_pause_reap_the_owned_check() {
        let (dir, mut gui, view) = opened("sleep 30");
        let running = run(&mut gui, &view);
        let pid = gui
            .review_context
            .as_ref()
            .unwrap()
            .check_run
            .as_ref()
            .unwrap()
            .id();
        let db = crate::db::AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let original = db.load_store().unwrap();
        let mut deleted = original.clone();
        deleted.projects[0].features.clear();
        db.save_store(&deleted).unwrap();
        let cancelled = act(&mut gui, &running, ReviewAction::CancelCheck);
        assert!(matches!(
            cancelled.check.unwrap().status,
            ReviewCheckStatus::Cancelled
        ));
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        db.save_store(&original).unwrap();
        let view = snapshot(&mut gui).unwrap();
        let running = run(&mut gui, &view);
        let pid = gui
            .review_context
            .as_ref()
            .unwrap()
            .check_run
            .as_ref()
            .unwrap()
            .id();
        assert!(
            super::super::act(
                &mut gui,
                &running.workflow_id,
                running.revision,
                ReviewAction::Pause
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert!(poll(&mut gui, &running.workflow_id).is_err());
        let target = FeatureTarget {
            project_id: original.projects[0].id.clone(),
            feature_id: original.projects[0].features[0].id.clone(),
        };
        assert!(begin(&mut gui, target).unwrap().check.is_none());
    }

    #[test]
    fn changed_command_or_patch_and_unsaved_progress_refuse_launch_before_spawn() {
        let (dir, mut gui, view) = opened("touch should-not-exist");
        gui.app_for_workflow()
            .config
            .extension
            .final_review_check_command = Some("true".into());
        assert!(
            super::super::act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::RunCheck {
                    command: view.check_command.unwrap()
                }
            )
            .is_err()
        );
        let view = snapshot(&mut gui).unwrap();
        gui.review_context.as_mut().unwrap().save_error = Some("disk full".into());
        assert!(
            super::super::act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::RunCheck {
                    command: "true".into()
                }
            )
            .is_err()
        );
        gui.review_context.as_mut().unwrap().save_error = None;
        std::fs::write(dir.path().join("repo/code.txt"), "changed\n").unwrap();
        assert!(
            super::super::act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                ReviewAction::RunCheck {
                    command: "true".into()
                }
            )
            .is_err()
        );
        assert!(gui.review_context.as_ref().unwrap().check_run.is_none());
        assert!(!dir.path().join("repo/should-not-exist").exists());
    }

    fn check_pid(gui: &GuiHandle) -> u32 {
        gui.review_context
            .as_ref()
            .unwrap()
            .check_run
            .as_ref()
            .unwrap()
            .id()
    }

    #[test]
    fn deleted_or_moved_targets_cancel_inflight_checks() {
        for scenario in ["delete", "checkout"] {
            let (dir, mut gui, view) = opened("sleep 30");
            let running = run(&mut gui, &view);
            let pid = check_pid(&gui);
            let db = crate::db::AmfDb::open(&dir.path().join("amf.db")).unwrap();
            let mut store = db.load_store().unwrap();
            if scenario == "delete" {
                store.projects[0].features.clear();
            } else {
                store.projects[0].features[0].workdir = dir.path().join("other");
            }
            db.save_store(&store).unwrap();
            let stale = poll(&mut gui, &running.workflow_id).unwrap();
            assert!(
                matches!(stale.check.unwrap().status, ReviewCheckStatus::Stale),
                "{scenario}"
            );
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1, "{scenario}");
            assert!(stale.summary.is_some());
        }
    }

    #[test]
    fn changed_patch_progress_or_configuration_discard_the_result_on_completion() {
        for scenario in ["patch", "progress", "command"] {
            // Outside the repo, so releasing the check doesn't change the diff.
            let (dir, mut gui, view) = opened("while [ ! -e ../go ]; do sleep 0.02; done");
            let go = dir.path().join("go");
            let running = run(&mut gui, &view);
            match scenario {
                "patch" => std::fs::write(dir.path().join("repo/code.txt"), "changed\n").unwrap(),
                "progress" => {
                    let path = crate::app::review::review_progress_path(&dir.path().join("repo"));
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(path, "{}").unwrap();
                }
                _ => {
                    gui.app_for_workflow()
                        .config
                        .extension
                        .final_review_check_command = Some("false".into())
                }
            }
            // Nothing is re-diffed or re-read while the command runs.
            let polled = poll(&mut gui, &running.workflow_id).unwrap();
            assert!(
                matches!(polled.check.unwrap().status, ReviewCheckStatus::Running),
                "{scenario}"
            );
            std::fs::write(&go, "").unwrap();
            let stale = finish(&mut gui, &running);
            let check = stale.check.unwrap();
            assert!(
                matches!(check.status, ReviewCheckStatus::Stale),
                "{scenario}"
            );
            assert!(check.output.starts_with("Check finished; result discarded"));
            assert!(stale.summary.is_some());
        }
    }

    #[test]
    fn artifacts_outside_the_reviewed_set_keep_the_result_but_reviewed_rewrites_do_not() {
        let (dir, mut gui, view) =
            opened("mkdir -p snaps; printf x > coverage.out; printf y > snaps/a.snap.new");
        let running = run(&mut gui, &view);
        let view = finish(&mut gui, &running);
        let check = view.check.unwrap();
        assert!(matches!(check.status, ReviewCheckStatus::Passed));
        assert!(check.output.contains("outside the reviewed set"));
        assert!(check.output.contains("coverage.out"));
        assert!(dir.path().join("repo/coverage.out").exists());

        let (_dir, mut gui, view) = opened("printf 'formatted\\n' > code.txt");
        let running = run(&mut gui, &view);
        let check = finish(&mut gui, &running).check.unwrap();
        assert!(matches!(check.status, ReviewCheckStatus::Stale));
    }

    #[test]
    fn running_checks_block_review_mutation_and_refresh_invalidates_completed_results() {
        let (_dir, mut gui, view) = opened("true");
        let running = run(&mut gui, &view);
        assert!(
            super::super::act(
                &mut gui,
                &running.workflow_id,
                running.revision,
                ReviewAction::General {
                    text: "overwrite".into()
                }
            )
            .is_err()
        );
        let view = finish(&mut gui, &running);
        let refreshed = act(&mut gui, &view, ReviewAction::Refresh);
        assert!(matches!(
            refreshed.check.unwrap().status,
            ReviewCheckStatus::Stale
        ));
    }
}
