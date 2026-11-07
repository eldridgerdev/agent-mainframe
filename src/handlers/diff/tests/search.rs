use super::*;

fn search_state(app: &App) -> (&str, &[usize], Option<usize>, Option<usize>, bool) {
    match &app.mode {
        AppMode::DiffViewer(s) => (
            s.search_query.as_str(),
            s.search_matches.as_slice(),
            s.search_match_pos,
            s.comment_cursor,
            s.editing_search,
        ),
        _ => panic!("not in review"),
    }
}

/// A review app whose file carries the blobs context expansion needs: a
/// 30-line file with line 15 rewritten.
fn make_review_app_with_expandable_file(workdir: &Path) -> App {
    let old: String = (1..=30).map(|i| format!("l{i}\n")).collect();
    let new = old.replace("l15\n", "l15 changed\n");
    let mut app = make_review_app_with_patch(
        workdir,
        "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -12,7 +12,7 @@
 l12
 l13
 l14
-l15
+l15 changed
 l16
 l17
 l18
",
    );
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].old_content = Some(old);
        state.files[0].new_content = Some(new);
    }
    app
}

fn context_hunk_len(app: &App) -> usize {
    match &app.mode {
        AppMode::DiffViewer(state) => state.files[0].hunks[0].old_lines,
        _ => panic!("not in the diff viewer"),
    }
}

#[test]
fn plus_and_minus_step_the_context_in_final_review() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_expandable_file(dir.path());
    assert_eq!(context_hunk_len(&app), 7);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('+'))).unwrap();
    assert_eq!(context_hunk_len(&app), 21);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('-'))).unwrap();
    assert_eq!(context_hunk_len(&app), 7);

    // `*` jumps straight to the whole file and back.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('*'))).unwrap();
    assert_eq!(context_hunk_len(&app), 30);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('*'))).unwrap();
    assert_eq!(context_hunk_len(&app), 7);
}

#[test]
fn context_keys_still_work_with_the_line_cursor_active() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_expandable_file(dir.path());

    // `c` activates the cursor; the cursor-mode block must not swallow the
    // context keys, since widening context is most useful while reading a
    // specific line.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.comment_cursor.is_some()));

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('='))).unwrap();
    assert_eq!(context_hunk_len(&app), 21);
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.comment_cursor.is_some()));
}

#[test]
fn slash_opens_search_and_typing_jumps_cursor() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_patch(dir.path(), SEARCH_PATCH);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('/'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.editing_search));

    for c in "beta".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    let (query, matches, pos, cursor, editing) = search_state(&app);
    assert_eq!(query, "beta");
    assert_eq!(matches, &[1, 2]);
    assert_eq!(pos, Some(0));
    // Cursor jumped from the start (0) to the first match (line index 1).
    assert_eq!(cursor, Some(1));
    assert!(editing);

    // Enter commits: prompt closes but the query + matches persist.
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    let (query, matches, _, _, editing) = search_state(&app);
    assert_eq!(query, "beta");
    assert_eq!(matches, &[1, 2]);
    assert!(!editing);
}

#[test]
fn n_and_shift_n_cycle_matches_with_wraparound() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_patch(dir.path(), SEARCH_PATCH);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('/'))).unwrap();
    for c in "beta".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    // matches [1, 2], starting at pos 0 (cursor 1).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
    let (_, _, pos, cursor, _) = search_state(&app);
    assert_eq!((pos, cursor), (Some(1), Some(2)));
    // Wrap forward.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
    let (_, _, pos, cursor, _) = search_state(&app);
    assert_eq!((pos, cursor), (Some(0), Some(1)));
    // Backward wraps to the last match.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('N'))).unwrap();
    let (_, _, pos, cursor, _) = search_state(&app);
    assert_eq!((pos, cursor), (Some(1), Some(2)));
}

#[test]
fn esc_clears_a_committed_search() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_patch(dir.path(), SEARCH_PATCH);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('/'))).unwrap();
    for c in "beta".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    assert!(app.diff_search_active());

    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    let (query, matches, pos, _, editing) = search_state(&app);
    assert_eq!(query, "");
    assert!(matches.is_empty());
    assert_eq!(pos, None);
    assert!(!editing);
    // Still in the review viewer (Esc peeled off the search, not the review).
    assert!(matches!(app.mode, AppMode::DiffViewer(_)));
}

#[test]
fn n_navigates_files_when_no_search_is_active() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    let selected = |app: &App| match &app.mode {
        AppMode::DiffViewer(s) => s.selected_file,
        _ => panic!("not in review"),
    };
    assert_eq!(selected(&app), 0);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
    assert_eq!(selected(&app), 1);
}
