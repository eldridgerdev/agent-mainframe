use super::*;

#[test]
fn history_navigation_loads_archive_only_after_live_rounds() {
    let dir = tempfile::TempDir::new().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("final-review-feedback.md"),
        "\
# Final Review Feedback

## Review — r3

newest

## Review — r2

middle
",
    )
    .unwrap();
    std::fs::write(
        claude_dir.join("final-review-feedback-archive.md"),
        "\
# Final Review Feedback Archive

## Review — r1

oldest
",
    )
    .unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('H'))).unwrap();
    let history = match &app.mode {
        AppMode::DiffViewer(state) => state.review_history.as_ref().unwrap(),
        _ => panic!("not in review"),
    };
    assert_eq!(history.rounds.len(), 2);
    assert_eq!(history.selected, 0);
    assert!(!history.archive_loaded);

    // Current -> newest live -> older live does not touch the archive.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('l'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Right)).unwrap();
    let history = match &app.mode {
        AppMode::DiffViewer(state) => state.review_history.as_ref().unwrap(),
        _ => panic!("not in review"),
    };
    assert_eq!(history.selected, 2);
    assert!(!history.archive_loaded);

    // Crossing the loaded tail lazily appends the archive newest-first.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('l'))).unwrap();
    let history = match &app.mode {
        AppMode::DiffViewer(state) => state.review_history.as_ref().unwrap(),
        _ => panic!("not in review"),
    };
    assert_eq!(history.selected, 3);
    assert!(history.archive_loaded);
    assert_eq!(history.rounds.len(), 3);
    assert_eq!(history.rounds[2].title, "Review — r1");

    // History owns the keys while open: a verdict binding must not leak to
    // the underlying review.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(state) if state.decisions.is_empty()));

    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(state) if state.review_history.is_none()));
}
