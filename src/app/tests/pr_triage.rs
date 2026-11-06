use super::support::*;
use crate::app::*;
use crate::automation::{SeedAiReviewFinding, SeedAiReviewRequest};
use crate::project::{AgentKind, SessionKind};
use crate::token_tracking::{SessionTokenUsage, TokenUsageProvider, TokenUsageSource};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent};
use std::collections::HashMap;
use tempfile::NamedTempFile;
use tempfile::TempDir;

fn sample_ai_review_finding(body: &str) -> crate::app::ai_review::AiReviewFinding {
    crate::app::ai_review::AiReviewFinding {
        path: None,
        line: None,
        side: None,
        body: body.to_string(),
        diff_hunk: None,
        skipped: false,
        published: false,
    }
}

fn reply_editor_text(app: &App) -> String {
    match &app.mode {
        AppMode::PrReview(state) => state.reply.as_ref().unwrap().editor.text().to_string(),
        _ => unreachable!(),
    }
}

fn pr_review_selected(app: &App) -> usize {
    match &app.mode {
        AppMode::PrReview(state) => state.selected,
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_jump_sibling_cycles_within_the_batch_and_hints_otherwise() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 4);
    // Comments 1 and 3 (indices 0 and 2) were fixed in one combined batch.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments[0].batch_id = Some("batch-x".to_string());
        state.review.comments[2].batch_id = Some("batch-x".to_string());
        state.selected = 0;
    }

    app.pr_review_jump_sibling(true);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    assert_eq!(state.selected, 2, "forward jump lands on the other sibling");

    app.pr_review_jump_sibling(true);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    assert_eq!(state.selected, 0, "jump cycles back around");

    // A comment that isn't part of any batch: no move, and a hint toast.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
    }
    app.pr_review_jump_sibling(true);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    assert_eq!(
        state.selected, 1,
        "no sibling jump off a non-batched comment"
    );
}

/// Enter the review pane with comments built from `(id, path, author, is_bot)`
/// tuples, in that fetch order, for exercising `sort_mode`.
#[test]
fn pr_review_open_without_selection_shows_message() {
    // No projects/features selected, so opening should not enter a PR mode.
    let mut app = pr_review_test_app();
    app.open_pr_review();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.message.is_some());
}

#[test]
fn feature_for_view_resolves_by_project_and_feature_name() {
    let store = store_with_feature(ProjectStatus::Active);
    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let view = view_state_for("my-project", "my-feat");

    let feature = app.feature_for_view(&view).expect("feature should resolve");
    assert_eq!(feature.id, "feat-1");
}

#[test]
fn feature_for_view_returns_none_for_unknown_feature() {
    let store = store_with_feature(ProjectStatus::Active);
    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let view = view_state_for("my-project", "does-not-exist");

    assert!(app.feature_for_view(&view).is_none());
}

#[test]
fn open_pr_review_from_view_is_noop_outside_viewing_mode() {
    // Leader commands only fire while `Viewing`, but guard the entry point
    // itself in case something else ever calls it from another mode.
    let mut app = pr_review_test_app();
    app.mode = AppMode::Normal;

    app.open_pr_review_from_view();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.message.is_none());
}

#[test]
fn open_pr_review_from_view_shows_message_for_unresolvable_feature() {
    // The session view names a project/feature that no longer exists in the
    // store — resolve must fail gracefully rather than panicking or spending
    // a `gh` call on a bogus workdir.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(view_state_for("my-project", "does-not-exist"));

    app.open_pr_review_from_view();

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert_eq!(app.message.as_deref(), Some("No active feature to review"));
}

#[test]
fn dedicated_review_session_working_for_workdir_reads_outside_pr_review_mode() {
    // The ambient Viewing-mode badge calls this while `self.mode` is
    // `Viewing`, not `PrReview` — unlike `pr_review_dedicated_session_working`,
    // it must not depend on the pane being open.
    let mut store = store_with_feature(ProjectStatus::Active);
    let dedicated_session_id = store.projects[0].features[0]
        .add_session_named(
            SessionKind::Claude,
            crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        )
        .id
        .clone();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let workdir = store.projects[0].features[0].workdir.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(view_state_for("my-project", "my-feat"));

    assert_eq!(
        app.dedicated_review_session_working_for_workdir(&workdir),
        Some(false)
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(
        app.dedicated_review_session_working_for_workdir(&workdir),
        Some(true)
    );
}

#[test]
fn ai_review_running_for_workdir_matches_the_pending_reviews_workdir() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let workdir = origin.workdir.clone();
    let other_workdir = PathBuf::from("/tmp/other-workdir");

    // No background job yet.
    assert!(!app.ai_review_running_for_workdir(&workdir));

    let (_tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin));

    assert!(app.ai_review_running_for_workdir(&workdir));
    assert!(!app.ai_review_running_for_workdir(&other_workdir));
}

/// Enter the review pane with comments 1..=n where the given ids are resolved.
fn enter_pr_review_with_resolved(app: &mut App, n: u64, resolved: &[u64]) {
    let comments: Vec<crate::github::ReviewComment> = (1..=n)
        .map(|id| crate::github::ReviewComment {
            id,
            path: Some(format!("src/file{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: "alice".to_string(),
                kind: "User".to_string(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let threads: Vec<crate::github::ReviewThread> = resolved
        .iter()
        .map(|&id| crate::github::ReviewThread {
            id: format!("T{id}"),
            is_resolved: true,
            comment_ids: vec![id],
        })
        .collect();
    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let review = crate::app::pr_review::normalize(pr, comments, vec![], vec![], threads);
    app.mode = AppMode::PrReview(PrReviewState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        review,
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        hide_resolved: false,
        sort_mode: crate::app::pr_review::PrSortMode::default(),
        fix_target: crate::app::pr_review::FixTarget::default(),
        fix_target_picked: false,
        usage_baselines: std::collections::HashMap::new(),
        review_harness: None,
        dedicated_session_label: crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        harness_pick: None,
        new_feature_setup: None,
        integrate: None,
        fix_confirm: None,
        fix_vim_enabled: false,
        mark_pick: None,
        reply_kind_pick: None,
        reply: None,
        memory_add: None,
        marked: std::collections::HashSet::new(),
        pending_batch: false,
        checked_out_branch: Some("main".to_string()),
        pending_ai_review_findings: 0,
        ai_review_last_run: None,
        investigations: Vec::new(),
        investigation_harness_pick: None,
        investigation_action_pick: None,
        investigation_follow_up: None,
        pending_follow_up: None,
        investigation_context: Default::default(),
    });
}

#[test]
fn pr_review_hide_resolved_skips_resolved_comments() {
    // Comments 1,2,3; comment 2 is resolved on GitHub.
    let mut app = pr_review_test_app();
    enter_pr_review_with_resolved(&mut app, 3, &[2]);

    app.pr_review_toggle_resolved();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.hide_resolved);
            assert_eq!(state.visible_indices(), vec![0, 2]);
            assert_eq!(state.hidden_resolved_count(), 1);
        }
        _ => panic!("not in PrReview mode"),
    }

    // Navigation jumps over the hidden (resolved) comment at index 1.
    assert_eq!(pr_review_selected(&app), 0);
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 2);
    app.pr_review_select_prev();
    assert_eq!(pr_review_selected(&app), 0);
}

#[test]
fn pr_review_toggle_snaps_selection_off_hidden_comment() {
    let mut app = pr_review_test_app();
    enter_pr_review_with_resolved(&mut app, 3, &[2]);

    // Park the selection on the resolved comment, then hide resolved.
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 1);
    app.pr_review_toggle_resolved();

    // Index 1 is now hidden, so selection snaps to the next visible comment.
    assert_eq!(pr_review_selected(&app), 2);
}

#[test]
fn pr_review_detail_scroll_clamps() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1); // single short comment

    // Scrolling up at the top is a no-op.
    app.pr_review_scroll_detail_up(5);
    match &app.mode {
        AppMode::PrReview(state) => assert_eq!(state.detail_scroll, 0),
        _ => panic!("not in PrReview mode"),
    }

    // Render once so the detail pane records how many lines it actually drew;
    // the scroll-down clamp bounds against that recorded count.
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    // Scrolling far down clamps to the rendered line count, never past it.
    app.pr_review_scroll_detail_down(1000);
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.detail_content_lines > 0);
            assert_eq!(state.detail_scroll, state.detail_content_lines - 1);
        }
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_selection_change_resets_detail_scroll() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);

    app.pr_review_scroll_detail_down(2);
    app.pr_review_select_next();
    match &app.mode {
        AppMode::PrReview(state) => assert_eq!(state.detail_scroll, 0),
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_open_fix_confirm_without_comments_shows_message() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 0);

    app.pr_review_open_fix_confirm();
    assert_eq!(app.pr_review_fix_editing(), None);
    assert!(app.message.is_some());
}

#[test]
fn poll_ai_pr_review_bg_warns_when_reviewing_and_done_arrive_together() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    tx.send(crate::app::ai_review::AiReviewProgress::Reviewing {
        token_estimate: 50_000,
    })
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: String::new(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("Large diff") && t.message.contains("50000")),
        "toasts: {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
    assert!(
        !app.toasts
            .iter()
            .any(|toast| toast.message.contains("check the debug log")),
        "a valid summary-only response is a clean zero-finding review"
    );
}

#[test]
fn completed_ai_review_updates_stashed_triage_pending_count_and_summary() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![
                sample_ai_review_finding("first"),
                sample_ai_review_finding("second"),
            ],
            summary: Some("Two correctness risks need attention.".to_string()),
            raw_output: "review output".to_string(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(
                state.summary.as_deref(),
                Some("Two correctness risks need attention.")
            );
            assert_eq!(state.findings.len(), 2);
        }
        _ => panic!("expected AI Review pane"),
    }
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.pending_ai_review_findings, 2);
            assert!(matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Findings(2))
            ));
        }
        _ => panic!("expected stashed PR Triage pane"),
    }

    app.ai_review_toggle_skip();
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => assert_eq!(state.pending_ai_review_findings, 1),
        _ => panic!("expected stashed PR Triage pane"),
    }
}

#[test]
fn completed_ai_review_carries_run_attribution_into_the_pane_and_cache() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let pr = origin.pr.clone();

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });
    let attribution = crate::app::ai_review::AiReviewAttribution {
        harness: Some("claude".to_string()),
        model: Some("sonnet".to_string()),
        input_tokens: Some(9_000),
        output_tokens: Some(1_200),
        estimated_cost: Some("$0.05".to_string()),
        ..Default::default()
    };
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![sample_ai_review_finding("first")],
            summary: Some("One risk.".to_string()),
            raw_output: "review output".to_string(),
            attribution: attribution.clone(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.attribution.as_ref(), Some(&attribution));
        }
        _ => panic!("expected AI Review pane"),
    }
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_ai_review_cache(pr.number, &pr.head_sha)
        .unwrap()
        .unwrap();
    assert_eq!(cached.attribution.as_ref(), Some(&attribution));
}

#[test]
fn completed_zero_ai_review_updates_visible_triage_immediately() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let origin = match &app.mode {
        AppMode::PrReview(state) => {
            sample_ai_review_state(state.workdir.clone(), state.review.pr.clone())
        }
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin));
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: "## Summary\nNo actionable issues found.".to_string(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.pending_ai_review_findings, 0);
            assert!(matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Findings(0))
            ));
        }
        _ => panic!("expected visible PR Triage pane"),
    }
}

#[test]
fn ai_review_errors_update_visible_triage_for_agent_and_diff_failures() {
    for detail in ["agent exited with status 1", "failed to fetch PR diff"] {
        let store = store_with_feature(ProjectStatus::Active);
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        enter_pr_review_for_feature(&mut app, 1);
        let origin = match &app.mode {
            AppMode::PrReview(state) => {
                sample_ai_review_state(state.workdir.clone(), state.review.pr.clone())
            }
            _ => unreachable!(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        app.ai_review_run.set_receiver_for_test(Some(rx));
        app.ai_review_run.set_origin_for_test(Some(origin));
        tx.send(crate::app::ai_review::AiReviewProgress::Done(Err(
            anyhow::anyhow!(detail),
        )))
        .unwrap();

        assert!(app.poll_ai_pr_review_bg());
        match &app.mode {
            AppMode::PrReview(state) => assert!(matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Error(message))
                    if message == detail
            )),
            _ => panic!("expected visible PR Triage pane"),
        }
    }
}

#[test]
fn disconnected_ai_review_worker_persists_error_and_updates_triage() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    let (workdir, pr) = match &app.mode {
        AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run
        .set_origin_for_test(Some(sample_ai_review_state(workdir, pr.clone())));
    drop(tx);

    assert!(app.poll_ai_pr_review_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Error(message))
                    if message.contains("disconnected")
            )
    ));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_ai_review_cache(pr.number, &pr.head_sha)
        .unwrap()
        .unwrap();
    assert!(matches!(
        cached.last_run.unwrap().outcome,
        crate::app::ai_review::AiReviewRunOutcome::Error(message)
            if message.contains("disconnected")
    ));
}

#[test]
fn escape_keeps_ai_review_running_through_triage_return_and_completion() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    app.cancel_ai_pr_review();
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    assert!(app.ai_review_run.is_pending());
    app.close_ai_review();
    match &app.mode {
        AppMode::PrReview(state) => assert!(matches!(
            app.ai_review_triage_status(state),
            crate::app::ai_review::AiReviewTriageStatus::Running
        )),
        _ => panic!("expected restored PR Triage pane"),
    }

    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: "## Summary\nNo actionable issues found.".to_string(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Findings(0))
            )
    ));
}

