use super::*;

#[test]
fn v_toggles_layout_in_final_review() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::Unified
    ));

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();

    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::SideBySide
    ));
}

#[test]
fn v_on_new_file_in_final_review_is_a_no_op_with_a_message() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].status = DiffFileStatus::Added;
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();

    // The toggle is a no-op (still Unified, the only layout new files can
    // render), but unlike the pre-fix behavior it must explain why rather
    // than silently doing nothing.
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::Unified
    ));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("new/untracked"))
    );
}

#[test]
fn layout_preference_survives_a_new_file_in_final_review() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[1].status = DiffFileStatus::Untracked;
    }

    // Choose side-by-side on the ordinary file.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::SideBySide
    ));

    // Moving onto the untracked file forces the *render* to unified, but
    // must not overwrite the stored preference.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
    assert!(app.diff_viewer_selected_file_is_new());
    assert_eq!(
        app.diff_viewer_layout(),
        Some(crate::app::DiffViewerLayout::Unified)
    );
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::SideBySide
    ));

    // Moving back restores the side-by-side render.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('p'))).unwrap();
    assert!(!app.diff_viewer_selected_file_is_new());
    assert_eq!(
        app.diff_viewer_layout(),
        Some(crate::app::DiffViewerLayout::SideBySide)
    );
}

#[test]
fn jump_to_next_undecided_skips_decided_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs", "c.rs"]);

    // Decide a.rs and b.rs, then return to a.rs.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve a -> b
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve b -> c
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('p'))).unwrap(); // c -> b
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('p'))).unwrap(); // b -> a

    // u jumps past the decided a.rs/b.rs to the only undecided file, c.rs.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('u'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(s) => assert_eq!(s.selected_file, 2),
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn file_filter_cycles_and_navigation_skips_hidden_files() {
    use crate::app::FileFilter;
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs", "c.rs"]);

    let filter = |app: &App| match &app.mode {
        AppMode::DiffViewer(s) => s.file_filter,
        _ => panic!("expected diff viewer"),
    };
    let selected = |app: &App| match &app.mode {
        AppMode::DiffViewer(s) => s.selected_file,
        _ => panic!("expected diff viewer"),
    };

    // Approve a.rs (-> b.rs) and reject c.rs, leaving b.rs undecided.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve a -> b
    // Reject c.rs: move to c, then reject with feedback.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap(); // b -> c
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for ch in "nope".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // submit, c is last so stays

    // F cycles All -> Undecided. Only b.rs (index 1) is undecided, so the
    // selection snaps to it.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
    assert_eq!(filter(&app), FileFilter::Undecided);
    assert_eq!(selected(&app), 1, "snaps onto the only undecided file");

    // Navigation stays put: b.rs is the only visible file.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
    assert_eq!(selected(&app), 1);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('p'))).unwrap();
    assert_eq!(selected(&app), 1);

    // F again -> Rejected: only c.rs (index 2) is visible.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
    assert_eq!(filter(&app), FileFilter::Rejected);
    assert_eq!(selected(&app), 2, "snaps onto the only rejected file");

    // F again -> Blockers: c.rs was rejected explicitly (default Blocker
    // severity), so it stays visible.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
    assert_eq!(filter(&app), FileFilter::Blockers);
    assert_eq!(selected(&app), 2, "snaps onto the only blocker file");

    // F again wraps back to All (Changed is skipped with no prior review).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
    assert_eq!(filter(&app), FileFilter::All);
}

#[test]
fn file_comment_is_verdict_free_filterable_and_written() {
    use crate::app::FileFilter;
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('m'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.editing_file_comment
    ));
    for ch in "consider splitting this module".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(
                !state.decisions.contains_key("a.rs"),
                "comment is verdict-free"
            );
            let comment = state.file_comments.get("a.rs").expect("comment stored");
            assert_eq!(comment.text, "consider splitting this module");
            assert!(!comment.resolved);
        }
        _ => panic!("expected diff viewer"),
    }

    // All -> Undecided -> Rejected -> Blockers -> FileComments. Empty
    // filters remain valid states; the file-comment step selects this file.
    for _ in 0..4 {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
    }
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.file_filter == FileFilter::FileComments
    ));

    // Approving remains independent of the comment and finishing emits a
    // dedicated whole-file section.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    finish_review(&mut app);
    let feedback =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(feedback.contains("### File Comments"));
    assert!(feedback.contains("#### a.rs — [suggestion]"));
    assert!(feedback.contains("consider splitting this module"));
}
