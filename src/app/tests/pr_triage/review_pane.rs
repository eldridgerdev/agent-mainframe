use super::*;

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
