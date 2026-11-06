use super::*;

fn make_git_suggestion_review(workdir: &Path) -> App {
    run_git(workdir, &["init", "-q", "--initial-branch=main"]);
    run_git(workdir, &["config", "user.name", "AMF Test"]);
    run_git(workdir, &["config", "user.email", "amf@example.com"]);
    std::fs::write(workdir.join("a.rs"), "ctx\n").unwrap();
    std::fs::write(workdir.join(".gitignore"), ".claude/\n").unwrap();
    run_git(workdir, &["add", "a.rs", ".gitignore"]);
    run_git(workdir, &["commit", "-q", "-m", "base"]);
    run_git(workdir, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(workdir.join("a.rs"), "ctx\nadded line\n").unwrap();

    let snapshot = crate::diff::load_snapshot(workdir, None, false).unwrap();
    let mut app = make_review_app(workdir, &["a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = snapshot.files;
        state.branch = snapshot.branch;
        state.base_ref = snapshot.base_ref;
        state.base_commit = snapshot.base_commit;
    }
    app
}

fn add_replacement_suggestion(app: &mut App, replacement: &str) {
    handle_diff_viewer_key(app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(app, key(KeyCode::Char('S'))).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new(replacement.to_string());
    }
    handle_diff_viewer_key(app, key(KeyCode::Tab)).unwrap();
}

#[test]
fn x_applies_the_suggestion_under_the_cursor_and_refreshes() {
    let repo = tempfile::TempDir::new().unwrap();
    let mut app = make_git_suggestion_review(repo.path());
    add_replacement_suggestion(&mut app, "replacement");

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('x'))).unwrap();

    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.rs")).unwrap(),
        "ctx\nreplacement\n"
    );
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(state.applied_suggestions, vec!["a.rs:2"]);
            assert_eq!(state.pending_suggestion_count(), 0);
            assert!(!state.decisions.contains_key("a.rs"));
            let file = state
                .files
                .iter()
                .find(|file| file.path == "a.rs")
                .expect("refreshed source file");
            assert_eq!(file.new_content.as_deref(), Some("ctx\nreplacement\n"));
        }
        _ => panic!("expected refreshed diff viewer"),
    }
    assert!(
        app.message
            .as_deref()
            .is_some_and(|message| message.contains("Applied suggestion locally"))
    );
}

#[test]
fn finish_toggle_applies_all_suggestions_before_completing() {
    let repo = tempfile::TempDir::new().unwrap();
    let mut app = make_git_suggestion_review(repo.path());
    add_replacement_suggestion(&mut app, "replacement");

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('X'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.apply_suggestions_on_finish
    ));
    finish_review(&mut app);

    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.rs")).unwrap(),
        "ctx\nreplacement\n"
    );
    assert!(matches!(app.mode, AppMode::Viewing(_)));
    let message = app.message.as_deref().unwrap_or_default();
    assert!(
        message.contains("1 suggestion(s) applied locally (a.rs:2)"),
        "{message}"
    );
    let history =
        std::fs::read_to_string(repo.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(history.contains("**Local suggestion application:** 1 applied (a.rs:2)"));
    assert!(
        !history.contains("### Line Comments"),
        "a successfully applied suggestion should be recorded in history, not dispatched as feedback"
    );
}

#[test]
fn finish_reports_dirty_file_and_leaves_suggestion_for_the_agent() {
    let repo = tempfile::TempDir::new().unwrap();
    let mut app = make_git_suggestion_review(repo.path());
    add_replacement_suggestion(&mut app, "replacement");
    // Simulate an agent/user edit after the viewer loaded but before finish.
    std::fs::write(repo.path().join("a.rs"), "ctx\nchanged concurrently\n").unwrap();

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('X'))).unwrap();
    finish_review(&mut app);

    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.rs")).unwrap(),
        "ctx\nchanged concurrently\n"
    );
    let message = app.message.as_deref().unwrap_or_default();
    assert!(
        message.contains("1 suggestion(s) not applied locally"),
        "{message}"
    );
    let feedback =
        std::fs::read_to_string(repo.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(feedback.contains("file changed since the diff was loaded"));
    assert!(feedback.contains("```suggestion\nreplacement\n```"));
}

