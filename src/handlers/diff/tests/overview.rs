use super::*;

#[test]
fn walkthrough_is_noop_when_developer_note_exists() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state
            .review_notes
            .insert("a.rs".into(), "hand-written".into());
    }

    // `w` must not spawn a walkthrough when a developer note is present.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('w'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.walkthrough_child.is_none());
            assert!(state.generated_notes.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn walkthrough_for_binary_file_sets_message_without_spawning() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["img.png"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].is_binary = true;
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('w'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.walkthrough_child.is_none());
            assert!(
                state
                    .generated_notes
                    .get("img.png")
                    .is_some_and(|n| n.contains("Binary"))
            );
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn poll_walkthrough_without_child_is_noop() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    app.poll_review_walkthrough().unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert!(state.generated_notes.is_empty()),
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn shift_o_opens_changeset_overview_modal_and_q_closes_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // `O` first raises the pre-call notice; continuing runs the pass and
    // opens the overview modal.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('O'))).unwrap();
    assert!(matches!(&app.mode, AppMode::PromptPrecall(p)
            if p.prompt_id == crate::prompts::PromptId::ReviewChangesetOverview));
    app.precall_confirm().unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert!(state.changeset_overview_open),
        _ => panic!("expected diff viewer"),
    }

    // While the modal is open, q closes it rather than finishing the
    // review (the modal takes full precedence over the underlying keys).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert!(!state.changeset_overview_open),
        _ => panic!("expected diff viewer, not finished"),
    }
}

#[test]
fn cancelling_the_changeset_overview_pre_call_leaves_the_viewer_clean() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('O'))).unwrap();
    assert!(matches!(&app.mode, AppMode::PromptPrecall(_)));
    app.precall_cancel();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.changeset_overview_open, "no half-open modal");
            assert!(state.changeset_overview_child.is_none());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn capital_i_opens_interdiff_modal_and_q_closes_it() {
    let dir = tempfile::TempDir::new().unwrap();

    // Round 1: finish a review so a snapshot with content lands on disk.
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].new_content = Some("fn a() {}\n".into());
    }
    app.finish_final_review().unwrap();

    // Round 2: same file, different content.
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].new_content = Some("fn a() {\n    changed();\n}\n".into());
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('I'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert!(state.interdiff_open),
        _ => panic!("expected diff viewer"),
    }

    // While the modal is open, q closes it rather than finishing the
    // review (the modal takes full precedence over the underlying keys).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert!(!state.interdiff_open),
        _ => panic!("expected diff viewer, not finished"),
    }
}

#[test]
fn reopening_a_cached_overview_does_not_regenerate() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.changeset_overview = Some("cached overview".to_string());
    }
    app.message = Some("sentinel".to_string());

    app.open_changeset_overview();
    // Generation is skipped when a cached overview already exists, so the
    // message (which generation would overwrite) is untouched.
    assert_eq!(app.message.as_deref(), Some("sentinel"));
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.changeset_overview_open);
            assert!(state.changeset_overview_child.is_none());
        }
        _ => panic!("expected diff viewer"),
    }
}
