use super::*;

#[test]
fn ctrl_j_and_k_scroll_the_patch_further_than_j_and_k() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_long_patch_app(dir.path(), 60);

    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('j'))).unwrap();
    assert_eq!(patch_scroll_and_cursor(&app).0, PATCH_JUMP_STEP);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    assert_eq!(patch_scroll_and_cursor(&app).0, PATCH_JUMP_STEP + 1);
    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('k'))).unwrap();
    assert_eq!(patch_scroll_and_cursor(&app).0, 1);
}

#[test]
fn ctrl_j_and_k_move_the_line_cursor_further_than_j_and_k() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_long_patch_app(dir.path(), 60);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    assert_eq!(patch_scroll_and_cursor(&app).1, Some(0));

    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('j'))).unwrap();
    assert_eq!(patch_scroll_and_cursor(&app).1, Some(PATCH_JUMP_STEP));
    handle_diff_viewer_key(&mut app, ctrl(KeyCode::Char('k'))).unwrap();
    assert_eq!(patch_scroll_and_cursor(&app).1, Some(0));
}

#[test]
fn mouse_wheel_scrolls_the_patch_without_moving_the_line_cursor() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_long_patch_app(dir.path(), 60);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();

    handle_diff_viewer_wheel(&mut app, 3, true);
    handle_diff_viewer_wheel(&mut app, 3, true);
    assert_eq!(patch_scroll_and_cursor(&app), (6, Some(0)));
    handle_diff_viewer_wheel(&mut app, 3, false);
    assert_eq!(patch_scroll_and_cursor(&app), (3, Some(0)));
}

#[test]
fn mouse_wheel_scrolls_an_open_overlay_instead_of_the_patch() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_long_patch_app(dir.path(), 60);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('?'))).unwrap();
    // The overlay's scroll limit comes from its last draw.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.help_rendered_lines = 50;
        state.help_view_height = 10;
    }

    handle_diff_viewer_wheel(&mut app, 3, true);
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(state.patch_scroll, 0);
            assert_eq!(state.help_scroll, 3);
        }
        _ => unreachable!(),
    }
}

#[test]
fn mouse_wheel_does_nothing_while_typing_a_search() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_long_patch_app(dir.path(), 60);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.editing_search = true;
    }

    handle_diff_viewer_wheel(&mut app, 3, true);
    assert_eq!(patch_scroll_and_cursor(&app).0, 0);
}

#[test]
fn question_mark_toggles_the_review_help_overlay() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('?'))).unwrap();
    assert!(help_open(&app));

    // `?` closes it again, like the dashboard overlay.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('?'))).unwrap();
    assert!(!help_open(&app));
}

#[test]
fn help_overlay_opens_and_closes_with_the_line_cursor_active() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_patch(dir.path(), SEARCH_PATCH);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    let cursor_before = match &app.mode {
        AppMode::DiffViewer(state) => state.comment_cursor,
        _ => unreachable!(),
    };
    assert!(cursor_before.is_some());

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('?'))).unwrap();
    assert!(help_open(&app));

    // Esc closes the overlay without also unwinding cursor mode underneath.
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    assert!(!help_open(&app));
    match &app.mode {
        AppMode::DiffViewer(state) => assert_eq!(state.comment_cursor, cursor_before),
        _ => unreachable!(),
    }
}

#[test]
fn help_overlay_swallows_review_keys_while_open() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('?'))).unwrap();
    // `a` would approve, `n` would advance, `q` would start the finish flow.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.help_open);
            assert!(state.decisions.is_empty());
            assert_eq!(state.selected_file, 0);
        }
        _ => unreachable!(),
    }

    // `q` closes the overlay rather than opening the pre-finish summary.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.help_open);
            assert!(!state.summary_open);
        }
        _ => unreachable!(),
    }
}

#[test]
fn help_overlay_scroll_clamps_to_the_rendered_body() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('?'))).unwrap();

    // Stand in for what the renderer records each frame.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.help_rendered_lines = 40;
        state.help_view_height = 10;
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('G'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert_eq!(state.help_scroll, 30),
        _ => unreachable!(),
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert_eq!(state.help_scroll, 30),
        _ => unreachable!(),
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('g'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert_eq!(state.help_scroll, 0),
        _ => unreachable!(),
    }

    // Reopening always lands back at the top.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('G'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('?'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert_eq!(state.help_scroll, 0),
        _ => unreachable!(),
    }
}

#[test]
fn question_mark_is_inert_in_the_plain_diff_viewer() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.review = false;
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('?'))).unwrap();
    assert!(!help_open(&app));
}