#[test]
fn poll_ai_pr_review_bg_surfaces_streamed_activity_and_usage() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::Reviewing {
                token_estimate: 42_000,
            },
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    tx.send(crate::app::ai_review::AiReviewProgress::Activity(
        "Inspecting the repository".to_string(),
    ))
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Usage {
        input_tokens: 41_000,
        output_tokens: 900,
    })
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReviewRunning(state) => {
            assert_eq!(
                state.progress.activity.as_deref(),
                Some("Inspecting the repository")
            );
            assert_eq!(state.progress.usage, Some((41_000, 900)));
        }
        _ => panic!("expected running AI review"),
    }
}

#[test]
fn running_ai_review_can_reopen_preserved_progress_after_escape() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let started_at = std::time::Instant::now() - std::time::Duration::from_secs(75);
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.ai_review_run
        .set_progress_for_test(Some(crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at,
            activity: None,
            usage: None,
        }));

    // Progress that lands after leaving the full-screen view is retained in
    // the app-level run state rather than discarded with the old mode.
    tx.send(crate::app::ai_review::AiReviewProgress::Reviewing {
        token_estimate: 95_000,
    })
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Activity(
        "Inspecting the repository".to_string(),
    ))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());

    app.start_ai_pr_review();
    match &app.mode {
        AppMode::AiReviewRunning(state) => {
            assert_eq!(state.progress.started_at, started_at);
            assert_eq!(
                state.progress.stage,
                crate::app::ai_review::AiReviewStage::Reviewing {
                    token_estimate: 95_000
                }
            );
            assert_eq!(
                state.progress.activity.as_deref(),
                Some("Inspecting the repository")
            );
        }
        _ => panic!("expected preserved AI review progress"),
    }
}

#[test]
fn open_ai_review_for_pr_reads_the_memory_docs_the_next_run_would_use() {
    use crate::app::review_memory::{MemoryDocState, MemoryScope};

    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- One\n- Two\n",
    )
    .unwrap();
    let mut app = ai_review_test_app();
    // Keeps the test off the developer's real `~/.config/amf/review-memory.md`.
    app.config.global_review_memory_path =
        Some(tmp.path().join("absent-global.md").display().to_string());

    let pr = crate::github::PrRef {
        number: 1,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/1".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    app.open_ai_review_for_pr(repo.clone(), pr);

    let AppMode::AiReview(state) = &app.mode else {
        panic!("expected AiReview");
    };
    let status = state.memory_status.as_ref().expect("status read on open");
    assert_eq!(status.docs[0].scope, MemoryScope::Project);
    assert_eq!(status.docs[0].state, MemoryDocState::Loaded);
    assert_eq!(status.docs[0].findings, 2);
    assert_eq!(
        status.docs[0].path,
        repo.join(".amf").join("review-memory.md")
    );
    assert_eq!(status.docs[1].state, MemoryDocState::Missing);
}

#[test]
fn ai_review_from_linked_worktree_reads_shared_memory_instead_of_local_copy() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("main");
    std::fs::create_dir_all(&repo).unwrap();
    git_out(&repo, &["init", "--initial-branch=main"]);
    std::fs::write(repo.join(".gitignore"), ".amf/\n").unwrap();
    git_out(&repo, &["add", ".gitignore"]);
    git_out(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "initial",
        ],
    );
    let workdir = tmp.path().join("feature");
    git_out(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            workdir.to_str().unwrap(),
        ],
    );
    for (root, text) in [
        (&repo, "- Shared one\n- Shared two\n"),
        (&workdir, "- Stray local finding\n"),
    ] {
        std::fs::create_dir_all(root.join(".amf")).unwrap();
        std::fs::write(root.join(".amf/review-memory.md"), text).unwrap();
    }
    let mut app = ai_review_test_app();
    // Exercise the production git-root lookup rather than an identity mock.
    app.worktree = std::sync::Arc::new(crate::worktree::WorktreeManager);
    app.config.global_review_memory_path = Some(tmp.path().join("absent.md").display().to_string());
    app.open_ai_review_for_pr(workdir.clone(), pr_review_with_comments(1).pr);
    let AppMode::AiReview(state) = &app.mode else {
        panic!("expected AiReview")
    };
    let status = state.memory_status.as_ref().unwrap();
    let shared = repo.canonicalize().unwrap().join(".amf/review-memory.md");
    assert_eq!(status.docs[0].path, shared);
    assert_eq!(status.docs[0].findings, 2);
    // The run resolves its input through this same path, keeping display and delivery aligned.
    let paths = app.review_memory_paths(&app.repo_for_project_path(&workdir));
    assert_eq!(paths.project, shared);
    assert_eq!(
        paths.worktree_local,
        Some(
            workdir
                .canonicalize()
                .unwrap()
                .join(".amf/review-memory.md")
        )
    );
    let loaded = crate::app::review_memory::load_review_memory(&paths);
    assert!(loaded.context.contains("Shared one"));
    assert!(!loaded.context.contains("Stray local"));
}

#[test]
fn open_ai_review_for_pr_starts_empty_with_no_stashed_return() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    // A stash left over from a previous PR Triage-originated visit must not
    // leak into a fresh dashboard/session/picker entry.
    let pr = crate::github::PrRef {
        number: 1,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/1".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    app.ai_review_return_to = Some(Box::new(AppMode::Normal));

    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), pr.clone());

    assert!(app.ai_review_return_to.is_none());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert!(state.findings.is_empty());
            assert_eq!(state.pr.number, 1);
            assert_eq!(state.workdir, PathBuf::from("/tmp/test-workdir"));
        }
        other => panic!("expected AiReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn open_ai_review_for_pr_reopens_cached_findings_and_summary() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    let pr = pr_review_with_comments(1).pr;
    app.db
        .as_ref()
        .unwrap()
        .save_ai_review_cache(
            pr.number,
            &pr.head_sha,
            &crate::app::ai_review::AiReviewCacheEntry {
                findings: vec![sample_ai_review_finding("cached finding")],
                last_run: Some(crate::app::ai_review::AiReviewRun {
                    ran_at: chrono::Local::now(),
                    outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
                }),
                summary: Some("Cached review summary.".to_string()),
                attribution: None,
            },
        )
        .unwrap();
    assert_eq!(app.ai_review_triage_snapshot(&pr).pending_findings, 1);

    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), pr);

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.findings[0].body, "cached finding");
            assert_eq!(state.summary.as_deref(), Some("Cached review summary."));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn seed_ai_review_fixture_persists_and_opens_without_running_an_agent() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    let workdir = tempfile::tempdir().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());

    let response = app
        .seed_ai_review_from_request(&SeedAiReviewRequest {
            pr_number: 55,
            head_sha: "fixture-sha".to_string(),
            summary: "Deterministic completed review.".to_string(),
            findings: vec![SeedAiReviewFinding {
                body: "A deterministic finding.".to_string(),
                ..Default::default()
            }],
            open: true,
            workdir: Some(workdir.path().to_path_buf()),
            repository: Some("owner/repository".to_string()),
            head_ref: Some("fixture-branch".to_string()),
            attribution: None,
        })
        .unwrap();

    assert!(response.ok);
    assert!(response.opened);
    assert_eq!(response.finding_count, 1);
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.pr.number, 55);
            assert_eq!(state.pr.head_sha, "fixture-sha");
            assert_eq!(state.findings[0].body, "A deterministic finding.");
            assert_eq!(
                state.summary.as_deref(),
                Some("Deterministic completed review.")
            );
        }
        _ => panic!("expected seeded AI Review pane"),
    }
}

#[test]
fn pr_triage_zero_result_survives_reopen_and_restart_but_not_new_head() {
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let review = pr_review_with_comments(1);
    let zero_run = crate::app::ai_review::AiReviewRun {
        ran_at: chrono::Local::now(),
        outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(0),
    };

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_path).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&review)
        .unwrap();
    app.db
        .as_ref()
        .unwrap()
        .save_ai_review_cache(
            review.pr.number,
            &review.pr.head_sha,
            &crate::app::ai_review::AiReviewCacheEntry {
                findings: vec![],
                last_run: Some(zero_run.clone()),
                summary: None,
                attribution: None,
            },
        )
        .unwrap();

    app.enter_pr_review(PathBuf::from("/tmp/test-workdir"), review.pr.clone());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if state.ai_review_last_run.as_ref() == Some(&zero_run)
    ));
    app.mode = AppMode::Normal;
    app.enter_pr_review(PathBuf::from("/tmp/test-workdir"), review.pr.clone());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if state.ai_review_last_run.as_ref() == Some(&zero_run)
    ));
    drop(app);

    let store = store_with_feature(ProjectStatus::Idle);
    let mut restarted = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    restarted.db = Some(crate::db::AmfDb::open(&db_path).unwrap());
    restarted.enter_pr_review(PathBuf::from("/tmp/test-workdir"), review.pr.clone());
    assert!(matches!(
        &restarted.mode,
        AppMode::PrReview(state)
            if state.ai_review_last_run.as_ref() == Some(&zero_run)
    ));

    let mut moved = review;
    moved.pr.head_sha = "new-head".to_string();
    restarted
        .db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&moved)
        .unwrap();
    restarted.enter_pr_review(PathBuf::from("/tmp/test-workdir"), moved.pr.clone());
    assert!(matches!(
        &restarted.mode,
        AppMode::PrReview(state)
            if state.review.pr.head_sha == "new-head"
                && state.ai_review_last_run.is_none()
                && state.pending_ai_review_findings == 0
    ));
}

