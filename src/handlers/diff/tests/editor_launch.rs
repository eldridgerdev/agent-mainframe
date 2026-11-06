use super::*;

/// A review over `SEARCH_PATCH` whose `a.rs` also exists on disk, since
/// opening in `$EDITOR` validates the file in the worktree before asking
/// the main loop to suspend.
fn make_editor_review_app(workdir: &Path) -> App {
    std::fs::write(
        workdir.join("a.rs"),
        "fn alpha() {}\nfn beta_two() {}\nfn gamma_alpha() {}\nfn delta() {}\n",
    )
    .unwrap();
    make_review_app_with_patch(workdir, SEARCH_PATCH)
}

#[test]
fn e_requests_the_editor_at_the_cursored_line() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_editor_review_app(dir.path());

    // Park the cursor on `fn gamma_alpha() {}` — addressable index 3,
    // new-side line 3.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        assert!(state.comment_cursor.is_some(), "expected cursor mode");
        state.comment_cursor = Some(3);
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('E'))).unwrap();

    let request = app.pending_editor.as_ref().expect("editor request");
    assert_eq!(request.path, dir.path().join("a.rs"));
    assert_eq!(request.workdir, dir.path());
    assert_eq!(request.line, Some(3));
    assert_eq!(request.display, "a.rs");
}

#[test]
fn e_without_the_line_cursor_opens_at_the_first_hunk() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_editor_review_app(dir.path());

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('E'))).unwrap();

    let request = app.pending_editor.as_ref().expect("editor request");
    assert_eq!(request.line, Some(1));
}

#[test]
fn e_reports_instead_of_opening_a_file_with_nothing_on_disk() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_editor_review_app(dir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].status = DiffFileStatus::Deleted;
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('E'))).unwrap();

    assert!(
        app.pending_editor.is_none(),
        "a deleted file must not suspend the TUI"
    );
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("deleted")),
        "expected an explanation, got {:?}",
        app.message
    );
}

#[test]
fn e_reports_when_the_file_is_missing_from_the_worktree() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_editor_review_app(dir.path());
    std::fs::remove_file(dir.path().join("a.rs")).unwrap();

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('E'))).unwrap();

    assert!(app.pending_editor.is_none());
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.starts_with("Cannot open a.rs")),
        "expected a resolution failure, got {:?}",
        app.message
    );
}