#[test]
fn line_cursor_activates_on_first_change_and_navigates() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // `c` activates the cursor on the first changed line (the added line,
    // index 1; index 0 is context).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    let cursor = |app: &App| match &app.mode {
        AppMode::DiffViewer(s) => s.comment_cursor,
        _ => panic!("expected diff viewer"),
    };
    assert_eq!(cursor(&app), Some(1));

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('k'))).unwrap();
    assert_eq!(cursor(&app), Some(0));
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('k'))).unwrap();
    assert_eq!(cursor(&app), Some(0), "clamps at the top");
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    assert_eq!(cursor(&app), Some(1), "clamps at the bottom");

    // `c` again deactivates the cursor.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    assert_eq!(cursor(&app), None);
}

#[test]
fn line_comment_is_written_to_feedback_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Activate cursor (on the added line), open the comment editor, type,
    // and submit with Tab.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(s) if s.editing_line_comment
    ));
    for c in "bug here".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.editing_line_comment);
            let comments = state.line_comments.get("a.rs").expect("comment stored");
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].text, "bug here");
            assert_eq!(comments[0].location.new_line, Some(2));
        }
        _ => panic!("expected diff viewer"),
    }

    // The comment auto-rejected the file (it now has a verdict), so q
    // opens the summary rather than the undecided-files confirmation, and
    // a second q writes the feedback file.
    finish_review(&mut app);
    let feedback =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(feedback.contains("## Line Comments"));
    assert!(feedback.contains("### a.rs:2"));
    assert!(feedback.contains("bug here"));
    assert!(feedback.contains("**Line comments:** 1"));
    assert!(matches!(app.mode, AppMode::Viewing(_)));
}

#[test]
fn resolving_the_only_comment_clears_auto_reject_and_omits_it_from_feedback() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Write a comment: it auto-rejects the file.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "bug here".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(s) if s.decisions.contains_key("a.rs")
    ));

    // Resolving the only comment clears the auto-set verdict — no open
    // thread means nothing left forcing "needs revision".
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('R'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.decisions.contains_key("a.rs"));
            let comments = state.line_comments.get("a.rs").unwrap();
            assert!(comments[0].resolved);
        }
        _ => panic!("expected diff viewer"),
    }

    // Reopening it puts the auto-reject back.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('R'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(s) if s.decisions.contains_key("a.rs")
    ));

    // Resolve again, then approve explicitly so the file has a verdict.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('R'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    finish_review(&mut app);

    // A resolved thread never reaches the round's actionable sections.
    // The all-approved round itself is still recorded for the history
    // browser.
    let history =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(history.contains("**Approved:** 1"));
    assert!(!history.contains("### Line Comments"));
    assert!(!history.contains("bug here"));
}

#[test]
fn unresolved_filter_is_skipped_when_no_open_thread_but_shown_once_one_exists() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // With no comments at all, cycling through the filters must skip
    // straight past `Unresolved` (mirroring the `Changed` skip when there's
    // no prior snapshot).
    for _ in 0..4 {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(s) if s.file_filter != FileFilter::Unresolved
        ));
    }

    // Land back on `All`, then add an open comment.
    while !matches!(&app.mode, AppMode::DiffViewer(s) if s.file_filter == FileFilter::All) {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "needs work".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    // Now the cycle should land on `Unresolved` with the file visible.
    let mut saw_unresolved = false;
    for _ in 0..5 {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        if let AppMode::DiffViewer(state) = &app.mode
            && state.file_filter == FileFilter::Unresolved
        {
            saw_unresolved = true;
            assert_eq!(state.visible_file_indices(), vec![0]);
        }
    }
    assert!(saw_unresolved, "Unresolved filter never appeared");
}

#[test]
fn carried_unresolved_comment_is_tagged_in_the_feedback_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "from last round".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    // Mark it carried, mirroring how a restored thread arrives from the
    // previous round's snapshot.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.line_comments.get_mut("a.rs").unwrap()[0].carried = true;
    }

    finish_review(&mut app);
    let feedback =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(
        feedback.contains("(unresolved from a previous round)"),
        "{feedback}"
    );
}

