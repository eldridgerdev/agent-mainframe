use super::*;

fn pr_review_fix_confirm(app: &App) -> &crate::app::FixConfirmState {
    match &app.mode {
        AppMode::PrReview(state) => state
            .fix_confirm
            .as_ref()
            .expect("fix confirm dialog should be open"),
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_open_fix_confirm_seeds_editor_from_selection() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);

    assert_eq!(app.pr_review_fix_editing(), None);
    app.pr_review_open_fix_confirm();

    // Dialog opens in confirm (not edit) mode, seeded with the comment's prompt.
    assert_eq!(app.pr_review_fix_editing(), Some(false));
    let confirm = pr_review_fix_confirm(&app);
    let expected = match &app.mode {
        AppMode::PrReview(state) => state.selected_comment().unwrap().fix_prompt(),
        _ => unreachable!(),
    };
    assert!(confirm.editor.text().starts_with(&expected));
    assert!(
        confirm
            .editor
            .text()
            .contains("Address this PR review comment.")
    );
    assert!(
        confirm
            .editor
            .text()
            .contains("Do not post replies to GitHub")
    );
    assert!(
        confirm
            .editor
            .text()
            .contains("amf reply-draft --pr-number 7 --comment-id 1 --request-id")
    );
    assert_eq!(confirm.reply_draft_requests.len(), 1);
    assert_eq!(confirm.reply_draft_requests[0].comment_id, 1);
    assert_eq!(confirm.reply_draft_requests[0].base_head_sha, "sha");
    assert!(
        confirm
            .editor
            .text()
            .contains(&confirm.reply_draft_requests[0].request_id)
    );
}

#[test]
fn pr_review_fix_edit_mode_forwards_keys_and_cancel_closes() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_fix_confirm();

    // Keys are ignored until edit mode is entered.
    assert!(!app.pr_review_fix_editor_key(KeyEvent::from(KeyCode::Char('Z'))));
    let before = pr_review_fix_confirm(&app).editor.text().to_string();

    app.pr_review_fix_edit();
    assert_eq!(app.pr_review_fix_editing(), Some(true));
    assert!(app.pr_review_fix_editor_key(KeyEvent::from(KeyCode::Char('Z'))));
    assert_eq!(
        pr_review_fix_confirm(&app).editor.text(),
        format!("{before}Z")
    );

    // Leaving edit mode keeps the edited text; cancel closes the dialog.
    app.pr_review_fix_stop_edit();
    assert_eq!(app.pr_review_fix_editing(), Some(false));
    app.pr_review_cancel_fix();
    assert_eq!(app.pr_review_fix_editing(), None);
}

#[test]
fn pr_review_fix_vim_toggle_persists_across_reopen() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    // Default keymap is plain.
    app.pr_review_open_fix_confirm();
    assert!(app.pr_review_fix_vim_mode().is_none());

    // Toggling vim flips the editor and the remembered pane preference.
    app.pr_review_fix_toggle_vim();
    assert!(app.pr_review_fix_vim_mode().is_some());
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.fix_vim_enabled),
        _ => unreachable!(),
    }

    // Closing and reopening the dialog (e.g. for another comment) keeps vim on.
    app.pr_review_cancel_fix();
    app.pr_review_open_fix_confirm();
    assert!(
        app.pr_review_fix_vim_mode().is_some(),
        "reopened dialog should remember the vim choice"
    );

    // Toggling back off is likewise remembered.
    app.pr_review_fix_toggle_vim();
    app.pr_review_cancel_fix();
    app.pr_review_open_fix_confirm();
    assert!(app.pr_review_fix_vim_mode().is_none());
}

#[test]
fn pr_review_fix_scroll_moves_offset_and_clears_cursor_follow() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_fix_confirm();

    app.pr_review_fix_scroll(3);
    let confirm = pr_review_fix_confirm(&app);
    assert_eq!(confirm.scroll, 3);
    assert!(
        !confirm.sync_to_cursor,
        "an explicit scroll should stop following the cursor"
    );

    // Scrolling up saturates at zero rather than underflowing.
    app.pr_review_fix_scroll(-10);
    assert_eq!(pr_review_fix_confirm(&app).scroll, 0);
}

#[test]
fn pr_review_fix_confirm_scrolls_before_editing() {
    use crossterm::event::{KeyCode, KeyModifiers};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_fix_confirm();

    crate::handlers::handle_pr_review_key(
        &mut app,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    )
    .unwrap();
    assert_eq!(pr_review_fix_confirm(&app).scroll, 10);
    assert!(!pr_review_fix_confirm(&app).sync_to_cursor);

    crate::handlers::handle_pr_review_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
    )
    .unwrap();
    assert_eq!(pr_review_fix_confirm(&app).scroll, 9);
}

#[test]
fn pr_review_fix_confirm_can_change_target_without_losing_edits() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    app.pr_review_harness_pick_confirm();
    app.pr_review_harness_pick_confirm();
    app.pr_review_fix_edit();
    assert!(app.pr_review_fix_editor_key(KeyEvent::from(KeyCode::Char('Z'))));
    app.pr_review_fix_stop_edit();
    let prompt = pr_review_fix_confirm(&app).editor.text().to_string();

    app.pr_review_change_fix_target();
    assert!(app.pr_review_harness_picking());
    app.pr_review_harness_pick_move(-1); // Dedicated default -> existing live.
    app.pr_review_harness_pick_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.fix_target,
                crate::app::pr_review::FixTarget::ExistingLive
            );
            assert_eq!(state.fix_confirm.as_ref().unwrap().editor.text(), prompt);
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_reopened_target_picker_highlights_current_target() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    app.pr_review_harness_pick_confirm();
    app.pr_review_harness_pick_confirm();

    // Redirect the fix to the existing live session, then reopen the picker
    // the way "review and redirect" does.
    app.pr_review_change_fix_target();
    app.pr_review_harness_pick_move(-1); // Dedicated default -> existing live (row 0).
    app.pr_review_harness_pick_confirm();
    app.pr_review_change_fix_target();

    match &app.mode {
        AppMode::PrReview(state) => {
            let pick = state.harness_pick.as_ref().expect("picker reopened");
            assert_eq!(
                pick.selected, 0,
                "reopened picker should highlight the current ExistingLive target, not the dedicated default"
            );
        }
        _ => unreachable!(),
    }
}
