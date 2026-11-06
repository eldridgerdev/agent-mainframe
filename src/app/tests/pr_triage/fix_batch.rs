use super::*;

#[test]
fn pr_review_first_fix_opens_harness_picker_then_confirm() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    // First `f`: no dedicated session and no harness chosen yet → the picker
    // opens; the fix confirm has NOT opened yet.
    app.pr_review_open_fix_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(
                state.harness_pick.is_some(),
                "harness picker should be open"
            );
            assert!(state.fix_confirm.is_none(), "fix confirm should wait");
            assert!(state.review_harness.is_none());
            // First row is always "existing live session"; default highlight
            // is the dedicated row for the project's preferred agent.
            let pick = state.harness_pick.as_ref().unwrap();
            assert_eq!(
                pick.rows[0],
                crate::app::pr_review::FixTargetPickRow::ExistingLive(None)
            );
            assert_eq!(
                pick.rows[pick.selected],
                crate::app::pr_review::FixTargetPickRow::Dedicated(AgentKind::default())
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    // Choosing the harness advances to the optional session-name step. A
    // second confirmation accepts the backwards-compatible default name and
    // continues into the fix confirm.
    app.pr_review_harness_pick_confirm();
    assert!(app.pr_review_harness_pick_naming());
    app.pr_review_harness_pick_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none());
            assert_eq!(state.review_harness, Some(AgentKind::default()));
            assert_eq!(
                state.dedicated_session_label,
                crate::app::pr_review::TRIAGE_SESSION_LABEL
            );
            assert!(
                state.fix_confirm.is_some(),
                "fix confirm should now be open"
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_dedicated_target_accepts_a_custom_session_name() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();
    app.pr_review_harness_pick_confirm();
    assert!(app.pr_review_harness_pick_naming());
    for c in "PR 321 security".chars() {
        app.pr_review_harness_pick_name_push(c);
    }
    app.pr_review_harness_pick_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.dedicated_session_label, "PR 321 security");
            assert_eq!(state.review_harness, Some(AgentKind::default()));
            assert!(state.harness_pick.is_none());
            assert!(state.fix_confirm.is_some());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_dedicated_target_rejects_a_name_owned_by_another_harness() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "PR 321 security".to_string());
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_open_fix_confirm();
    if let AppMode::PrReview(state) = &mut app.mode {
        let pick = state.harness_pick.as_mut().unwrap();
        pick.selected = pick
            .rows
            .iter()
            .position(|row| {
                *row == crate::app::pr_review::FixTargetPickRow::Dedicated(AgentKind::Codex)
            })
            .unwrap();
    }
    app.pr_review_harness_pick_confirm();
    for c in "PR 321 security".chars() {
        app.pr_review_harness_pick_name_push(c);
    }
    app.pr_review_harness_pick_confirm();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state)
            if state.harness_pick.is_some()
                && state.review_harness.is_none()
                && state.fix_confirm.is_none()),
        "the naming step should stay open after a harness collision"
    );
    assert!(
        app.toasts
            .last()
            .is_some_and(|toast| toast.message.contains("already runs Claude")),
        "expected an actionable harness-conflict toast, got {:?}",
        app.toasts.last().map(|toast| &toast.message)
    );
}