#[test]
fn suggestion_prefills_line_and_writes_suggestion_block() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Activate the cursor (on the added line), open the suggestion editor with
    // `S`: it pre-fills with the line's current text (prefix stripped).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.editing_suggestion);
            assert_eq!(state.feedback_editor.text(), "added line");
        }
        _ => panic!("expected diff viewer"),
    }

    // Replace the text and submit with Tab: a suggestion-only comment is
    // stored (empty prose).
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new("replaced line".to_string());
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.editing_suggestion);
            let comments = state.line_comments.get("a.rs").expect("comment stored");
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].text, "");
            assert_eq!(comments[0].suggestion.as_deref(), Some("replaced line"));
        }
        _ => panic!("expected diff viewer"),
    }

    // Finishing writes the suggestion as a fenced block in the feedback
    // file. The suggestion auto-rejected the file, so q opens the summary
    // (rather than the undecided confirmation) and a second q finishes.
    finish_review(&mut app);
    let feedback =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(feedback.contains("### a.rs:2"));
    assert!(feedback.contains("```suggestion\nreplaced line\n```"));
}

#[test]
fn emptying_a_suggestion_only_comment_deletes_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Create a suggestion-only comment.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new("x".to_string());
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::DiffViewer(s) if s.line_comments.get("a.rs").is_some_and(|c| c.len() == 1)
    ));

    // Re-open the suggestion and clear it: the prose-less comment is removed.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new(String::new());
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.line_comments.get("a.rs").is_none_or(|c| c.is_empty()));
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn suggestion_preserves_existing_comment_prose() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Write a prose comment first.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for ch in "prose".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    // Now add a suggestion on the same line: the prose survives.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new("newcode".to_string());
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            let comments = state.line_comments.get("a.rs").expect("comment stored");
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].text, "prose");
            assert_eq!(comments[0].suggestion.as_deref(), Some("newcode"));
        }
        _ => panic!("expected diff viewer"),
    }
}

/// Give the review app's single file a hunk with two added lines (plus a
/// leading context line) so a multi-line selection has room to span.
fn set_two_added_hunk(app: &mut App) {
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![DiffHunk {
            header: "@@ -1,1 +1,3 @@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 3,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " ctx".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+first".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+second".into(),
                },
            ],
        }];
    }
}

#[test]
fn multiline_comment_spans_selected_range() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_two_added_hunk(&mut app);

    // Activate the cursor (lands on the first added line, index 1), anchor a
    // selection with `v`, extend down one line, then comment the span.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for ch in "whole block".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            // The selection anchor is cleared once the comment is stored.
            assert!(state.comment_anchor.is_none());
            let comments = state.line_comments.get("a.rs").expect("comment stored");
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].text, "whole block");
            // Span covers new lines 2..3 (the two added lines).
            assert_eq!(comments[0].location.new_line, Some(3));
            assert_eq!(
                comments[0].start.and_then(|s| s.new_line),
                Some(2),
                "start anchors the first selected line"
            );
            assert!(comments[0].is_range());
        }
        _ => panic!("expected diff viewer"),
    }

    // The feedback file records the range anchor `a.rs:2-3`. The comment
    // auto-rejected the file, so q opens the summary without the
    // undecided-files confirm prompt, and a second q finishes.
    finish_review(&mut app);
    let feedback =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(feedback.contains("### a.rs:2-3"));
    assert!(feedback.contains("whole block"));
}

#[test]
fn editing_a_commented_line_preserves_its_span() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_two_added_hunk(&mut app);

    // Create a 2-line comment over the added lines.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for ch in "v1".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    // Move the cursor to the *end* of the span and re-open: editing must snap
    // onto the existing span and replace (not duplicate) the comment.
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for ch in " edited".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            let comments = state.line_comments.get("a.rs").expect("comment stored");
            assert_eq!(comments.len(), 1, "edit replaces rather than duplicates");
            assert_eq!(comments[0].text, "v1 edited");
            assert!(comments[0].is_range());
            assert_eq!(comments[0].start.and_then(|s| s.new_line), Some(2));
            assert_eq!(comments[0].location.new_line, Some(3));
        }
        _ => panic!("expected diff viewer"),
    }
}