#[test]
fn refreshed_pr_head_clears_stale_ai_review_completion() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.ai_review_last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(0),
        });
    }
    app.open_ai_review_from_triage();
    let (workdir, old_pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let mut moved = pr_review_with_comments(2);
    moved.pr.head_sha = "new-head".to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir,
        pr: old_pr,
    });
    tx.send(Ok(moved)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.review.pr.head_sha, "new-head");
            assert!(state.ai_review_last_run.is_none());
            assert_eq!(state.pending_ai_review_findings, 0);
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn ai_review_post_dialog_is_seeded_with_generated_summary_and_attribution() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings = vec![sample_ai_review_finding("A general finding")];
        state.summary = Some("The patch has one correctness risk.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let body = state.post_confirm.as_ref().unwrap().editor.text();
            assert!(body.starts_with("The patch has one correctness risk."));
            assert!(body.contains("A general finding"));
            assert!(body.ends_with("— AI review via AMF"));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn ai_review_post_dialog_seeds_usage_summary_only_on_the_overall_review() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        let mut anchored = sample_ai_review_finding("An anchored finding");
        anchored.path = Some("src/lib.rs".to_string());
        anchored.line = Some(10);
        anchored.side = Some(crate::diff::DiffSide::New);
        anchored.diff_hunk = Some("@@ -1 +1 @@".to_string());
        state.findings = vec![anchored];
        state.summary = Some("One risk.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
        });
        state.attribution = Some(crate::app::ai_review::AiReviewAttribution {
            harness: Some("claude".to_string()),
            model: Some("sonnet".to_string()),
            input_tokens: Some(12_300),
            output_tokens: Some(4_500),
            estimated_cost: Some("$0.10".to_string()),
            ..Default::default()
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let post = state.post_confirm.as_ref().unwrap();
            let body = post.editor.text();
            assert!(
                body.contains(
                    "<details>\n<summary>AI review usage · harness claude · model sonnet"
                ) && body.contains("\n\n- Harness: claude\n- Model: sonnet"),
                "summary should carry the usage section: {body}"
            );
            assert!(body.contains("Input tokens: 12.3k"));
            assert!(body.contains("Estimated cost: $0.10"));
            assert!(body.ends_with("— AI review via AMF"));
            assert!(
                !post.inline[0].body.contains("AI review usage"),
                "inline comment must not carry the usage section: {}",
                post.inline[0].body
            );
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn ai_review_post_dialog_drops_generated_summary_when_a_finding_is_skipped() {
    // Regression: `state.summary` is model prose written over the *complete*
    // finding set, so it can still describe a finding the user has since
    // skipped (a false positive, or one too sensitive to post) even though
    // that finding is excluded from the posted findings below it. Once
    // anything's skipped, the dialog must fall back to the generic
    // placeholder rather than risk republishing what `skipped` was meant to
    // suppress.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        let mut skipped = sample_ai_review_finding("A sensitive finding");
        skipped.skipped = true;
        state.findings = vec![skipped, sample_ai_review_finding("A kept finding")];
        state.summary = Some("The patch has a sensitive issue plus a kept finding.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(2),
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let body = state.post_confirm.as_ref().unwrap().editor.text();
            assert!(
                !body.contains("sensitive issue"),
                "the model summary describing the skipped finding must not be posted: {body}"
            );
            assert!(body.starts_with("AI review, via AMF."));
            assert!(body.contains("A kept finding"));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn open_ai_review_from_triage_stashes_the_pane_and_close_restores_it() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let triage_workdir = match &app.mode {
        AppMode::PrReview(state) => state.workdir.clone(),
        _ => unreachable!(),
    };

    app.open_ai_review_from_triage();

    match &app.mode {
        AppMode::AiReview(state) => assert_eq!(state.workdir, triage_workdir),
        other => panic!("expected AiReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.ai_review_return_to.is_some());

    app.close_ai_review();
    assert!(matches!(&app.mode, AppMode::PrReview(_)));
    assert!(app.ai_review_return_to.is_none());
}

#[test]
fn post_success_refresh_updates_stashed_triage_without_leaving_ai_review() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
        state.hide_resolved = true;
        state.marked.insert(2);
    }
    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh { workdir, pr });
    tx.send(Ok(pr_review_with_comments(3))).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.review.comments.len(), 3);
            assert_eq!(state.selected_comment().map(|comment| comment.id), Some(2));
            assert!(state.hide_resolved);
            assert!(state.marked.contains(&2));
            assert_eq!(state.pending_ai_review_findings, 0);
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_snaps_selection_off_a_newly_resolved_comment() {
    // Regression: restoring the selection by id after a refresh can land it
    // on a comment that `hide_resolved` now excludes (its thread resolved
    // upstream since the last fetch), leaving `selected` pointing at a row
    // `visible_indices()` doesn't include.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 3);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1; // comment id 2, not yet resolved
        state.hide_resolved = true;
    }
    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh { workdir, pr });

    // The refresh comes back with comment 2's thread now resolved.
    let comments: Vec<crate::github::ReviewComment> = (1..=3u64)
        .map(|id| crate::github::ReviewComment {
            id,
            path: Some(format!("src/file{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: "alice".to_string(),
                kind: "User".to_string(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let threads = vec![crate::github::ReviewThread {
        id: "T2".to_string(),
        is_resolved: true,
        comment_ids: vec![2],
    }];
    let refreshed_pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let refreshed =
        crate::app::pr_review::normalize(refreshed_pr, comments, vec![], vec![], threads);
    tx.send(Ok(refreshed)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_ne!(
                state.selected_comment().map(|comment| comment.id),
                Some(2),
                "selection must not stay on a comment hide_resolved now excludes"
            );
            assert!(
                state.visible_indices().contains(&state.selected),
                "selection must land on a row the current filter shows"
            );
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_snaps_selection_off_a_newly_collated_amf_reply() {
    use crate::app::pr_review::AMF_ATTRIBUTION_FOOTER;

    // Regression: `selected` can itself be an orphaned AMF follow-up reply
    // (its root wasn't fetched, so it stood as its own row). If a refresh
    // fetches the root, the reply becomes collated under it and vanishes
    // from `visible_indices()`. `all_sorted_indices()` used to also drop
    // collated replies, so `position()` couldn't find `selected` there
    // either and both neighbor searches short-circuited to `visible[0]`
    // instead of the nearest neighbor (the reply's own root).
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 1);

    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let user = |login: &str| crate::github::GhUser {
        login: login.to_string(),
        kind: "User".to_string(),
    };

    // An unrelated comment that sorts before the reply's root in fetch order.
    // Its presence is what distinguishes "snap to the nearest neighbor" from
    // "snap to the first visible row" — with only the reply and its root,
    // the root would also happen to be `visible[0]`, masking the bug.
    let unrelated = crate::github::ReviewComment {
        id: 3,
        path: Some("src/other.rs".into()),
        line: Some(4),
        original_line: Some(4),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: "Unrelated finding.".into(),
        user: user("reviewer"),
        in_reply_to_id: None,
        pull_request_review_id: None,
    };

    // Initial fetch: the AMF reply came back but its root did not, so it's
    // orphaned and shown as its own row. It's selected.
    let orphan_reply = crate::github::ReviewComment {
        id: 2,
        path: Some("src/lib.rs".into()),
        line: Some(10),
        original_line: Some(10),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: format!("Done in `abc123`.\n\n{AMF_ATTRIBUTION_FOOTER}"),
        user: user("author"),
        in_reply_to_id: Some(1),
        pull_request_review_id: Some(91),
    };
    app.mode = AppMode::PrReview(PrReviewState {
        selected: 1,
        review: crate::app::pr_review::normalize(
            pr.clone(),
            vec![unrelated.clone(), orphan_reply.clone()],
            vec![],
            vec![],
            vec![],
        ),
        ..match app.mode {
            AppMode::PrReview(state) => state,
            _ => unreachable!(),
        }
    });

    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir,
        pr: pr.clone(),
    });

    // The refresh comes back with the reply's root now present, so the
    // reply collates under it and drops out of `visible_indices()`.
    let root = crate::github::ReviewComment {
        id: 1,
        path: Some("src/lib.rs".into()),
        line: Some(10),
        original_line: Some(10),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: "Please add a test here.".into(),
        user: user("reviewer"),
        in_reply_to_id: None,
        pull_request_review_id: Some(90),
    };
    let refreshed = crate::app::pr_review::normalize(
        pr,
        vec![unrelated, root, orphan_reply],
        vec![],
        vec![],
        vec![],
    );
    tx.send(Ok(refreshed)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_ne!(
                state.selected_comment().map(|comment| comment.id),
                Some(2),
                "selection must not stay on the now-collated AMF reply"
            );
            assert_eq!(
                state.selected_comment().map(|comment| comment.id),
                Some(1),
                "selection should snap to the reply's own root, its nearest visible neighbor"
            );
            assert!(
                state.visible_indices().contains(&state.selected),
                "selection must land on a row the current filter shows"
            );
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_caches_fresh_triage_when_ai_review_has_no_return_pane() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    let stale = pr_review_with_comments(1);
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&stale)
        .unwrap();
    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), stale.pr.clone());

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir: PathBuf::from("/tmp/test-workdir"),
        pr: stale.pr.clone(),
    });
    tx.send(Ok(pr_review_with_comments(3))).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_pr_review_cache(stale.pr.number, &stale.pr.head_sha)
        .unwrap()
        .unwrap();
    assert_eq!(cached.comments.len(), 3);
}

#[test]
fn close_ai_review_with_no_stash_returns_to_the_dashboard() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    assert!(app.ai_review_return_to.is_none());

    app.close_ai_review();

    assert!(matches!(&app.mode, AppMode::Normal));
}

#[test]
fn pr_picker_choose_ai_review_is_noop_outside_picker_mode() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.pr_picker_choose_ai_review();
    assert!(matches!(&app.mode, AppMode::Normal));
}

// ---------------------------------------------------------------------------
// PR Triage: the `New feature…` fix target (companion triage feature)
// ---------------------------------------------------------------------------

/// An app whose single feature lives at `/tmp/test-workdir`, with a worktree
/// mock that resolves the repo root — enough for the fix-target picker and the
/// triage-feature setup overlay, neither of which touches git.
fn triage_target_app(store: ProjectStore) -> App {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree))
}

fn triage_setup(app: &App) -> &crate::app::TriageFeatureSetupState {
    match &app.mode {
        AppMode::PrReview(state) => state
            .new_feature_setup
            .as_ref()
            .expect("triage-feature setup overlay should be open"),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

/// Move the fix-target picker's highlight onto the `New feature…` row.
fn select_new_feature_row(app: &mut App) {
    if let AppMode::PrReview(state) = &mut app.mode {
        let pick = state
            .harness_pick
            .as_mut()
            .expect("fix-target picker should be open");
        pick.selected = pick
            .rows
            .iter()
            .position(|r| matches!(r, crate::app::pr_review::FixTargetPickRow::NewFeature))
            .expect("New feature… row should be present");
    }
}

#[test]
fn pr_review_fix_target_picker_offers_new_feature_last() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let pick = state.harness_pick.as_ref().expect("picker should be open");
            assert_eq!(
                pick.rows.last(),
                Some(&crate::app::pr_review::FixTargetPickRow::NewFeature),
                "the isolated option is last so it reads as the deliberate choice"
            );
            // …and it is not what the cursor lands on by default.
            assert_ne!(pick.selected, pick.rows.len() - 1);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn choosing_new_feature_opens_setup_instead_of_the_fix_confirm() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none(), "picker closes");
            assert!(
                state.fix_confirm.is_none(),
                "nothing is injected until the feature exists"
            );
            assert!(state.new_feature_setup.is_some(), "setup overlay opens");
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    // The branch is pre-filled from the PR's head branch, and is deliberately
    // NOT that branch — git can't check one branch out in two worktrees.
    let setup = triage_setup(&app);
    assert_eq!(setup.branch, "main-triage");
    assert_ne!(setup.branch, "main");
}

#[test]
fn triage_feature_records_the_prs_own_branch_not_the_checked_out_one() {
    // The documented "other PR" flow triages a PR whose head branch is not what
    // the feature has checked out. `TriageSource::pr_branch` is used verbatim as
    // the `git push origin <triage>:<dest>` destination, so reading it off the
    // source feature would push review fixes onto an unrelated remote branch.
    let repo_dir = TempDir::new().unwrap();
    let repo = repo_dir.path().to_path_buf();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
    };
    git(&repo, &["init", "-q", "-b", "my-feat"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "base"]);
    // The PR lives on its own branch, one commit ahead of what's checked out —
    // so `my-feat` does not even contain the PR head.
    git(&repo, &["checkout", "-q", "-b", "pr-head"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "pr work"]);
    let head_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git(&repo, &["checkout", "-q", "my-feat"]);

    let triage_workdir = repo.join(".worktrees").join("pr-head-triage");
    std::fs::create_dir_all(&triage_workdir).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_for_root = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_for_root.clone()));
    let triage_workdir_clone = triage_workdir.clone();
    worktree
        .expect_create_from()
        .times(1)
        // The worktree is branched off the PR's branch, not the checkout's.
        .withf(|_, _, branch, base| branch == "pr-head-triage" && base == "pr-head")
        .returning(move |_, _, _, _| Ok(triage_workdir_clone.clone()));

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| false);
    tmux.expect_create_session_with_window()
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].repo = repo.clone();
    store.projects[0].features[0].workdir = repo.clone();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(worktree));
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();

    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.clone();
        state.review.pr.head_ref = "pr-head".to_string();
        state.review.pr.head_sha = head_sha.clone();
        state.checked_out_branch = Some("my-feat".to_string());
    }

    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();
    // The overlay pre-fills from the PR's branch, and the created feature has
    // to agree with it.
    assert_eq!(triage_setup(&app).branch, "pr-head-triage");
    app.pr_review_triage_setup_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => assert!(
            state.new_feature_setup.is_none(),
            "creation failed: {:?}",
            state
                .new_feature_setup
                .as_ref()
                .and_then(|s| s.error.clone())
        ),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    let companion = app.store.projects[0]
        .features
        .iter()
        .find(|f| f.name == "pr-head-triage")
        .expect("the companion feature should have been created");
    let link = companion
        .triage_source
        .as_ref()
        .expect("the companion is linked back to the PR");
    assert_eq!(
        link.pr_branch, "pr-head",
        "fixes land on the PR's branch, not the source feature's `my-feat`"
    );
    assert_eq!(link.source_feature_id, "feat-1");
}

#[test]
fn triage_setup_branch_is_deduplicated_against_existing_features() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    // A previous triage feature for this branch already claimed the name.
    let mut earlier = store.projects[0].features[0].clone();
    earlier.id = "feat-2".to_string();
    earlier.name = "main-triage".to_string();
    earlier.workdir = PathBuf::from("/tmp/test-workdir-triage");
    store.projects[0].features.push(earlier);

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    assert_eq!(triage_setup(&app).branch, "main-triage-2");
}

#[test]
fn triage_setup_rows_cycle_their_values() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    // Row 0 is Preset; move to Mode and cycle it. This is the setting the
    // whole target exists for — triaging in a different mode than the PR was
    // built in.
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibeless);
    app.pr_review_triage_setup_move(2);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Mode
    );
    app.pr_review_triage_setup_adjust(1);
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibe);
    app.pr_review_triage_setup_adjust(-1);
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibeless);

    // Booleans toggle regardless of direction.
    app.pr_review_triage_setup_move(1);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Review
    );
    assert!(!triage_setup(&app).review);
    app.pr_review_triage_setup_adjust(1);
    assert!(triage_setup(&app).review);

    // Row movement wraps: from Review (index 3), -4 lands on Branch (index 5).
    app.pr_review_triage_setup_move(-4);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Branch
    );
}

#[test]
fn triage_setup_branch_row_takes_typed_input() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    // Not on the branch row: typing is not routed here at all.
    assert!(!app.pr_review_triage_setup_on_branch_row());

    app.pr_review_triage_setup_move(5);
    assert!(app.pr_review_triage_setup_on_branch_row());
    app.pr_review_triage_setup_branch_backspace();
    app.pr_review_triage_setup_branch_push('X');
    assert_eq!(triage_setup(&app).branch, "main-triagX");

    // Cycling is a no-op on the text row (nothing to cycle through).
    app.pr_review_triage_setup_adjust(1);
    assert_eq!(triage_setup(&app).branch, "main-triagX");
}

#[test]
fn triage_setup_preset_fills_in_the_rows_below_it() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
    let mut app = triage_target_app(store);
    app.active_extension.feature_presets = vec![crate::extension::FeaturePreset {
        name: "Careful triage".to_string(),
        branch_prefix: Some("triage/".to_string()),
        mode: VibeMode::Vibeless,
        agent: AgentKind::Codex,
        review: true,
        plan_mode: true,
        quick_plan: false,
        enable_chrome: true,
        remote_control: false,
    }];
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    assert_eq!(triage_setup(&app).preset_label(), "Manual");
    // Row 0 is the preset row; cycling forward selects the only preset.
    app.pr_review_triage_setup_adjust(1);

    let setup = triage_setup(&app);
    assert_eq!(setup.preset_label(), "Careful triage");
    assert_eq!(setup.agent(), AgentKind::Codex);
    assert!(setup.review);
    assert!(setup.enable_chrome);
    assert_eq!(
        setup.branch, "triage/main-triage",
        "the preset's branch prefix applies to the companion branch too"
    );
}

#[test]
fn cancelling_triage_setup_leaves_the_fix_target_unresolved() {
    // Backing out of the setup means no target was chosen — the next `f` must
    // re-offer every option rather than silently using the default.
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    app.pr_review_triage_setup_cancel();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.new_feature_setup.is_none());
            assert!(state.fix_confirm.is_none());
            assert!(!state.fix_target_picked);
            assert!(!state.pending_batch);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_some()),
        "the picker is offered again"
    );
}

