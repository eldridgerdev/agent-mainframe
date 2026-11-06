use super::*;

#[test]
fn finishing_with_undecided_files_requires_confirmation() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    // Approve only a.rs; b.rs has no verdict.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    // q shows the confirmation rather than finishing.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.finish_confirm));
    // A second q (finish-anyway) opens the pre-finish summary instead of
    // finishing outright.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.summary_open));
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if !s.finish_confirm));
    // A third q, from the summary, actually finishes.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    assert!(matches!(app.mode, AppMode::Viewing(_)));
}

#[test]
fn escape_cancels_finish_confirmation() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap(); // both undecided
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.finish_confirm));
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap(); // cancel
    match &app.mode {
        AppMode::DiffViewer(s) => assert!(!s.finish_confirm),
        _ => panic!("should still be reviewing"),
    }
}

#[test]
fn deciding_during_confirmation_clears_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap(); // undecided -> confirm
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.finish_confirm));
    // Approving clears the confirmation and records the verdict.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(s) => {
            assert!(!s.finish_confirm);
            assert!(s.decisions.contains_key("a.rs"));
        }
        _ => panic!("expected diff viewer"),
    }
    // q now opens the pre-finish summary immediately (all files decided,
    // no undecided-files confirmation needed).
    finish_review(&mut app);
    assert!(matches!(app.mode, AppMode::Viewing(_)));
}

#[test]
fn summary_lists_every_verdict_and_general_feedback_in_order() {
    use crate::app::SummaryItem;
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    // Reject a.rs with feedback, approve b.rs, then write general feedback.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for c in "fix it".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // advance to b.rs
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('f'))).unwrap();
    for c in "nice work overall".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.summary_open);
            assert_eq!(
                state.summary_items(),
                vec![
                    SummaryItem::File { file_idx: 0 },
                    SummaryItem::File { file_idx: 1 },
                    SummaryItem::General,
                ]
            );
        }
        _ => panic!("expected diff viewer with summary open"),
    }
}

#[test]
fn esc_closes_summary_without_finishing() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.summary_open));

    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.summary_open);
            // Nothing about the review itself changed.
            assert!(state.decisions.contains_key("a.rs"));
        }
        _ => panic!("expected the review to still be open, not finished"),
    }
    assert!(!dir.path().join(".claude/final-review-feedback.md").exists());
}

#[test]
fn summary_navigation_clamps_at_both_ends() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs", "c.rs"]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();

    let selected = |app: &App| match &app.mode {
        AppMode::DiffViewer(s) => s.summary_selected,
        _ => panic!("expected diff viewer"),
    };
    assert_eq!(selected(&app), 0);
    // k at the top stays put.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('k'))).unwrap();
    assert_eq!(selected(&app), 0);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('G'))).unwrap();
    assert_eq!(selected(&app), 2);
    // j at the bottom stays put.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    assert_eq!(selected(&app), 2);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('g'))).unwrap();
    assert_eq!(selected(&app), 0);
}

#[test]
fn summary_enter_on_line_comment_jumps_and_opens_editor_prefilled() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "bug here".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    // The comment auto-rejected the file, so summary_items is
    // [File { 0 }, LineComment { 0, 0 }].
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.summary_open);
            assert!(state.editing_line_comment);
            assert_eq!(state.feedback_editor.text(), "bug here");
            assert_eq!(state.selected_file, 0);
        }
        _ => panic!("expected diff viewer back in the line-comment editor"),
    }
}

#[test]
fn summary_enter_on_rejected_file_jumps_and_opens_feedback_editor_prefilled() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for c in "needs work".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // -> b.rs
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    // Selection starts on the a.rs row (its rejection).
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.summary_open);
            assert!(state.feedback_editing);
            assert_eq!(state.feedback_editor.text(), "needs work");
            assert_eq!(state.selected_file, 0);
        }
        _ => panic!("expected diff viewer back in the rejection editor"),
    }
}

#[test]
fn summary_enter_on_approved_file_just_navigates() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap(); // jump to a.rs's row

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.summary_open);
            assert!(!state.feedback_editing);
            assert!(!state.editing_general);
            assert_eq!(state.selected_file, 0);
        }
        _ => panic!("expected diff viewer, no editor opened"),
    }
}

#[test]
fn summary_enter_on_file_comment_jumps_and_opens_editor_prefilled() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('m'))).unwrap();
    for c in "consider splitting".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    // File comments are verdict-free, so a.rs is still undecided: q raises
    // the confirmation first.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('y'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap(); // -> FileComment row
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.summary_open);
            assert!(state.editing_file_comment);
            assert_eq!(state.feedback_editor.text(), "consider splitting");
        }
        _ => panic!("expected diff viewer back in the file-comment editor"),
    }
}

#[test]
fn summary_enter_on_general_feedback_jumps_and_opens_editor_prefilled() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('f'))).unwrap();
    for c in "great work".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap(); // -> General row
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.summary_open);
            assert!(state.editing_general);
            assert_eq!(state.feedback_editor.text(), "great work");
        }
        _ => panic!("expected diff viewer back in the general-feedback editor"),
    }
}

#[test]
fn esc_pauses_review_without_writing_feedback() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    // Both files are undecided; q here would only raise the finish
    // confirmation, but top-level Esc pauses immediately without it.
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert!(!dir.path().join(".claude/final-review-feedback.md").exists());
}

#[test]
fn esc_pauses_review_after_a_decision_and_keeps_progress_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // Approving persists progress incrementally, independent of pause/finish.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    let progress_path = dir.path().join(".claude/final-review-progress.json");
    assert!(progress_path.exists());

    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert!(!dir.path().join(".claude/final-review-feedback.md").exists());
    // Pausing must not clear the progress a reviewer will resume from.
    let saved = std::fs::read_to_string(&progress_path).unwrap();
    assert!(saved.contains("a.rs"));
}

#[test]
// The child is only waited on in the success-path match arm below; the
// `_ => panic!(...)` arm exists purely to fail the test and never
// reaches it, which clippy's static analysis can't prove is fine.
#[allow(clippy::zombie_processes)]
fn esc_does_not_pause_while_finish_check_is_running() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();

    // Simulate `q` having already spawned a background finish-check
    // command that hasn't reported back yet.
    let child = crate::app::review::ReviewCheckRun::spawn(dir.path(), "sleep 30").unwrap();
    let child_id = child.id();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.finish_check_command = Some("sleep 30".to_string());
        state.finish_check_child = Some(child);
    }

    // Esc must not drop the DiffViewerState here — doing so would orphan
    // the check child and the review would never actually complete.
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    match &mut app.mode {
        AppMode::DiffViewer(state) => {
            let child = state.finish_check_child.as_mut().expect("child kept");
            assert_eq!(child.id(), child_id);
            child.kill().ok();
            child.wait().ok();
        }
        _ => panic!("expected the viewer to stay open while the check runs"),
    }
}
