use super::*;

fn make_review_app_with_comments(workdir: &Path) -> App {
    let mut app = make_review_app(workdir, &["a.rs"]);
    let files = crate::diff::parse_unified_diff(TWO_FILE_PATCH).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = files;
        for path in ["a.rs", "b.rs"] {
            state.line_comments.insert(
                path.to_string(),
                vec![crate::app::LineComment {
                    location: crate::diff::DiffLineLocation {
                        old_line: None,
                        new_line: Some(2),
                    },
                    start: None,
                    text: format!("note on {path}"),
                    draft: false,
                    suggestion: None,
                    severity: crate::app::Severity::default(),
                    anchor_context: None,
                    start_anchor_context: None,
                    anchor_lost: false,
                    resolved: false,
                    carried: false,
                }],
            );
        }
    }
    app
}

fn cursor_position(app: &App) -> (usize, Option<usize>) {
    match &app.mode {
        AppMode::DiffViewer(state) => (state.selected_file, state.comment_cursor),
        _ => panic!("left the diff viewer"),
    }
}

#[test]
fn brace_keys_walk_comments_across_files_and_wrap() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_comments(dir.path());

    // With the cursor off, `}` activates it on the current file's comment.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('}'))).unwrap();
    assert_eq!(cursor_position(&app), (0, Some(2)));
    // Then across the file boundary — Tab could never leave a.rs.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('}'))).unwrap();
    assert_eq!(cursor_position(&app), (1, Some(2)));
    // And wraps back to the first comment in the review.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('}'))).unwrap();
    assert_eq!(cursor_position(&app), (0, Some(2)));
    // Backward from the first comment wraps to the last.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('{'))).unwrap();
    assert_eq!(cursor_position(&app), (1, Some(2)));
}

#[test]
fn brace_keys_report_when_the_review_has_no_comments() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_patch(dir.path(), SEARCH_PATCH);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('}'))).unwrap();
    assert_eq!(cursor_position(&app), (0, None));
    assert_eq!(
        app.message.as_deref(),
        Some("No comments in this review yet")
    );
}