/// Attach a companion triage feature for PR #7 to the store, linked back to
/// the source feature the way `create_triage_feature` persists it.
fn push_companion_feature(store: &mut ProjectStore, agent: AgentKind, mode: VibeMode) {
    let source_id = store.projects[0].features[0].id.clone();
    let mut companion = store.projects[0].features[0].clone();
    companion.id = "feat-triage".to_string();
    companion.name = "main-triage".to_string();
    companion.branch = "main-triage".to_string();
    companion.workdir = PathBuf::from("/tmp/test-workdir/.worktrees/main-triage");
    companion.is_worktree = true;
    companion.agent = agent;
    companion.mode = mode;
    companion.sessions = vec![];
    companion.add_session_named(SessionKind::Claude, "PR Triage".to_string());
    companion.triage_source = Some(crate::project::TriageSource {
        pr_number: 7,
        source_feature_id: source_id,
        // The PR's head branch (`pr_review_with_comments` puts the PR on
        // `main`), not the source feature's own branch — that's what
        // `create_triage_feature` records, and it's the push destination.
        pr_branch: "main".to_string(),
        base_sha: "basesha".to_string(),
    });
    store.projects[0].features.push(companion);
}

#[test]
fn companion_target_resolves_sessions_in_the_triage_feature() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    // A same-named session in the *source* feature must not be mistaken for
    // the companion's — that would inject fixes into the wrong worktree.
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibeless);

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }

    assert_eq!(
        app.pr_review_target_feature(),
        Some((0, 1)),
        "the companion feature, not the source one"
    );
}

#[test]
fn opening_a_pr_with_an_existing_companion_adopts_it() {
    // The companion is reused for every fix in the PR, across pane re-opens
    // and restarts — so entering the pane must not re-ask for a target.
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);

    app.adopt_existing_triage_feature();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.fix_target,
                crate::app::pr_review::FixTarget::NewFeature
            );
            assert!(state.fix_target_picked, "no re-ask on the next fix");
            assert_eq!(state.review_harness, Some(AgentKind::Codex));
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_none()
            && state.new_feature_setup.is_none()
            && state.fix_confirm.is_some()),
        "the fix goes straight to the confirm dialog in the adopted feature"
    );
}

#[test]
fn adopt_is_a_no_op_without_a_companion_for_this_pr() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    // Same feature, different PR: the link is keyed on the PR number too.
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .pr_number = 99;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if !state.fix_target_picked),
        "an unrelated PR's triage feature is not adopted"
    );
}

#[test]
fn a_vanished_companion_really_does_reopen_the_picker() {
    // The error text promises "press f and pick a target again", so the pane
    // must actually un-resolve the target — both `fix_target_picked` and
    // `review_harness` short-circuit the picker.
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    // The companion is deleted mid-visit (from the dashboard, say).
    app.store.projects[0].features.remove(1);
    app.pr_review_inject_fix().unwrap();

    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("no longer exists")),
        "the pane reports it: {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(!state.fix_target_picked);
            assert!(
                state.review_harness.is_none(),
                "the dead harness is dropped"
            );
        }
        other => panic!(
            "the pane stays open so `f` is still reachable, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_some()),
        "`f` re-offers every target instead of resolving against the dead one"
    );
}

#[test]
fn fix_confirm_names_the_companion_feature_and_its_mode() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibeless);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    assert_eq!(
        app.pr_review_triage_feature_summary().as_deref(),
        Some("main-triage · Codex · Vibeless")
    );

    // The in-feature targets say nothing extra — they run where the user
    // already is.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::DedicatedReview;
    }
    assert!(app.pr_review_triage_feature_summary().is_none());
}

#[test]
fn integrate_is_rejected_for_the_in_feature_targets() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_open_integrate();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.integrate.is_none()),
        "the dedicated/live targets already commit on the PR branch"
    );
    assert!(!app.toasts.is_empty(), "the user is told why");
}

#[test]
fn integrate_reports_the_commits_and_blocks_cherry_pick_on_a_dirty_source() {
    use crate::app::TriageIntegration;

    // Real git: a source repo with a commit, and a companion worktree branched
    // from it carrying one triage commit.
    let repo = TempDir::new().unwrap();
    let source = repo.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    };
    git(&source, &["init", "-q", "-b", "main"]);
    git(&source, &["config", "user.email", "t@example.com"]);
    git(&source, &["config", "user.name", "T"]);
    std::fs::write(source.join("a.txt"), "one\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "base"]);
    let base_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&source)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let triage = repo.path().join("triage");
    git(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "main-triage",
            triage.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(triage.join("a.txt"), "one\ntwo\n").unwrap();
    git(&triage, &["add", "-A"]);
    git(&triage, &["commit", "-qm", "apply review comment"]);

    // Dirty the source worktree — the cherry-pick must refuse rather than
    // clobber in-progress work.
    std::fs::write(source.join("b.txt"), "wip\n").unwrap();

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].workdir = source.clone();
    store.projects[0].features[0].branch = "main".to_string();
    push_companion_feature(&mut store, AgentKind::Claude, VibeMode::Vibeless);
    store.projects[0].features[1].workdir = triage.clone();
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .base_sha = base_sha;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = source.clone();
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }

    app.pr_review_open_integrate();

    let (commits, source_dirty) = match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().expect("overlay should be open");
            assert_eq!(integrate.triage_branch, "main-triage");
            assert_eq!(integrate.pr_branch, "main");
            (integrate.commits.clone(), integrate.source_dirty.clone())
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    };
    assert_eq!(commits.len(), 1, "one triage commit to land");
    assert!(commits[0].contains("apply review comment"));
    assert!(
        source_dirty.is_some(),
        "the dirty source worktree disables the cherry-pick"
    );

    // Confirming the cherry-pick refuses instead of running it, and leaves the
    // source worktree exactly as it was.
    if let AppMode::PrReview(state) = &mut app.mode {
        let integrate = state.integrate.as_mut().unwrap();
        integrate.selected = TriageIntegration::ALL
            .iter()
            .position(|o| *o == TriageIntegration::CherryPick)
            .unwrap();
    }
    app.pr_review_integrate_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().unwrap();
            assert!(
                integrate
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("Cherry-pick refused")),
                "got {:?}",
                integrate.error
            );
            assert!(integrate.done.is_none());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert_eq!(
        std::fs::read_to_string(source.join("a.txt")).unwrap(),
        "one\n",
        "the source worktree is untouched"
    );
}

#[test]
fn integrate_cherry_picks_into_a_clean_source_worktree() {
    let repo = TempDir::new().unwrap();
    let source = repo.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    };
    git(&source, &["init", "-q", "-b", "main"]);
    git(&source, &["config", "user.email", "t@example.com"]);
    git(&source, &["config", "user.name", "T"]);
    std::fs::write(source.join("a.txt"), "one\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "base"]);
    let base_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&source)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let triage = repo.path().join("triage");
    git(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "main-triage",
            triage.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(triage.join("b.txt"), "fix\n").unwrap();
    git(&triage, &["add", "-A"]);
    git(&triage, &["commit", "-qm", "apply review comment"]);

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].workdir = source.clone();
    store.projects[0].features[0].branch = "main".to_string();
    push_companion_feature(&mut store, AgentKind::Claude, VibeMode::Vibeless);
    store.projects[0].features[1].workdir = triage.clone();
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .base_sha = base_sha;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = source.clone();
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }
    app.pr_review_open_integrate();
    if let AppMode::PrReview(state) = &mut app.mode {
        let integrate = state.integrate.as_mut().unwrap();
        assert!(integrate.source_dirty.is_none());
        integrate.selected = crate::app::TriageIntegration::ALL
            .iter()
            .position(|o| *o == crate::app::TriageIntegration::CherryPick)
            .unwrap();
    }

    app.pr_review_integrate_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().unwrap();
            assert!(integrate.error.is_none(), "got {:?}", integrate.error);
            assert!(
                integrate
                    .done
                    .as_deref()
                    .is_some_and(|d| d.contains("Cherry-picked 1 commit")),
                "got {:?}",
                integrate.done
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(
        source.join("b.txt").exists(),
        "the triage commit landed in the source worktree"
    );
}

#[test]
fn closed_fetch_cannot_replace_a_reopened_pr_with_its_queued_result() {
    let mut app = pr_review_test_app();
    let workdir = tempfile::TempDir::new().unwrap();
    let old_review = pr_review_with_comments(1);
    let (old_tx, old_rx) = std::sync::mpsc::channel();
    app.pr_review_work.begin_fetch(old_rx);
    app.mode = AppMode::PrReviewLoading(PrReviewLoadState {
        workdir: workdir.path().to_path_buf(),
        pr: old_review.pr.clone(),
        usage_baselines: HashMap::new(),
    });
    old_tx.send(Ok(old_review)).unwrap();

    app.close_pr_review();
    assert!(!app.poll_pr_review_bg());
    assert!(matches!(app.mode, AppMode::Normal));

    let mut new_review = pr_review_with_comments(0);
    new_review.pr.number = 2;
    let (new_tx, new_rx) = std::sync::mpsc::channel();
    app.pr_review_work.begin_fetch(new_rx);
    app.mode = AppMode::PrReviewLoading(PrReviewLoadState {
        workdir: workdir.path().to_path_buf(),
        pr: new_review.pr.clone(),
        usage_baselines: HashMap::new(),
    });
    assert!(
        old_tx
            .send(Err(anyhow::anyhow!("late old failure")))
            .is_err()
    );
    new_tx.send(Ok(new_review)).unwrap();
    assert!(app.poll_pr_review_bg());
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected the reopened PR");
    };
    assert_eq!(state.review.pr.number, 2);
    assert!(state.review.comments.is_empty());
    assert!(!app.pr_review_work.fetch_pending());
}

/// PR Triage opened, then AI Review opened over it from `A`, with `n` findings.
fn ai_review_over_triage(app: &mut App, n: usize) {
    enter_pr_review(app, 2);
    app.open_ai_review_from_triage();
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings = (0..n)
            .map(|i| sample_ai_review_finding(&format!("finding {i}")))
            .collect();
    }
}

#[test]
fn ai_review_fix_hands_the_finding_to_triage_without_posting() {
    let mut app = ai_review_test_app();
    ai_review_over_triage(&mut app, 2);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.selected = 1;
    }

    app.ai_review_fix_selected();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR Triage after handing off a finding");
    };
    let comment = state.selected_comment().unwrap();
    assert!(comment.is_local_finding());
    assert!(comment.body.contains("finding 1"));
    let confirm = state.fix_confirm.as_ref().expect("fix dialog should open");
    assert!(confirm.batch.is_none());
    assert!(confirm.editor.text().contains("finding 1"));
    // Nothing to reply to on GitHub, so no reply-draft handoff.
    assert!(!confirm.editor.text().contains("amf reply-draft"));
    assert_eq!(confirm.reply_draft_requests[0].comment_id, comment.id);
    // The stashed pane was restored, not replaced by a fresh one.
    assert!(app.ai_review_return_to.is_none());
    assert_eq!(state.review.comments.len(), 3);
}

#[test]
fn ai_review_batch_fix_combines_marked_findings() {
    let mut app = ai_review_test_app();
    ai_review_over_triage(&mut app, 3);
    app.ai_review_toggle_mark();
    app.ai_review_select_next();
    app.ai_review_select_next();
    app.ai_review_toggle_mark();

    app.ai_review_fix_marked();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR Triage");
    };
    let confirm = state
        .fix_confirm
        .as_ref()
        .expect("batch dialog should open");
    assert_eq!(confirm.batch.as_ref().map(Vec::len), Some(2));
    assert!(confirm.editor.text().contains("finding 0"));
    assert!(confirm.editor.text().contains("finding 2"));
    assert!(!confirm.editor.text().contains("finding 1"));
    assert!(!confirm.editor.text().contains("amf reply-draft"));
}

#[test]
fn ai_review_fix_refuses_posted_and_skipped_findings() {
    let mut app = ai_review_test_app();
    ai_review_over_triage(&mut app, 2);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings[0].published = true;
        state.findings[1].skipped = true;
    }

    app.ai_review_fix_selected();
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    assert!(app.message.as_deref().unwrap().contains("Already posted"));

    app.ai_review_select_next();
    app.ai_review_toggle_mark();
    assert!(app.message.as_deref().unwrap().contains("Skipped"));
    app.ai_review_fix_marked();
    assert!(matches!(app.mode, AppMode::AiReview(_)));
}

#[test]
fn a_local_finding_cannot_be_replied_to_on_github() {
    let mut app = ai_review_test_app();
    ai_review_over_triage(&mut app, 1);
    app.ai_review_fix_selected();
    app.pr_review_cancel_fix();

    app.pr_review_open_reply_pick();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR Triage");
    };
    assert!(state.reply_kind_pick.is_none());
    assert!(app.message.as_deref().unwrap().contains("not posted"));
}

// ── PR picker Review tab ────────────────────────────────────────────────────

fn reviewable(number: u32, title: &str) -> crate::github::ReviewablePr {
    crate::github::ReviewablePr {
        number,
        title: title.to_string(),
        author: "alice".to_string(),
        is_draft: false,
        updated_at: String::new(),
        base_ref: "main".to_string(),
        base_oid: "base".to_string(),
        head_ref: "topic".to_string(),
        head_oid: "head".to_string(),
        is_cross_repository: false,
        head_owner: "acme".to_string(),
    }
}

use crate::app::pr_review::runtime::ReviewListLoaded;

/// The draft key the test loaders report, and the test opener pins.
const TEST_REPO_KEY: &str = "github.com/acme/widgets";

