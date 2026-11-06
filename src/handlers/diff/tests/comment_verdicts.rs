use super::*;

/// Give the review app's single file two hunks (context + added line each),
/// so addressable indices 0,1 belong to the first hunk and 2,3 to the
/// second. Used by the jump-by-hunk tests.
fn set_two_hunks(app: &mut App) {
    let make_hunk = |old_start: usize, new_start: usize| DiffHunk {
        header: format!("@@ -{old_start},1 +{new_start},2 @@"),
        old_start,
        old_lines: 1,
        new_start,
        new_lines: 2,
        lines: vec![
            DiffLine {
                kind: DiffLineKind::Context,
                text: " ctx".into(),
            },
            DiffLine {
                kind: DiffLineKind::Added,
                text: "+added".into(),
            },
        ],
    };
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![make_hunk(1, 1), make_hunk(10, 11)];
    }
}

#[test]
fn bracket_keys_jump_cursor_between_hunks() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_two_hunks(&mut app);

    let cursor = |app: &App| match &app.mode {
        AppMode::DiffViewer(s) => s.comment_cursor,
        _ => panic!("expected diff viewer"),
    };

    // `]` with the cursor off activates it on the first hunk's start.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char(']'))).unwrap();
    assert_eq!(cursor(&app), Some(0));
    // `]` hops to the second hunk, then clamps there (no hunk after).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char(']'))).unwrap();
    assert_eq!(cursor(&app), Some(2));
    handle_diff_viewer_key(&mut app, key(KeyCode::Char(']'))).unwrap();
    assert_eq!(cursor(&app), Some(2), "clamps in the last hunk");

    // From mid-hunk, `[` first snaps back to the current hunk's start…
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    assert_eq!(cursor(&app), Some(3));
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('['))).unwrap();
    assert_eq!(cursor(&app), Some(2));
    // …then to the previous hunk, clamping at the first.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('['))).unwrap();
    assert_eq!(cursor(&app), Some(0));
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('['))).unwrap();
    assert_eq!(cursor(&app), Some(0), "clamps at the first hunk");
}

#[test]
fn line_comment_auto_rejects_file_and_removal_clears_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Storing a comment defaults the undecided file to "needs revision"
    // with empty feedback (the comment carries the specifics).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "bug".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.decisions.get("a.rs"),
                Some(&crate::app::ReviewDecision::Reject {
                    feedback: String::new(),
                    severity: crate::app::Severity::Suggestion,
                })
            );
            assert!(state.auto_rejected.contains("a.rs"));
        }
        _ => panic!("expected diff viewer"),
    }

    // Deleting the file's only comment (empty re-submit) clears the
    // auto-set verdict again.
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new(String::new());
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty());
            assert!(state.auto_rejected.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn explicit_verdict_wins_over_comment_auto_reject() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Comment (auto-rejects), then explicitly approve: the approval wins.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "note".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    let decision = |app: &App| match &app.mode {
        AppMode::DiffViewer(s) => s.decisions.get("a.rs").cloned(),
        _ => panic!("expected diff viewer"),
    };
    assert_eq!(decision(&app), Some(crate::app::ReviewDecision::Approve));

    // Editing the comment afterwards does not re-default the verdict.
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in " more".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    assert_eq!(decision(&app), Some(crate::app::ReviewDecision::Approve));
}

#[test]
fn skip_after_comment_clears_implicit_reject() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "note".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('s'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty(), "skip clears the verdict");
            assert!(state.auto_rejected.is_empty());
            // The comment itself is kept — only the verdict was cleared.
            assert!(
                state
                    .line_comments
                    .get("a.rs")
                    .is_some_and(|c| c.len() == 1)
            );
        }
        _ => panic!("expected diff viewer"),
    }
}

/// Seed an unaccepted AI draft comment on the review app's added line
/// (addressable index 1, new line 2 — where `c` places the cursor).
fn seed_draft_comment(app: &mut App) {
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.line_comments.insert(
            "a.rs".into(),
            vec![crate::app::LineComment {
                location: crate::diff::DiffLineLocation {
                    old_line: None,
                    new_line: Some(2),
                },
                start: None,
                text: "AI finding".into(),
                draft: true,
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

#[test]
fn ctrl_e_cycles_line_comment_severity() {
    use crate::app::Severity;
    let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Open the line-comment editor (defaults to Suggestion), cycle once with
    // Ctrl+E to Nit, type, and submit.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    handle_diff_viewer_key(&mut app, ctrl('e')).unwrap();
    for c in "typo".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            let comment = &state.line_comments.get("a.rs").unwrap()[0];
            assert_eq!(comment.severity, Severity::Nit);
            // A blocker line comment would drive the Blockers filter; a nit
            // does not.
            assert!(!state.file_has_blocker("a.rs"));
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn accepted_draft_auto_rejects_dismissed_draft_does_not() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Dismissing a draft leaves the file undecided (a draft was never a
    // kept comment).
    seed_draft_comment(&mut app);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('d'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }

    // Accepting a draft is a human-affirmed finding: it counts like a
    // hand-written comment and defaults the verdict.
    seed_draft_comment(&mut app);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.decisions.get("a.rs"),
                Some(&crate::app::ReviewDecision::Reject {
                    feedback: String::new(),
                    severity: crate::app::Severity::Suggestion,
                })
            );
            assert!(state.auto_rejected.contains("a.rs"));
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn implicit_reject_points_agent_at_line_comments() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "bug".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    // Auto-rejected, so the review is fully decided: q opens the summary
    // and a second q finishes.
    finish_review(&mut app);

    let feedback =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(feedback.contains("**Needs work:** 1"));
    // The rejection heading carries the (auto-reject default) severity tag.
    assert!(feedback.contains("#### a.rs — [suggestion]\n"));
    assert!(feedback.contains("(Needs revision — see this file's line comments below)"));
    assert!(feedback.contains("bug"));
}
