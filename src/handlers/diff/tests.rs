use super::{
    PATCH_JUMP_STEP, handle_diff_viewer_key, handle_diff_viewer_wheel,
    handle_review_harness_pick_key,
};
use crate::app::App;
use crate::app::{
    AppMode, DiffViewerFocus, DiffViewerLayout, DiffViewerState, FileFilter, ViewState,
};
use crate::diff::{DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind};
use crate::project::{
    AgentKind, Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

/// Finish a fully-decided review: the first `q` opens the pre-finish
/// summary, the second (`q` again, now handled by the summary modal)
/// actually writes/dispatches. Tests that only care about the end state
/// use this instead of a single `q` press.
fn finish_review(app: &mut App) {
    handle_diff_viewer_key(app, key(KeyCode::Char('q'))).unwrap();
    assert!(
        matches!(&app.mode, AppMode::DiffViewer(s) if s.summary_open),
        "expected the pre-finish summary to open"
    );
    handle_diff_viewer_key(app, key(KeyCode::Char('q'))).unwrap();
}

fn make_review_app(workdir: &Path, paths: &[&str]) -> App {
    let mut app = crate::app::App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut state = DiffViewerState::new(
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "sess".into(),
            "claude".into(),
            "Claude".into(),
            crate::project::SessionKind::Claude,
            VibeMode::Vibeless,
            true,
        ),
        workdir.to_path_buf(),
    );
    state.review = true;
    state.files = paths
        .iter()
        .map(|p| DiffFile {
            old_path: Some((*p).to_string()),
            path: (*p).to_string(),
            status: DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        })
        .collect();
    app.mode = AppMode::DiffViewer(state);
    app
}

/// A review app whose single file carries real hunks parsed from `patch`, so
/// `addressable_lines()` is non-empty (needed to exercise the line cursor and
/// diff search).
fn make_review_app_with_patch(workdir: &Path, patch: &str) -> App {
    let mut app = make_review_app(workdir, &["a.rs"]);
    let files = crate::diff::parse_unified_diff(patch).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = files;
    }
    app
}

/// Matches `SEARCH_PATCH` in the review.rs tests: addressable lines are
/// 0 alpha, 1 beta, 2 beta_two, 3 gamma_alpha, 4 delta.
const SEARCH_PATCH: &str = "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,4 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
+fn gamma_alpha() {}
 fn delta() {}
";

/// A single-file review app with `lines` added lines, focused on the patch.
fn make_long_patch_app(workdir: &Path, lines: usize) -> App {
    let mut patch =
        format!("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -0,0 +1,{lines} @@\n");
    for i in 0..lines {
        patch.push_str(&format!("+line {i}\n"));
    }
    let mut app = make_review_app_with_patch(workdir, &patch);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.focus = DiffViewerFocus::Patch;
    }
    app
}

fn patch_scroll_and_cursor(app: &App) -> (usize, Option<usize>) {
    match &app.mode {
        AppMode::DiffViewer(state) => (state.patch_scroll, state.comment_cursor),
        _ => unreachable!(),
    }
}

fn help_open(app: &App) -> bool {
    matches!(&app.mode, AppMode::DiffViewer(state) if state.help_open)
}

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

/// Give the review app's single file a real hunk (one context + one added
/// line) so it has addressable lines to put a cursor on.
fn set_single_hunk(app: &mut App) {
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![DiffHunk {
            header: "@@ -1,1 +1,2 @@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 2,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " ctx".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+added line".into(),
                },
            ],
        }];
    }
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Two files, each with the same shape: addressable lines are
/// 0 context, 1 removed, 2 added, 3 context.
const TWO_FILE_PATCH: &str = "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
 fn delta() {}
diff --git a/b.rs b/b.rs
index 3333333..4444444 100644
--- a/b.rs
+++ b/b.rs
@@ -1,3 +1,3 @@
 fn one() {}
-fn two() {}
+fn two_prime() {}
 fn three() {}
";

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

mod viewport_navigation;

mod search;

mod editor_launch;

mod finish_flow;

mod comment_suggestions;

mod overview;

mod file_navigation;

mod editor_behavior;

mod completion_handoff;

mod base_ref;

mod comment_verdicts;

mod file_tree;
