use super::*;

#[test]
fn base_ref_prompt_opens_types_and_submits_into_override() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // `b` opens the prompt; verdict keys are suppressed while it is open.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('b'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.editing_base_ref));

    for c in "develop".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Backspace)).unwrap(); // -> "develo"

    // Enter submits: the override is recorded and the viewer re-enters
    // loading so the diff reloads against it.
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    match &app.mode {
        AppMode::DiffViewerLoading(s) => {
            assert_eq!(s.override_base_ref.as_deref(), Some("develo"));
            assert!(!s.editing_base_ref);
        }
        _ => panic!("expected loading state after submit"),
    }
}

#[test]
fn base_ref_prompt_escape_cancels_without_override() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('b'))).unwrap();
    for c in "main".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(s) => {
            assert!(!s.editing_base_ref);
            assert!(s.override_base_ref.is_none());
            assert!(s.base_ref_input.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn base_ref_prompt_blank_submit_clears_override() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.override_base_ref = Some("develop".into());
    }

    // Open (pre-fills with the current override), clear it, submit blank.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('b'))).unwrap();
    for _ in 0.."develop".len() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Backspace)).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::DiffViewerLoading(s) => assert!(s.override_base_ref.is_none()),
        _ => panic!("expected loading state after submit"),
    }
}
