use super::*;

#[test]
fn feedback_editor_accepts_multiline_input_and_tab_submits() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // Reject a.rs with a two-line note: Enter inserts a newline (not submit),
    // Tab submits.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for c in "first".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "second".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    // Still editing after Enter (newline, not submit).
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.feedback_editing
    ));
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.feedback_editing);
            match state.decisions.get("a.rs") {
                Some(crate::app::ReviewDecision::Reject { feedback, .. }) => {
                    assert_eq!(feedback, "first\nsecond");
                }
                other => panic!("expected rejection with feedback, got {other:?}"),
            }
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn review_vim_defaults_off_and_applies_to_every_editor_surface() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state)
            if !state.vim_enabled && state.feedback_editor.vim_mode().is_none()
    ));
    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('t'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state)
            if state.vim_enabled
                && state.feedback_editor.vim_mode()
                    == Some(crate::editor::VimMode::Normal)
    ));

    // Rejection, general, and whole-file comment editors all inherit the
    // same session preference.
    for open in ['r', 'f', 'm'] {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(open))).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if state.feedback_editor.vim_mode()
                    == Some(crate::editor::VimMode::Normal)
        ));
        handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('q'))).unwrap();
    }

    // Line comments and suggested replacements use that preference too.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state)
            if state.editing_line_comment
                && state.feedback_editor.vim_mode()
                    == Some(crate::editor::VimMode::Normal)
    ));
    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('q'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state)
            if state.editing_suggestion
                && state.feedback_editor.vim_mode()
                    == Some(crate::editor::VimMode::Normal)
    ));
}

#[test]
fn vim_commands_flow_through_comment_and_suggestion_workflows() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('t'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    // Reuse TextEditor's insert, motion, and delete-operator commands.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('i'))).unwrap();
    for c in "alpha beta".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('0'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('d'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('w'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    // Switch to the suggestion editor without toggling again. It opens in
    // Normal mode and preserves the already-submitted prose on the span.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('0'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('w'))).unwrap();
    for c in "replacement".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.vim_enabled);
            let comment = &state.line_comments["a.rs"][0];
            assert_eq!(comment.text, "beta");
            assert_eq!(comment.suggestion.as_deref(), Some("replacement line"));
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn disabling_vim_preserves_text_and_resets_cursor_and_undo() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('f'))).unwrap();
    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('t'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('i'))).unwrap();
    for c in "draft".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('0'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.feedback_editor.cursor() == 0
    ));

    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('t'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.vim_enabled);
            assert_eq!(state.feedback_editor.text(), "draft");
            assert_eq!(state.feedback_editor.cursor(), "draft".len());
            assert!(state.feedback_editor.vim_mode().is_none());
        }
        _ => panic!("expected diff viewer"),
    }

    // Re-enabling creates a fresh Normal editor. `u` cannot reach the Vim
    // history that existed before Vim was disabled.
    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('t'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('u'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state)
            if state.feedback_editor.text() == "draft"
                && state.feedback_editor.vim_mode()
                    == Some(crate::editor::VimMode::Normal)
    ));
}

#[test]
fn vim_editor_state_survives_review_focus_updates() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('t'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('f'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('i'))).unwrap();
    for c in "draft".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('0'))).unwrap();

    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
        state.on_file_changed();
        assert_eq!(state.feedback_editor.text(), "draft");
        assert_eq!(state.feedback_editor.cursor(), 0);
        assert_eq!(
            state.feedback_editor.vim_mode(),
            Some(crate::editor::VimMode::Normal)
        );
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('u'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.feedback_editor.text().is_empty()
    ));
}

#[test]
fn vim_boundary_motions_stay_inside_the_editor() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.vim_enabled = true;
        state.feedback_editing = true;
        state.comment_cursor = Some(7);
        state.reset_feedback_editor("top\nbottom".to_string());
    }

    // Move to the first line, then press up again at the boundary. Neither
    // the line cursor nor selected file may receive the unproductive move.
    handle_diff_viewer_key(&mut app, key(KeyCode::Home)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Up)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Up)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(state.feedback_editor.cursor_row_col().0, 0);
            assert_eq!(state.comment_cursor, Some(7));
            assert_eq!(state.selected_file, 0);
        }
        _ => panic!("expected diff viewer"),
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Down)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Down)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(state.feedback_editor.cursor_row_col().0, 1);
            assert_eq!(state.comment_cursor, Some(7));
            assert_eq!(state.selected_file, 0);
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn vim_cancel_discards_unsent_comment_and_suggestion() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('t'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('i'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('x'))).unwrap();
    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('q'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.line_comments.is_empty()
    ));

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('i'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('x'))).unwrap();
    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('q'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state)
            if state.line_comments.is_empty()
                && state.vim_enabled
                && state.feedback_editor.text().is_empty()
    ));
}

#[test]
fn closing_and_reopening_review_clears_unsent_editor_state_and_vim() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('t'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('f'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('i'))).unwrap();
    for c in "unsent".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }

    app.close_diff_viewer();
    let AppMode::Viewing(view) = &app.mode else {
        panic!("expected return to viewing mode");
    };
    let reopened = DiffViewerState::new(view.clone(), dir.path().to_path_buf());
    assert!(!reopened.vim_enabled);
    assert!(reopened.feedback_editor.text().is_empty());
    assert!(reopened.feedback_editor.vim_mode().is_none());
    assert_eq!(reopened.feedback_editor.cursor(), 0);
}