fn two_open_prs(_: &std::path::Path, _: bool) -> ReviewListLoaded {
    ReviewListLoaded {
        prs: Ok(vec![reviewable(7, "first"), reviewable(9, "second")]),
        current_user: Some("me".to_string()),
        repo_key: Some(TEST_REPO_KEY.to_string()),
    }
}

fn gh_not_signed_in(_: &std::path::Path, _: bool) -> ReviewListLoaded {
    ReviewListLoaded {
        prs: Err(anyhow::anyhow!(
            "`gh pr list` failed: To get started with GitHub CLI, please run:  gh auth login"
        )),
        current_user: None,
        repo_key: None,
    }
}

fn press(app: &mut App, code: KeyCode) {
    crate::handlers::handle_key(app, KeyEvent::from(code), 40).unwrap();
}

/// Poll until the Review tab's worker result has been applied.
fn settle_review_list(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.pr_review_work.review_list_pending() {
        app.poll_pr_review_list_bg();
        assert!(
            std::time::Instant::now() < deadline,
            "review list never loaded"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn review_list(app: &App) -> &PrReviewListState {
    match &app.mode {
        AppMode::PrReviewList(state) => state,
        _ => panic!("not on the Review tab"),
    }
}

#[test]
fn tab_switches_from_triage_to_review_and_back_restoring_the_triage_picker() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    enter_pr_picker_for_test(&mut app);
    if let AppMode::PrPicker(picker) = &mut app.mode {
        picker.include_closed = true;
    }

    press(&mut app, KeyCode::Tab);
    // The load is off-thread: the tab opens on "Loading", never blocking.
    assert_eq!(review_list(&app).load, PrReviewListLoad::Loading);
    assert!(review_list(&app).triage.is_some());
    settle_review_list(&mut app);
    match &review_list(&app).load {
        PrReviewListLoad::Loaded(prs) => {
            assert_eq!(prs.iter().map(|p| p.number).collect::<Vec<_>>(), [7, 9]);
        }
        other => panic!("expected a loaded list, got {other:?}"),
    }
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(review_list(&app).selected, 1);

    press(&mut app, KeyCode::BackTab);
    match &app.mode {
        AppMode::PrPicker(picker) => {
            assert!(picker.include_closed, "triage state came back verbatim");
            assert_eq!(
                picker.workdir,
                std::path::PathBuf::from("/tmp/test-workdir")
            );
        }
        _ => panic!("Tab should return to the Triage tab"),
    }
}

#[test]
fn review_list_load_failure_then_retry_then_success() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(gh_not_signed_in);
    enter_pr_picker_for_test(&mut app);
    press(&mut app, KeyCode::Tab);
    settle_review_list(&mut app);
    match &review_list(&app).load {
        PrReviewListLoad::Failed(err) => {
            assert_eq!(err.kind, PrReviewListErrorKind::Auth);
            assert!(err.hint().contains("gh auth login"));
        }
        other => panic!("expected a failed load, got {other:?}"),
    }
    // A failed list must not cache "no gh user": the lookup never ran.
    assert_eq!(app.gh_current_user, None);

    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    press(&mut app, KeyCode::Char('r'));
    assert_eq!(review_list(&app).load, PrReviewListLoad::Loading);
    settle_review_list(&mut app);
    assert!(matches!(
        &review_list(&app).load,
        PrReviewListLoad::Loaded(prs) if prs.len() == 2
    ));
    assert_eq!(review_list(&app).current_user.as_deref(), Some("me"));
}

#[test]
fn a_review_list_result_after_the_tab_closed_is_dropped() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    enter_pr_picker_for_test(&mut app);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(!app.pr_review_work.review_list_pending());
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!app.poll_pr_review_list_bg());
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn a_retry_supersedes_the_load_in_flight() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(gh_not_signed_in);
    enter_pr_picker_for_test(&mut app);
    press(&mut app, KeyCode::Tab);
    let first = review_list(&app).request_id;
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    press(&mut app, KeyCode::Char('r'));
    assert_ne!(review_list(&app).request_id, first);
    settle_review_list(&mut app);
    // Only the retry's result lands, however the two threads were ordered.
    assert!(matches!(
        &review_list(&app).load,
        PrReviewListLoad::Loaded(_)
    ));
}

#[test]
fn dashboard_g_on_a_project_row_opens_the_review_tab_without_a_feature() {
    let mut app = App::new_for_test(
        store_with_feature(crate::project::ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    app.selection = Selection::Project(0);

    app.open_pr_review();

    let state = review_list(&app);
    assert_eq!(state.workdir, app.store.projects[0].repo);
    assert!(state.triage.is_none());
    settle_review_list(&mut app);
    assert!(matches!(
        &review_list(&app).load,
        PrReviewListLoad::Loaded(_)
    ));
}

#[test]
fn review_list_errors_are_classified_by_what_the_user_can_do() {
    let classify = |s: &str| PrReviewListError::classify(s.to_string()).kind;
    assert_eq!(
        classify("Failed to run `gh`."),
        PrReviewListErrorKind::GhMissing
    );
    assert_eq!(
        classify("gh: HTTP 401: Bad credentials"),
        PrReviewListErrorKind::Auth
    );
    assert_eq!(
        classify("`gh pr list` failed: error connecting to api.github.com"),
        PrReviewListErrorKind::Other
    );
}

// ── Opening a PR for review from the Review tab ─────────────────────────────

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A repo whose `teammate-pr` branch stands in for PR #9, checked out on an
/// unrelated feature branch with a dirty tracked file and an untracked one.
fn repo_with_teammate_pr() -> TempDir {
    let repo = TempDir::new().unwrap();
    let dir = repo.path();
    git_out(dir, &["init", "--quiet", "--initial-branch=main"]);
    git_out(dir, &["config", "user.name", "AMF Test"]);
    git_out(dir, &["config", "user.email", "amf@example.com"]);
    std::fs::write(dir.join("shared.rs"), "fn shared() {}\n").unwrap();
    git_out(dir, &["add", "shared.rs"]);
    git_out(dir, &["commit", "--quiet", "-m", "initial"]);
    git_out(dir, &["checkout", "--quiet", "-b", "teammate-pr"]);
    std::fs::write(dir.join("pr_only.rs"), "fn pr() {}\n").unwrap();
    git_out(dir, &["add", "pr_only.rs"]);
    git_out(dir, &["commit", "--quiet", "-m", "pr change"]);
    git_out(dir, &["checkout", "--quiet", "main"]);
    git_out(dir, &["checkout", "--quiet", "-b", "my-feature"]);
    std::fs::write(dir.join("shared.rs"), "local dirt\n").unwrap();
    std::fs::write(dir.join("untracked.rs"), "stray\n").unwrap();
    repo
}

/// Stands in for fetching PR #9: pins the repo's own branches and, like the
/// real fetch, leaves a private review ref behind for close to remove.
fn open_teammate_pr(
    workdir: &std::path::Path,
    pr: &crate::github::ReviewablePr,
) -> anyhow::Result<PrDiffTarget> {
    let head = git_out(workdir, &["rev-parse", "teammate-pr"]);
    let base = git_out(workdir, &["rev-parse", "main"]);
    let merge_base = git_out(workdir, &["merge-base", &base, &head]);
    git_out(workdir, &["update-ref", "refs/amf/review/9/head", &head]);
    let mut pinned = pr.clone();
    pinned.head_oid = head;
    pinned.base_oid = base;
    Ok(PrDiffTarget {
        repo: TEST_REPO_KEY.to_string(),
        pr: pinned,
        merge_base_oid: merge_base,
    })
}

/// [`two_open_prs`], but PR #9's row reports `teammate-pr`'s real head, as
/// `gh` would once the author pushes, so the list can tell a moved PR apart.
fn two_open_prs_at_real_heads(workdir: &std::path::Path, closed: bool) -> ReviewListLoaded {
    let mut loaded = two_open_prs(workdir, closed);
    if let Ok(prs) = &mut loaded.prs {
        for pr in prs.iter_mut().filter(|pr| pr.number == 9) {
            pr.head_oid = git_out(workdir, &["rev-parse", "teammate-pr"]);
        }
    }
    loaded
}

fn refuse_to_open(
    _: &std::path::Path,
    _: &crate::github::ReviewablePr,
) -> anyhow::Result<PrDiffTarget> {
    Err(anyhow::anyhow!(
        "`git fetch` for PR #9 failed: network down"
    ))
}

fn settle_review_open(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.pr_review_work.review_open_pending() {
        app.poll_pr_review_open_bg();
        assert!(std::time::Instant::now() < deadline, "PR never opened");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// An app on the Review tab of `repo`'s project, loaded with PRs #7 and #9
/// and the cursor on #9. The tmux mock has no expectations: any tmux call
/// (a window, a session) fails the test.
fn review_tab_on(repo: &std::path::Path) -> App {
    let mut app = App::new_for_test(
        store_with_repo(repo.to_path_buf(), crate::project::ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    app.selection = Selection::Project(0);
    app.open_pr_review();
    settle_review_list(&mut app);
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(review_list(&app).selected, 1);
    app
}

fn viewer(app: &App) -> &DiffViewerState {
    match &app.mode {
        AppMode::DiffViewer(state) | AppMode::DiffViewerLoading(state) => state,
        _ => panic!("not in the diff viewer"),
    }
}

#[test]
fn enter_opens_the_pr_in_the_review_viewer_and_esc_returns_to_the_same_row() {
    let repo = repo_with_teammate_pr();
    let dir = repo.path();
    let mut app = review_tab_on(dir);
    app.pr_review_work
        .set_review_opener_for_test(open_teammate_pr);
    let features_before = app.store.projects[0].features.len();

    press(&mut app, KeyCode::Enter);
    assert_eq!(
        review_list(&app).opening.as_ref().map(|o| o.number),
        Some(9)
    );
    settle_review_open(&mut app);
    assert!(matches!(app.mode, AppMode::DiffViewerLoading(_)));
    app.complete_diff_viewer_loading();

    let state = viewer(&app);
    assert!(state.review && state.is_pr_review());
    let paths: Vec<&str> = state.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        ["pr_only.rs"],
        "only the PR's files, none of the feature's"
    );
    // No feature, session, or tmux window came into being.
    assert_eq!(app.store.projects[0].features.len(), features_before);
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .all(|f| f.sessions.is_empty())
    );

    press(&mut app, KeyCode::Esc);
    let list = review_list(&app);
    assert_eq!(list.selected, 1, "the cursor is where it was");
    assert!(matches!(&list.load, PrReviewListLoad::Loaded(prs) if prs.len() == 2));
    assert!(list.opening.is_none());

    // Leaving drops the private review refs (off the UI thread).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !git_out(dir, &["for-each-ref", "refs/amf/review/"]).is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "review refs never removed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_pr_review_never_writes_into_the_checkout_and_says_why_it_refuses() {
    let repo = repo_with_teammate_pr();
    let dir = repo.path();
    let status_before = git_out(dir, &["status", "--porcelain=v1", "--untracked-files=all"]);
    let mut app = review_tab_on(dir);
    app.pr_review_work
        .set_review_opener_for_test(open_teammate_pr);
    press(&mut app, KeyCode::Enter);
    settle_review_open(&mut app);
    app.complete_diff_viewer_loading();

    // Local-checkout actions refuse with a reason, and the review stays open.
    // (`q` is not among them: it submits to GitHub instead of finishing into
    // an agent — see the submit tests.)
    for key in ['t', 'X', 'b', 'w', 'A', 'O', 'H', 'I'] {
        app.message = None;
        press(&mut app, KeyCode::Char(key));
        let message = app.message.clone().unwrap_or_default();
        assert!(
            message.starts_with("Not available in a PR review:"),
            "`{key}` gave {message:?}"
        );
        assert!(
            matches!(app.mode, AppMode::DiffViewer(_)),
            "`{key}` left the review"
        );
    }
    // `q` opens the submit dialog rather than finishing, and Esc closes it.
    press(&mut app, KeyCode::Char('q'));
    assert!(app.pr_submit_open());
    press(&mut app, KeyCode::Esc);
    assert!(!app.pr_submit_open());
    assert!(matches!(app.mode, AppMode::DiffViewer(_)));
    for refusal in [
        App::diff_review_open_in_editor as fn(&mut App),
        App::diff_review_apply_suggestion_under_cursor,
    ] {
        app.message = None;
        refusal(&mut app);
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.starts_with("Not available in a PR review:"))
        );
    }

    // Reviewing actions still work, and persist nothing into the checkout.
    press(&mut app, KeyCode::Char('a'));
    assert!(viewer(&app).decisions.contains_key("pr_only.rs"));
    app.persist_review_progress();
    assert!(!dir.join(".claude").exists());
    assert_eq!(
        git_out(dir, &["status", "--porcelain=v1", "--untracked-files=all"]),
        status_before
    );
}

#[test]
fn esc_while_a_pr_is_opening_stops_it_and_keeps_the_tab() {
    let repo = repo_with_teammate_pr();
    let mut app = review_tab_on(repo.path());
    app.pr_review_work
        .set_review_opener_for_test(open_teammate_pr);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);

    assert!(review_list(&app).opening.is_none());
    assert!(!app.pr_review_work.review_open_pending());
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!app.poll_pr_review_open_bg());
    assert!(matches!(app.mode, AppMode::PrReviewList(_)));
}

#[test]
fn a_pr_that_fails_to_open_stays_on_the_tab_with_the_reason() {
    let repo = repo_with_teammate_pr();
    let mut app = review_tab_on(repo.path());
    app.pr_review_work
        .set_review_opener_for_test(refuse_to_open);
    press(&mut app, KeyCode::Enter);
    settle_review_open(&mut app);

    let list = review_list(&app);
    assert!(list.opening.is_none());
    assert!(
        list.open_error
            .as_deref()
            .is_some_and(|e| e.contains("network down"))
    );
}

