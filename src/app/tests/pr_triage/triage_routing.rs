use super::*;

#[test]
fn pr_review_open_fix_confirm_without_comments_shows_message() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 0);

    app.pr_review_open_fix_confirm();
    assert_eq!(app.pr_review_fix_editing(), None);
    assert!(app.message.is_some());
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
