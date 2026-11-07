use super::*;

#[test]
fn github_repository_resolution_uses_the_selected_project_repo() {
    let repo = TempDir::new().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .args(["remote", "add", "origin", "git@github.com:Acme/Widget.git"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success()
    );

    let app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let repository = app.github_repository_for_project(0).unwrap();
    assert_eq!(repository.canonical(), "github.com/acme/widget");
}

#[test]
fn pr_review_navigation_clamps_at_both_ends() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);

    // Already at top: prev is a no-op.
    app.pr_review_select_prev();
    assert_eq!(pr_review_selected(&app), 0);

    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 1);
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 2);

    // At the bottom: next is a no-op (no wrap).
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 2);
}

#[test]
fn pr_review_close_returns_to_dashboard() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.close_pr_review();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(!app.pr_review_work.fetch_pending());
}

#[test]
fn pr_review_toggle_to_session_requires_existing_session() {
    // `store_with_feature` has no sessions, so there's no dedicated triage
    // session to jump to yet — the toggle must hint rather than create one
    // as a side effect of a quick peek.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_toggle_to_session().unwrap();

    assert!(matches!(app.mode, AppMode::PrReview(_)));
    assert!(app.pr_review_return.is_none());
}

#[test]
fn pr_review_toggle_to_session_jumps_and_stashes_state() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
        state.detail_scroll = 3;
    }

    app.pr_review_toggle_to_session().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session, "amf-my-feat");
            assert_eq!(view.session_label, "PR Triage");
        }
        other => panic!("expected Viewing, got {:?}", std::mem::discriminant(other)),
    }
    let stash = app
        .pr_review_return
        .as_ref()
        .expect("pane state should be stashed");
    assert_eq!(stash.session, "amf-my-feat");
    assert_eq!(stash.state.selected, 1);
    assert_eq!(stash.state.detail_scroll, 3);
}

#[test]
fn pr_review_toggle_to_session_uses_the_selected_custom_name() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());
    store.projects[0].features[0]
        .add_session_named(SessionKind::Codex, "PR 321 security".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.dedicated_session_label = "PR 321 security".to_string();
        state.fix_target_picked = true;
    }

    app.pr_review_toggle_to_session().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session_label, "PR 321 security");
            assert_eq!(view.session_kind, SessionKind::Codex);
        }
        other => panic!("expected Viewing, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_return_to_pane_restores_stashed_state() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
    }
    app.pr_review_toggle_to_session().unwrap();

    app.pr_review_return_to_pane();

    match &app.mode {
        AppMode::PrReview(state) => assert_eq!(state.selected, 1),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(
        app.pr_review_return.is_none(),
        "stash should be consumed on restore"
    );
}

#[test]
fn pr_review_return_to_pane_ignores_mismatched_session() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_toggle_to_session().unwrap();

    // Simulate having navigated away to an unrelated session's view.
    if let AppMode::Viewing(view) = &mut app.mode {
        view.session = "amf-other-feat".to_string();
        view.window = "claude".to_string();
    }

    app.pr_review_return_to_pane();

    assert!(
        matches!(&app.mode, AppMode::Viewing(view) if view.session == "amf-other-feat"),
        "should not have jumped into the stashed pane from an unrelated session"
    );
    assert!(
        app.pr_review_return.is_some(),
        "the stash should be left alone, not dropped, in case the user navigates back"
    );
}

#[test]
fn pr_review_return_to_pane_without_stash_shows_message() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Normal;

    app.pr_review_return_to_pane();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.pr_review_return.is_none());
}