// ── PR review drafts ────────────────────────────────────────────────────────

/// `review_tab_on`, with a real database attached and the test opener.
fn review_tab_with_db(repo: &std::path::Path, db_file: &NamedTempFile) -> App {
    let mut app = review_tab_on(repo);
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.pr_review_work
        .set_review_opener_for_test(open_teammate_pr);
    // Reload so the badges read the database that is now attached.
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    app
}

fn open_selected_pr(app: &mut App) {
    press(app, KeyCode::Enter);
    settle_review_open(app);
    app.complete_diff_viewer_loading();
    assert!(viewer(app).is_pr_review());
}

/// Comment on the first commentable line of the current file.
fn comment_first_line(app: &mut App, text: &str) {
    press(app, KeyCode::Char('c'));
    press(app, KeyCode::Enter);
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
    press(app, KeyCode::Tab);
    press(app, KeyCode::Esc); // leave the line cursor
}

/// Whether the Review tab's row for PR `number` carries the `↻ updated`
/// badge: a saved draft whose head is not the row's current one.
fn row_shows_updated_badge(app: &App, number: u32) -> bool {
    let list = review_list(app);
    let PrReviewListLoad::Loaded(prs) = &list.load else {
        panic!("the list is not loaded");
    };
    let row = prs
        .iter()
        .find(|pr| pr.number == number)
        .expect("PR listed");
    list.drafts
        .get(&number)
        .is_some_and(|draft| draft.head_oid != row.head_oid)
}

fn saved_draft(app: &App) -> Option<crate::db::pr_review_drafts::PrReviewDraft> {
    app.db
        .as_ref()
        .unwrap()
        .load_pr_review_draft(TEST_REPO_KEY, 9)
        .unwrap()
}

fn line_comment_texts(app: &App) -> Vec<String> {
    let mut texts: Vec<String> = viewer(app)
        .line_comments
        .values()
        .flatten()
        .map(|c| c.text.clone())
        .collect();
    texts.sort();
    texts
}

#[test]
fn a_pr_review_draft_survives_leaving_and_reopening() {
    let repo = repo_with_teammate_pr();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(repo.path(), &db_file);
    open_selected_pr(&mut app);

    comment_first_line(&mut app, "why pr?");
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, AppMode::PrReviewList(_)));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("draft saved"))
    );

    let draft = saved_draft(&app).expect("the draft is in the database");
    assert_eq!(
        draft.head_oid,
        git_out(repo.path(), &["rev-parse", "teammate-pr"])
    );
    assert!(draft.file_fingerprints.contains_key("pr_only.rs"));
    // …and nothing was written into the checkout.
    assert!(!repo.path().join(".claude").exists());

    // The list shows the draft; reopening resumes it.
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    assert_eq!(
        review_list(&app).drafts.get(&9).map(|d| d.comments),
        Some(1)
    );
    open_selected_pr(&mut app);
    assert_eq!(line_comment_texts(&app), ["why pr?"]);
    assert!(viewer(&app).decisions.contains_key("pr_only.rs"));
}

#[test]
fn opening_a_pr_without_reviewing_it_leaves_no_draft() {
    let repo = repo_with_teammate_pr();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(repo.path(), &db_file);
    open_selected_pr(&mut app);
    press(&mut app, KeyCode::Esc);
    assert_eq!(saved_draft(&app), None);
}

#[test]
fn a_moved_pr_keeps_its_draft_comments_but_not_its_verdicts() {
    let repo = repo_with_teammate_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs_at_real_heads);
    open_selected_pr(&mut app);
    comment_first_line(&mut app, "keep me");
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);
    // Before the author pushes, the row and the draft agree: no badge.
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    assert!(!row_shows_updated_badge(&app, 9));

    // The author pushes again (without touching the user's checkout).
    git_out(
        dir,
        &["worktree", "add", "--quiet", "../pr-wt", "teammate-pr"],
    );
    let pr_wt = dir.parent().unwrap().join("pr-wt");
    std::fs::write(pr_wt.join("pr_only.rs"), "fn pr() {}\nfn more() {}\n").unwrap();
    git_out(&pr_wt, &["commit", "--quiet", "-am", "second push"]);
    git_out(
        dir,
        &["worktree", "remove", "--force", pr_wt.to_str().unwrap()],
    );
    // The list's rows now report the new head, so the badge says "updated".
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    assert!(row_shows_updated_badge(&app, 9));

    open_selected_pr(&mut app);
    assert_eq!(line_comment_texts(&app), ["keep me"]);
    assert!(
        viewer(&app).decisions.is_empty(),
        "an approval of older code is not kept"
    );
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("changed since your draft")),
        "{:?}",
        app.message
    );
}

#[test]
fn comments_on_a_file_that_left_the_pr_are_kept_in_the_draft() {
    let repo = repo_with_teammate_pr();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(repo.path(), &db_file);
    // A draft saved when the PR still touched `gone.rs`.
    let draft = crate::db::pr_review_drafts::PrReviewDraft {
        repo_key: TEST_REPO_KEY.to_string(),
        pr_number: 9,
        base_oid: "old-base".to_string(),
        head_oid: "old-head".to_string(),
        merge_base_oid: "old-mb".to_string(),
        status: crate::db::pr_review_drafts::PrReviewDraftStatus::Draft,
        progress: serde_json::json!({
            "line_comments": {"gone.rs": [{
                "location": {"old_line": null, "new_line": 1},
                "text": "about a file the PR dropped"
            }]}
        })
        .to_string(),
        file_fingerprints: Default::default(),
        updated_at: "2026-09-25T00:00:00Z".to_string(),
    };
    app.db
        .as_ref()
        .unwrap()
        .upsert_pr_review_draft(&draft)
        .unwrap();

    open_selected_pr(&mut app);
    assert!(
        viewer(&app).line_comments.is_empty(),
        "not shown on a file not in the diff"
    );
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("1 comment(s) are on files no longer in the PR")),
        "{:?}",
        app.message
    );
    comment_first_line(&mut app, "new one");
    press(&mut app, KeyCode::Esc);

    let saved = saved_draft(&app).unwrap();
    assert_eq!(crate::app::review::draft_comment_count(&saved.progress), 2);
    assert!(saved.progress.contains("about a file the PR dropped"));
}

#[test]
fn reloading_the_review_list_keeps_the_rows_and_the_highlighted_pr() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    enter_pr_picker_for_test(&mut app);
    press(&mut app, KeyCode::Tab);
    settle_review_list(&mut app);
    press(&mut app, KeyCode::Char('j'));

    press(&mut app, KeyCode::Char('r'));
    // The rows stay up (and navigable) while the reload is in flight.
    assert!(review_list(&app).reloading);
    assert!(matches!(
        &review_list(&app).load,
        PrReviewListLoad::Loaded(_)
    ));
    settle_review_list(&mut app);

    assert!(!review_list(&app).reloading);
    assert_eq!(
        review_list(&app).selected,
        1,
        "still on #9, not back to the top"
    );
}

#[test]
fn leaving_a_pr_review_says_so_when_the_draft_could_not_be_saved() {
    let repo = repo_with_teammate_pr();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(repo.path(), &db_file);
    open_selected_pr(&mut app);
    // Break the table out from under AMF with a second connection.
    rusqlite::Connection::open(db_file.path())
        .unwrap()
        .execute_batch("DROP TABLE pr_review_drafts;")
        .unwrap();

    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);

    let message = app.message.clone().unwrap_or_default();
    assert!(message.contains("draft NOT saved"), "{message}");
    assert!(!message.contains("draft saved;"), "{message}");
}

// ── PR updates: what a new push does to a saved draft ───────────────────────

/// PR #9 (`teammate-pr`) touching two files: `edit.rs`, whose added lines the
/// author will later replace, and `keep.rs`, which the author won't touch
/// again. The checkout sits on an unrelated branch.
fn repo_with_two_file_pr() -> TempDir {
    let repo = TempDir::new().unwrap();
    let dir = repo.path();
    git_out(dir, &["init", "--quiet", "--initial-branch=main"]);
    git_out(dir, &["config", "user.name", "AMF Test"]);
    git_out(dir, &["config", "user.email", "amf@example.com"]);
    std::fs::write(dir.join("edit.rs"), "fn edit() {}\n").unwrap();
    std::fs::write(dir.join("keep.rs"), "fn keep() {}\n").unwrap();
    git_out(dir, &["add", "."]);
    git_out(dir, &["commit", "--quiet", "-m", "initial"]);
    git_out(dir, &["checkout", "--quiet", "-b", "teammate-pr"]);
    std::fs::write(
        dir.join("edit.rs"),
        "fn edit() {}\nfn old_a() {}\nfn old_b() {}\n",
    )
    .unwrap();
    std::fs::write(dir.join("keep.rs"), "fn keep() {}\nfn kept_line() {}\n").unwrap();
    git_out(dir, &["commit", "--quiet", "-am", "pr v1"]);
    git_out(dir, &["checkout", "--quiet", "main"]);
    git_out(dir, &["checkout", "--quiet", "-b", "my-feature"]);
    repo
}

/// The author pushes a new commit to the PR branch, via a scratch worktree
/// so the user's checkout is never touched.
fn push_to_pr(dir: &std::path::Path, path: &str, content: &str) {
    let wt = dir.parent().unwrap().join(format!(
        "pr-push-{}",
        dir.file_name().unwrap().to_string_lossy()
    ));
    git_out(
        dir,
        &[
            "worktree",
            "add",
            "--quiet",
            wt.to_str().unwrap(),
            "teammate-pr",
        ],
    );
    std::fs::write(wt.join(path), content).unwrap();
    git_out(&wt, &["commit", "--quiet", "-am", "pr push"]);
    git_out(
        dir,
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    );
}

fn select_file(app: &mut App, path: &str) {
    let AppMode::DiffViewer(state) = &mut app.mode else {
        panic!("not in the viewer");
    };
    state.selected_file = state.files.iter().position(|f| f.path == path).unwrap();
    state.on_file_changed();
}

/// Comment (with the real keys) on the current file's line containing `needle`.
fn comment_on_line_containing(app: &mut App, needle: &str, text: &str) {
    let AppMode::DiffViewer(state) = &mut app.mode else {
        panic!("not in the viewer");
    };
    let file = &state.files[state.selected_file];
    let index = file
        .addressable_line_texts()
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?}"));
    state.comment_cursor = Some(index);
    press(app, KeyCode::Enter);
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
    press(app, KeyCode::Tab);
    press(app, KeyCode::Esc); // leave the line cursor
}

fn comment<'a>(app: &'a App, path: &str, text: &str) -> &'a LineComment {
    viewer(app).line_comments[path]
        .iter()
        .find(|c| c.text == text)
        .unwrap_or_else(|| panic!("no comment {text:?} on {path}"))
}

#[test]
fn a_pr_update_flags_only_changed_files_and_marks_comments_on_removed_code_outdated() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);

    // Review v1: approve both files, comment on code in each.
    select_file(&mut app, "edit.rs");
    comment_on_line_containing(&mut app, "old_a", "rename this");
    select_file(&mut app, "keep.rs");
    comment_on_line_containing(&mut app, "kept_line", "nice");
    for path in ["edit.rs", "keep.rs"] {
        select_file(&mut app, path);
        press(&mut app, KeyCode::Char('a'));
    }
    assert_eq!(viewer(&app).decisions.len(), 2);
    press(&mut app, KeyCode::Esc);

    // The author replaces the lines the edit.rs comment was on.
    push_to_pr(dir, "edit.rs", "fn edit() {}\nfn new_c() {}\n");
    open_selected_pr(&mut app);
    let state = viewer(&app);

    // Only the changed file is flagged, filtered to, and loses its verdict.
    assert_eq!(
        state.changed_since_last,
        std::collections::HashSet::from(["edit.rs".to_string()])
    );
    assert!(state.has_prior_review);
    assert_eq!(state.file_filter, FileFilter::Changed);
    assert_eq!(state.files[state.selected_file].path, "edit.rs");
    assert!(
        !state.decisions.contains_key("edit.rs"),
        "changed code is re-reviewed"
    );
    assert_eq!(
        state.decisions.get("keep.rs"),
        Some(&ReviewDecision::Approve)
    );

    // Both comments are kept: the one on removed code is outdated, the one on
    // untouched code still sits on its line.
    assert!(comment(&app, "edit.rs", "rename this").anchor_lost);
    let kept = comment(&app, "keep.rs", "nice");
    assert!(!kept.anchor_lost);
    let keep_file = state.files.iter().find(|f| f.path == "keep.rs").unwrap();
    let kept_index = keep_file
        .addressable_lines()
        .iter()
        .position(|loc| *loc == kept.location)
        .unwrap();
    assert!(keep_file.addressable_line_texts()[kept_index].contains("kept_line"));

    let message = app.message.clone().unwrap_or_default();
    assert!(message.contains("1 of 2 file(s) changed"), "{message}");
    assert!(
        message.contains("1 verdict(s) on them cleared"),
        "{message}"
    );
    assert!(message.contains("1 comment(s) are outdated"), "{message}");

    // The outdated comment survives the next save too.
    press(&mut app, KeyCode::Esc);
    let saved = saved_draft(&app).unwrap();
    assert!(saved.progress.contains("rename this"));
    assert_eq!(
        saved.head_oid,
        git_out(dir, &["rev-parse", "teammate-pr"]),
        "the draft now describes the revision just reviewed"
    );
}

