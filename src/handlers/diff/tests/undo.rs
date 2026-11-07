use super::*;

#[test]
fn undo_restores_the_previous_verdict_and_returns_to_the_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    // Approve a.rs: the verdict lands and the selection advances to b.rs.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(state.selected_file, 1);
            assert!(state.decisions.contains_key("a.rs"));
        }
        _ => panic!("left the diff viewer"),
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.decisions.contains_key("a.rs"));
            assert_eq!(
                state.selected_file, 0,
                "undo must land back on the file whose verdict it took back"
            );
            assert!(state.verdict_undo.is_empty());
        }
        _ => panic!("left the diff viewer"),
    }

    // Nothing left to undo.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    assert_eq!(app.message.as_deref(), Some("No verdict to undo"));
}

#[test]
fn undo_walks_back_through_successive_verdicts() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // a.rs
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('s'))).unwrap(); // skip b.rs
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty(), "both verdicts undone");
            assert_eq!(state.selected_file, 0);
        }
        _ => panic!("left the diff viewer"),
    }
}

#[test]
fn undo_restores_a_comment_implied_rejection_as_implicit() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    // a.rs is rejected implicitly by a line comment (see
    // `diff_review_sync_auto_reject`), not by a typed rejection.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.decisions.insert(
            "a.rs".to_string(),
            crate::app::ReviewDecision::Reject {
                feedback: String::new(),
                severity: crate::app::Severity::default(),
            },
        );
        state.auto_rejected.insert("a.rs".to_string());
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(matches!(
                state.decisions.get("a.rs"),
                Some(crate::app::ReviewDecision::Reject { .. })
            ));
            assert!(
                state.auto_rejected.contains("a.rs"),
                "undo must restore the implicit/explicit distinction, not just the verdict"
            );
        }
        _ => panic!("left the diff viewer"),
    }
}

#[test]
fn repeating_an_identical_verdict_does_not_stack_a_no_op_undo() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // Two approvals of the only file: the second changes nothing, so it must
    // not leave a `U` that appears to do something.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert_eq!(state.verdict_undo.len(), 1),
        _ => panic!("left the diff viewer"),
    }
}

// ── AMF's own files, grouped after the reviewer's ──────────────