#[test]
fn pr_review_second_fix_skips_harness_picker() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    // Harness already chosen for this PR: `f` goes straight to the fix confirm.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review_harness = Some(AgentKind::Codex);
    }
    app.pr_review_open_fix_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(
                state.harness_pick.is_none(),
                "no picker on subsequent fixes"
            );
            assert!(state.fix_confirm.is_some());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_toggle_mark_adds_and_removes() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);

    // Mark the first comment, move down, mark the second.
    app.pr_review_toggle_mark();
    app.pr_review_select_next();
    app.pr_review_toggle_mark();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.marked.len(), 2);
            assert!(state.marked.contains(&1));
            assert!(state.marked.contains(&2));
        }
        _ => unreachable!(),
    }

    // Toggling the same comment again unmarks it.
    app.pr_review_toggle_mark();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.marked.len(), 1);
            assert!(!state.marked.contains(&2));
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_with_nothing_marked_hints() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);

    app.pr_review_open_batch_confirm();
    assert!(
        app.message.as_deref().unwrap_or("").contains("press space"),
        "expected a hint to mark comments, got {:?}",
        app.message
    );
    // No dialog opened.
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.fix_confirm.is_none()),
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_opens_combined_dialog_for_marked() {
    // `pr_review_test_app` has no feature at /tmp/wd, so no harness pick is
    // needed — `B` builds the combined dialog directly.
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.marked.insert(1);
        state.marked.insert(3);
    }

    app.pr_review_open_batch_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("batch dialog should open");
            // The dialog carries both marked ids (list order).
            let mut ids = confirm.batch.clone().expect("dialog is a batch");
            ids.sort();
            assert_eq!(ids, vec![1, 3]);
            // One combined prompt, numbered, covering both files, unmarked one absent.
            let text = confirm.editor.text();
            assert!(text.starts_with("Address these PR review comments."));
            assert!(text.contains("Comment 1:") && text.contains("Comment 2:"));
            assert!(text.contains("File: src/file1.rs:1"));
            assert!(text.contains("File: src/file3.rs:3"));
            assert!(!text.contains("File: src/file2.rs"));
            assert_eq!(confirm.reply_draft_requests.len(), 2);
            assert_eq!(
                confirm
                    .reply_draft_requests
                    .iter()
                    .map(|request| request.comment_id)
                    .collect::<Vec<_>>(),
                vec![1, 3]
            );
            assert!(
                confirm
                    .reply_draft_requests
                    .iter()
                    .all(|request| request.base_head_sha == "sha")
            );
            assert!(text.contains("--comment-id 1 --request-id"));
            assert!(text.contains("--comment-id 3 --request-id"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_excludes_resolved_marks() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        // Mark both, but one is already resolved on GitHub.
        state.marked.insert(1);
        state.marked.insert(2);
        state.review.comments[0].is_resolved = true;
    }

    app.pr_review_open_batch_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("batch dialog should open");
            // Only the unresolved comment is included (token principle #6).
            assert_eq!(confirm.batch.clone().unwrap(), vec![2]);
            assert!(!confirm.editor.text().contains("File: src/file1.rs"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_all_resolved_hints() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.marked.insert(1);
        state.review.comments[0].is_resolved = true;
    }

    app.pr_review_open_batch_confirm();
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("all resolved"),
        "expected an all-resolved hint, got {:?}",
        app.message
    );
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.fix_confirm.is_none()),
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_routes_through_harness_pick_then_opens_combined() {
    // A dedicated-review PR with no session yet picks the harness before the
    // first fix — the batch flow must route the picker's continuation back to
    // the combined dialog (not the single-comment one).
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.marked.insert(1);
        state.marked.insert(2);
    }

    // `B`: picker opens, no dialog yet, and the pending action is the batch.
    app.pr_review_open_batch_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(
                state.harness_pick.is_some(),
                "harness picker should be open"
            );
            assert!(state.fix_confirm.is_none());
            assert!(state.pending_batch, "pending action should be the batch");
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    // Choosing the harness, then accepting the default session name, opens
    // the *combined* dialog and clears the flag.
    app.pr_review_harness_pick_confirm();
    assert!(app.pr_review_harness_pick_naming());
    app.pr_review_harness_pick_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(!state.pending_batch);
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("batch dialog should open");
            let mut ids = confirm.batch.clone().expect("dialog is a batch");
            ids.sort();
            assert_eq!(ids, vec![1, 2]);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_cancel_harness_picker_aborts_fix() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_open_fix_confirm();
    assert!(app.pr_review_harness_picking());

    // Cancel: picker closes, nothing injected, harness stays unset so the next
    // `f` asks again.
    app.pr_review_harness_pick_cancel();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none());
            assert!(state.fix_confirm.is_none());
            assert!(state.review_harness.is_none());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_i_is_noop_for_comment_without_file_path() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    // Conversation/summary comments carry no path — nothing to highlight.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments[0].path = None;
    }

    app.open_syntax_language_picker_for_selected_diff_file();

    assert!(
        matches!(app.mode, AppMode::PrReview(_)),
        "expected to stay in the review pane, got {:?}",
        std::mem::discriminant(&app.mode)
    );
}