#[test]
fn a_new_push_that_changes_no_file_keeps_every_verdict() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);
    for path in ["edit.rs", "keep.rs"] {
        select_file(&mut app, path);
        press(&mut app, KeyCode::Char('a'));
    }
    press(&mut app, KeyCode::Esc);

    // A new, empty commit: the head moves, but no file's diff changes.
    let empty = git_out(
        dir,
        &[
            "commit-tree",
            "-m",
            "empty push",
            "-p",
            "teammate-pr",
            "teammate-pr^{tree}",
        ],
    );
    git_out(dir, &["update-ref", "refs/heads/teammate-pr", &empty]);

    open_selected_pr(&mut app);
    let state = viewer(&app);
    assert!(state.changed_since_last.is_empty());
    assert_eq!(state.decisions.len(), 2);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("none of its files' changes differ")),
        "{:?}",
        app.message
    );
}

#[test]
fn the_pr_notes_panel_shows_changes_and_outdated_comments_and_the_footer_only_usable_keys() {
    use ratatui::{Terminal, backend::TestBackend};

    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");
    comment_on_line_containing(&mut app, "old_a", "rename this");
    press(&mut app, KeyCode::Esc);
    push_to_pr(dir, "edit.rs", "fn edit() {}\nfn new_c() {}\n");
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");
    press(&mut app, KeyCode::Char('e')); // expand the notes panel

    let mut terminal = Terminal::new(TestBackend::new(160, 50)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("PR Review Notes"), "{screen}");
    assert!(screen.contains("Changed since your draft"), "{screen}");
    assert!(screen.contains("Outdated comments"), "{screen}");
    assert!(screen.contains("rename this"), "{screen}");
    assert!(
        !screen.contains("review-notes.md"),
        "no feature-notes placeholder"
    );

    // The footer offers only what a PR review can do.
    press(&mut app, KeyCode::Char('e'));
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    for refused in [
        "$EDITOR",
        "gen walkthrough",
        "AI review",
        " overview",
        " history",
        "since last review",
        "base ref",
        "target:",
        "→ finish",
    ] {
        assert!(!screen.contains(refused), "footer still offers {refused:?}");
    }
    assert!(screen.contains("pause (draft saved)"), "{screen}");
}

// ── Submitting a PR review ──────────────────────────────────────────────────

use crate::app::pr_review::runtime::{PrPostOutcome, PrPostRequest};

/// What the stub posters were asked to post, by the test's repo path, so
/// parallel tests never read each other's requests.
fn posted_requests() -> &'static std::sync::Mutex<HashMap<std::path::PathBuf, Vec<PrPostRequest>>> {
    static POSTED: std::sync::OnceLock<
        std::sync::Mutex<HashMap<std::path::PathBuf, Vec<PrPostRequest>>>,
    > = std::sync::OnceLock::new();
    POSTED.get_or_init(Default::default)
}

fn record(request: &PrPostRequest) {
    posted_requests()
        .lock()
        .unwrap()
        .entry(request.workdir.clone())
        .or_default()
        .push(request.clone());
}

fn requests_for(dir: &std::path::Path) -> Vec<PrPostRequest> {
    posted_requests()
        .lock()
        .unwrap()
        .get(dir)
        .cloned()
        .unwrap_or_default()
}

fn github_accepts(request: &PrPostRequest) -> PrPostOutcome {
    record(request);
    PrPostOutcome::Posted {
        file_comment_failures: vec![],
    }
}

fn github_is_down(request: &PrPostRequest) -> PrPostOutcome {
    record(request);
    PrPostOutcome::Failed("`gh api` (create review) failed: HTTP 502".to_string())
}

fn pr_has_moved(request: &PrPostRequest) -> PrPostOutcome {
    record(request);
    PrPostOutcome::HeadMoved {
        current_head: "feedface00000000000000000000000000000000".to_string(),
    }
}

fn settle_review_post(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.pr_review_work.review_post_pending() {
        app.poll_pr_review_post_bg();
        assert!(std::time::Instant::now() < deadline, "post never answered");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn submit_status(app: &App) -> PrSubmitStatus {
    viewer(app)
        .pr_submit
        .as_ref()
        .expect("the submit dialog is open")
        .status
        .clone()
}

/// Type into the open general-feedback (summary) editor and save it.
fn write_summary(app: &mut App, text: &str) {
    press(app, KeyCode::Char('e'));
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
    press(app, KeyCode::Tab);
}

/// The two-file PR, opened for review, with a single-line comment on `edit.rs`.
fn reviewed_two_file_pr(
    repo: &std::path::Path,
    db_file: &NamedTempFile,
    poster: crate::app::pr_review::runtime::ReviewPoster,
) -> App {
    let mut app = review_tab_with_db(repo, db_file);
    app.pr_review_work.set_review_poster_for_test(poster);
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");
    comment_on_line_containing(&mut app, "old_a", "rename this");
    app
}

#[test]
fn each_review_event_posts_with_its_github_name_pinned_to_the_reviewed_head() {
    for (steps, expected) in [(0, "COMMENT"), (1, "APPROVE"), (2, "REQUEST_CHANGES")] {
        let repo = repo_with_two_file_pr();
        let dir = repo.path();
        let db_file = NamedTempFile::new().unwrap();
        let mut app = reviewed_two_file_pr(dir, &db_file, github_accepts);

        press(&mut app, KeyCode::Char('q'));
        assert_eq!(submit_status(&app), PrSubmitStatus::Ready);
        for _ in 0..steps {
            press(&mut app, KeyCode::Right);
        }
        write_summary(&mut app, "overall fine");
        press(&mut app, KeyCode::Enter);
        settle_review_post(&mut app);

        let requests = requests_for(dir);
        assert_eq!(requests.len(), 1, "{expected}");
        let request = &requests[0];
        assert_eq!(request.event.api_name(), expected);
        assert_eq!(request.pr.number, 9);
        assert_eq!(
            (request.pr.owner.as_str(), request.pr.repo.as_str()),
            ("acme", "widgets")
        );
        assert_eq!(
            request.pr.head_sha,
            git_out(dir, &["rev-parse", "teammate-pr"])
        );
        assert_eq!(request.body, "overall fine");
        assert_eq!(request.comments.len(), 1);
        assert!(request.comments[0].body.contains("rename this"));

        // Posted: back on the list, the draft marked posted, no badge.
        assert!(matches!(app.mode, AppMode::PrReviewList(_)), "{expected}");
        assert!(!review_list(&app).drafts.contains_key(&9));
        assert_eq!(
            saved_draft(&app).unwrap().status,
            crate::db::pr_review_drafts::PrReviewDraftStatus::Posted
        );
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.contains("Posted a") && m.contains("1 inline comment")),
            "{:?}",
            app.message
        );
    }
}

#[test]
fn a_range_comment_and_a_suggestion_post_as_github_expects() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    app.pr_review_work
        .set_review_poster_for_test(github_accepts);
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");

    // A range over old_a..old_b.
    {
        let AppMode::DiffViewer(state) = &mut app.mode else {
            unreachable!()
        };
        let file = &state.files[state.selected_file];
        let start = file
            .addressable_line_texts()
            .iter()
            .position(|l| l.contains("old_a"))
            .unwrap();
        state.comment_cursor = Some(start);
    }
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Enter);
    for ch in "both of these".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Esc);

    // A suggestion on keep.rs's added line.
    select_file(&mut app, "keep.rs");
    {
        let AppMode::DiffViewer(state) = &mut app.mode else {
            unreachable!()
        };
        let file = &state.files[state.selected_file];
        state.comment_cursor = file
            .addressable_line_texts()
            .iter()
            .position(|l| l.contains("kept_line"));
    }
    press(&mut app, KeyCode::Char('S'));
    for ch in "_v2".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    settle_review_post(&mut app);

    let request = requests_for(dir).pop().expect("posted");
    let range = request
        .comments
        .iter()
        .find(|c| c.path == "edit.rs")
        .unwrap();
    assert_eq!(
        (range.start_line, range.line, range.side, range.start_side),
        (Some(2), 3, "RIGHT", Some("RIGHT"))
    );
    assert!(range.body.contains("both of these"));
    let suggestion = request
        .comments
        .iter()
        .find(|c| c.path == "keep.rs")
        .unwrap();
    assert_eq!(suggestion.line, 2);
    assert!(
        suggestion.body.contains("```suggestion\n"),
        "{}",
        suggestion.body
    );
    assert!(suggestion.body.contains("_v2"), "{}", suggestion.body);
}

#[test]
fn a_failed_post_keeps_the_draft_and_a_retry_posts_it() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, github_is_down);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    settle_review_post(&mut app);

    match submit_status(&app) {
        PrSubmitStatus::Failed(err) => assert!(err.contains("HTTP 502"), "{err}"),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert!(
        matches!(app.mode, AppMode::DiffViewer(_)),
        "still reviewing"
    );
    let draft = saved_draft(&app).expect("saved before posting");
    assert_eq!(
        draft.status,
        crate::db::pr_review_drafts::PrReviewDraftStatus::Draft
    );

    app.pr_review_work
        .set_review_poster_for_test(github_accepts);
    press(&mut app, KeyCode::Enter); // retry
    settle_review_post(&mut app);

    assert_eq!(requests_for(dir).len(), 2);
    assert!(matches!(app.mode, AppMode::PrReviewList(_)));
    assert_eq!(
        saved_draft(&app).unwrap().status,
        crate::db::pr_review_drafts::PrReviewDraftStatus::Posted
    );

    // Reopening starts a new review rather than re-posting the old one.
    open_selected_pr(&mut app);
    assert!(viewer(&app).line_comments.is_empty());
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("already posted")),
        "{:?}",
        app.message
    );
}

#[test]
fn a_pr_that_moved_blocks_the_post_and_o_reopens_it_at_the_new_head() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, pr_has_moved);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    settle_review_post(&mut app);
    assert!(matches!(
        submit_status(&app),
        PrSubmitStatus::HeadMoved { current_head } if current_head.starts_with("feedface")
    ));

    // Enter doesn't try again: the only way on is to look at the new code.
    press(&mut app, KeyCode::Enter);
    assert!(!app.pr_review_work.review_post_pending());
    assert_eq!(requests_for(dir).len(), 1);

    press(&mut app, KeyCode::Char('o'));
    settle_review_open(&mut app);
    app.complete_diff_viewer_loading();
    assert!(viewer(&app).is_pr_review());
    assert!(viewer(&app).pr_submit.is_none());
    assert_eq!(line_comment_texts(&app), ["rename this"], "draft kept");
}

#[test]
fn approve_and_request_changes_are_refused_on_your_own_pr_before_posting() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, github_accepts);
    app.gh_current_user = Some(Some("Alice".to_string())); // the PR's author

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Right); // Approve
    press(&mut app, KeyCode::Enter);

    match submit_status(&app) {
        PrSubmitStatus::Failed(err) => assert!(err.contains("your own PR"), "{err}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(!app.pr_review_work.review_post_pending());
    assert!(requests_for(dir).is_empty());
}

#[test]
fn an_empty_review_is_refused_before_posting() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    app.pr_review_work
        .set_review_poster_for_test(github_accepts);
    open_selected_pr(&mut app);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        submit_status(&app),
        PrSubmitStatus::Failed(err) if err.contains("Nothing to post")
    ));

    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right); // Request changes
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        submit_status(&app),
        PrSubmitStatus::Failed(err) if err.contains("needs a summary")
    ));
    assert!(requests_for(dir).is_empty());
}

#[test]
fn esc_cannot_abandon_a_post_in_flight() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, github_accepts);
    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);

    // The answer isn't polled yet, so the post is still in flight.
    press(&mut app, KeyCode::Esc);
    assert!(matches!(
        submit_status(&app),
        PrSubmitStatus::Posting { .. }
    ));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("wait for its answer"))
    );
    settle_review_post(&mut app);
    assert!(matches!(app.mode, AppMode::PrReviewList(_)));
}

#[test]
fn comments_that_cannot_go_on_the_diff_are_posted_in_the_summary() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, github_accepts);
    press(&mut app, KeyCode::Esc);
    // The lines the comment was on are replaced: it becomes outdated.
    push_to_pr(dir, "edit.rs", "fn edit() {}\nfn new_c() {}\n");
    open_selected_pr(&mut app);
    assert!(comment(&app, "edit.rs", "rename this").anchor_lost);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    settle_review_post(&mut app);

    let request = requests_for(dir).pop().expect("posted");
    assert!(
        request.comments.is_empty(),
        "nothing inline: its code is gone"
    );
    assert!(
        request
            .body
            .contains("Comments that couldn't be placed on the diff"),
        "{}",
        request.body
    );
    assert!(request.body.contains("`edit.rs`"), "{}", request.body);
    assert!(
        request.body.contains("code no longer in the PR"),
        "{}",
        request.body
    );
    assert!(request.body.contains("rename this"), "{}", request.body);
}

// ── Acceptance: a real PR on GitHub (opt-in) ────────────────────────────────

/// Everything a PR review must leave byte-identical in a checkout.
fn checkout_snapshot(dir: &std::path::Path) -> Vec<String> {
    vec![
        git_out(dir, &["rev-parse", "--symbolic-full-name", "HEAD"]),
        git_out(dir, &["rev-parse", "HEAD"]),
        git_out(dir, &["status", "--porcelain=v1", "--untracked-files=all"]),
        git_out(dir, &["ls-files", "--stage"]),
        git_out(dir, &["diff", "--cached"]),
        git_out(dir, &["diff"]),
        git_out(dir, &["stash", "list", "--format=%H %gs"]),
        git_out(
            dir,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
                "refs/remotes",
            ],
        ),
    ]
}

