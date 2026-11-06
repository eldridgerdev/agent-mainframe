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

mod comment_navigation;

mod undo;