fn gh_json(args: &[&str]) -> serde_json::Value {
    let output = std::process::Command::new("gh")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gh {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Poll a background job until `pending` clears, allowing for the network.
fn settle_slowly(app: &mut App, pending: fn(&App) -> bool, poll: fn(&mut App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    while pending(app) {
        poll(app);
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting on GitHub"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Dirt of every kind in `dir`: staged, unstaged, untracked, and a stash.
fn make_dirty(dir: &std::path::Path) {
    git_out(dir, &["config", "user.name", "AMF Acceptance"]);
    git_out(dir, &["config", "user.email", "amf@example.com"]);
    let readme = dir.join("README.md");
    let original = std::fs::read_to_string(&readme).unwrap();
    std::fs::write(&readme, format!("{original}\nstashed\n")).unwrap();
    git_out(dir, &["stash", "push", "--quiet", "-m", "keep me"]);
    std::fs::write(&readme, format!("{original}\nunstaged edit\n")).unwrap();
    let changelog = dir.join("CHANGELOG.md");
    let log = std::fs::read_to_string(&changelog).unwrap();
    std::fs::write(&changelog, format!("{log}\nstaged edit\n")).unwrap();
    git_out(dir, &["add", "CHANGELOG.md"]);
    std::fs::write(dir.join("local-only-untracked.txt"), "stray\n").unwrap();
}

/// Task 11: review a real PR while the project's checkout and a feature
/// worktree are dirty. Talks to GitHub — it pushes a commit to the PR's
/// branch and posts a Comment review — so it runs only when asked:
///
/// ```text
/// AMF_PR_REVIEW_ACCEPTANCE_PR=654 cargo test -j 2 --lib -- --ignored acceptance_
/// ```
///
/// Needs `gh` signed in with push access to the PR's branch.
#[test]
#[ignore = "talks to GitHub; set AMF_PR_REVIEW_ACCEPTANCE_PR and run with --ignored"]
fn acceptance_review_a_real_pr_while_a_local_feature_is_dirty() {
    let Ok(number) = std::env::var("AMF_PR_REVIEW_ACCEPTANCE_PR") else {
        eprintln!("AMF_PR_REVIEW_ACCEPTANCE_PR not set; skipping");
        return;
    };
    let number: u32 = number.parse().expect("a PR number");
    let repo_url = std::env::var("AMF_PR_REVIEW_ACCEPTANCE_REPO")
        .unwrap_or_else(|_| "https://github.com/eldridgerdev/agent-mainframe".to_string());
    let repo_slug = repo_url
        .trim_start_matches("https://github.com/")
        .to_string();

    // A project clone with a dirty checkout and a dirty feature worktree.
    let root = TempDir::new().unwrap();
    let clone = root.path().join("project");
    git_out(root.path(), &["clone", "--quiet", &repo_url, "project"]);
    let feature = root.path().join("feature");
    git_out(
        &clone,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "my-feature",
            feature.to_str().unwrap(),
        ],
    );
    make_dirty(&clone);
    make_dirty(&feature);
    let clone_before = checkout_snapshot(&clone);
    let feature_before = checkout_snapshot(&feature);

    // What GitHub says the PR is.
    let pr = gh_json(&[
        "pr",
        "view",
        &number.to_string(),
        "-R",
        &repo_slug,
        "--json",
        "title,author,headRefName,headRefOid,isDraft,isCrossRepository,headRepositoryOwner,files",
    ]);
    let pr_files: std::collections::BTreeSet<String> = pr["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();

    // ── The Review tab lists it, correctly, without blocking. ──
    let db_file = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(clone.clone(), crate::project::ProjectStatus::Active),
        Box::new(MockTmuxOps::new()), // any tmux call fails the test
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.selection = Selection::Project(0);
    let features_before = app.store.projects[0].features.len();
    app.open_pr_review();
    assert_eq!(
        review_list(&app).load,
        PrReviewListLoad::Loading,
        "never blocks"
    );
    settle_slowly(
        &mut app,
        |a| a.pr_review_work.review_list_pending(),
        App::poll_pr_review_list_bg,
    );
    let row = match &review_list(&app).load {
        PrReviewListLoad::Loaded(prs) => prs
            .iter()
            .position(|p| p.number == number)
            .expect("the PR is listed"),
        other => panic!("list failed: {other:?}"),
    };
    {
        let PrReviewListLoad::Loaded(prs) = &review_list(&app).load else {
            unreachable!()
        };
        let listed = &prs[row];
        assert_eq!(listed.title, pr["title"].as_str().unwrap());
        assert_eq!(listed.author, pr["author"]["login"].as_str().unwrap());
        assert_eq!(listed.is_draft, pr["isDraft"].as_bool().unwrap());
        let branch = pr["headRefName"].as_str().unwrap();
        let expected_label = if pr["isCrossRepository"].as_bool().unwrap() {
            format!(
                "{}:{branch}",
                pr["headRepositoryOwner"]["login"].as_str().unwrap()
            )
        } else {
            branch.to_string()
        };
        assert_eq!(listed.branch_label(), expected_label);
    }
    if let AppMode::PrReviewList(state) = &mut app.mode {
        state.selected = row;
    }

    // ── Enter shows exactly the PR's merge-base → head files. ──
    let open = |app: &mut App| {
        press(app, KeyCode::Enter);
        settle_slowly(
            app,
            |a| a.pr_review_work.review_open_pending(),
            App::poll_pr_review_open_bg,
        );
        if let AppMode::PrReviewList(list) = &app.mode {
            panic!("could not open: {:?}", list.open_error);
        }
        app.complete_diff_viewer_loading();
        assert_eq!(viewer(app).error, None);
    };
    open(&mut app);
    let shown: std::collections::BTreeSet<String> =
        viewer(&app).files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(shown, pr_files, "exactly the PR's files");
    assert!(!shown.contains("local-only-untracked.txt"));

    // ── Comment, approve, pause, return, reopen: all restored. ──
    let (commented, comment_line) = viewer(&app)
        .files
        .iter()
        .find_map(|file| {
            file.hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .find(|line| {
                    line.kind == crate::diff::DiffLineKind::Added
                        && line.text.trim_start_matches('+').trim().len() > 4
                })
                .map(|line| {
                    (
                        file.path.clone(),
                        line.text.trim_start_matches('+').to_string(),
                    )
                })
        })
        .expect("a file with an added line");
    select_file(&mut app, &commented);
    comment_on_line_containing(&mut app, &comment_line, "AMF acceptance: inline note");
    for path in pr_files.iter() {
        select_file(&mut app, path);
        press(&mut app, KeyCode::Char('a'));
    }
    select_file(&mut app, &commented);
    let position = viewer(&app).selected_file;
    press(&mut app, KeyCode::Esc);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("draft saved"))
    );
    open(&mut app);
    assert_eq!(line_comment_texts(&app), ["AMF acceptance: inline note"]);
    assert_eq!(viewer(&app).decisions.len(), pr_files.len());
    assert_eq!(viewer(&app).selected_file, position, "position restored");

    // ── The user's checkouts are untouched; no agent, feature, or session. ──
    assert_eq!(checkout_snapshot(&clone), clone_before);
    assert_eq!(checkout_snapshot(&feature), feature_before);
    assert_eq!(app.store.projects[0].features.len(), features_before);
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .all(|f| f.sessions.is_empty())
    );

    // ── A new commit on the PR flags only the file it changed. ──
    press(&mut app, KeyCode::Esc);
    let touched = pr_files
        .iter()
        .find(|path| **path != commented && !path.ends_with(".rs"))
        .or_else(|| pr_files.iter().find(|path| **path != commented))
        .expect("a second file in the PR")
        .clone();
    let pusher = root.path().join("pusher");
    git_out(root.path(), &["clone", "--quiet", &repo_url, "pusher"]);
    git_out(&pusher, &["config", "user.name", "AMF Acceptance"]);
    git_out(&pusher, &["config", "user.email", "amf@example.com"]);
    let branch = pr["headRefName"].as_str().unwrap();
    git_out(&pusher, &["checkout", "--quiet", branch]);
    let touched_path = pusher.join(&touched);
    let before = std::fs::read_to_string(&touched_path).unwrap_or_default();
    std::fs::write(
        &touched_path,
        format!(
            "{before}\nAMF acceptance push {}\n",
            chrono::Utc::now().to_rfc3339()
        ),
    )
    .unwrap();
    git_out(
        &pusher,
        &["commit", "--quiet", "-am", "test: AMF acceptance push"],
    );
    git_out(&pusher, &["push", "--quiet", "origin", branch]);
    let new_head = git_out(&pusher, &["rev-parse", "HEAD"]);

    press(&mut app, KeyCode::Char('r'));
    settle_slowly(
        &mut app,
        |a| a.pr_review_work.review_list_pending(),
        App::poll_pr_review_list_bg,
    );
    open(&mut app);
    let state = viewer(&app);
    let DiffScope::PullRequest(target) = &state.scope else {
        unreachable!()
    };
    assert_eq!(target.pr.head_oid, new_head, "reviewing the new head");
    assert_eq!(
        state.changed_since_last,
        std::collections::HashSet::from([touched.clone()]),
        "only the pushed file is flagged"
    );
    assert!(
        !state.decisions.contains_key(&touched),
        "its approval is cleared"
    );
    assert_eq!(
        state.decisions.len(),
        pr_files.len() - 1,
        "the rest keep theirs"
    );
    assert_eq!(line_comment_texts(&app), ["AMF acceptance: inline note"]);

    // ── Posting a Comment pins the inline comment to the reviewed head. ──
    press(&mut app, KeyCode::Char('q'));
    let marker = format!("[AMF acceptance test {}]", chrono::Utc::now().to_rfc3339());
    write_summary(&mut app, &marker);
    press(&mut app, KeyCode::Enter);
    settle_slowly(
        &mut app,
        |a| a.pr_review_work.review_post_pending(),
        App::poll_pr_review_post_bg,
    );
    assert!(
        matches!(app.mode, AppMode::PrReviewList(_)),
        "posted and back on the list: {:?}",
        app.message
    );
    let reviews = gh_json(&["api", &format!("repos/{repo_slug}/pulls/{number}/reviews")]);
    let review = reviews
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|r| r["body"].as_str().is_some_and(|b| b.contains(&marker)))
        .expect("the review is on GitHub");
    assert_eq!(review["state"], "COMMENTED");
    assert_eq!(review["commit_id"].as_str().unwrap(), new_head);
    let inline = gh_json(&[
        "api",
        &format!(
            "repos/{repo_slug}/pulls/{number}/reviews/{}/comments",
            review["id"]
        ),
    ]);
    let inline = inline.as_array().unwrap();
    assert_eq!(inline.len(), 1);
    assert_eq!(inline[0]["path"].as_str().unwrap(), commented);
    assert!(
        inline[0]["body"]
            .as_str()
            .unwrap()
            .contains("AMF acceptance: inline note")
    );

    // ── The review's refs are gone; the checkouts are still untouched. ──
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !git_out(&clone, &["for-each-ref", "refs/amf/review/"]).is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "review refs left behind"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(checkout_snapshot(&clone), clone_before);
    assert_eq!(checkout_snapshot(&feature), feature_before);
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .all(|f| f.sessions.is_empty())
    );
}

#[test]
fn tui_reply_and_resolve_write_through_the_shared_github_boundary() {
    use crate::app::pr_review::github_access::fake::FakeGithub;
    let fake = FakeGithub::default();
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_github_for_test(std::sync::Arc::new(fake.clone()));
    enter_pr_review(&mut app, 2);

    // A failed write keeps the TUI's existing behaviour: back to the
    // dashboard with the error, nothing recorded as posted.
    fake.state().fail_writes = true;
    app.pr_review_open_reply_not_needed();
    if let AppMode::PrReview(state) = &mut app.mode {
        state.reply.as_mut().unwrap().editor =
            crate::editor::TextEditor::new("Already handled upstream.".into());
    }
    app.pr_review_post_reply().unwrap();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(fake.state().writes.is_empty());

    fake.state().fail_writes = false;
    enter_pr_review(&mut app, 2);
    app.pr_review_open_reply_not_needed();
    if let AppMode::PrReview(state) = &mut app.mode {
        state.reply.as_mut().unwrap().editor =
            crate::editor::TextEditor::new("Already handled upstream.".into());
        state.review.comments[1].thread_id = Some("THREAD".into());
    }
    app.pr_review_post_reply().unwrap();
    assert_eq!(
        fake.state().writes,
        ["reply 1: Already handled upstream.\n\n— posted via AMF"]
    );
    app.pr_review_select_next();
    app.pr_review_toggle_resolve();
    assert_eq!(fake.state().writes.len(), 2);
    assert_eq!(fake.state().writes[1], "resolve THREAD: true");
    let AppMode::PrReview(state) = &app.mode else {
        panic!("triage stays open after a successful write");
    };
    assert!(state.review.comments[1].is_resolved);
}

#[test]
fn tui_comment_fetch_failure_still_returns_to_the_dashboard() {
    use crate::app::pr_review::github_access::fake::FakeGithub;
    let fake = FakeGithub::default();
    fake.state().fail_fetch = Some("HTTP 502".into());
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_github_for_test(std::sync::Arc::new(fake.clone()));
    let pr = fake.pr_ref(9);
    app.start_pr_review_fetch(std::path::PathBuf::from("/tmp/wd"), pr);
    assert!(app.pr_review_loading());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !app.poll_pr_review_bg() {
        assert!(std::time::Instant::now() < until, "fetch never finished");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(fake.state().fetches, 1);
}

mod navigation;

mod investigations;

mod fix_handoff;

mod review_memory;

mod session_activity;

mod fix_batch;

mod fix_editor;

mod review_sort;

mod replies;

mod memory_workflows;

mod ai_review_picker;
